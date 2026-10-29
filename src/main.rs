#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gfx;
mod menu;
mod rng;
#[cfg(test)]
mod reference;
mod settings;
mod sim;
mod ui;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WKey, NamedKey};
use winit::window::{Fullscreen, Window, WindowId, WindowLevel};

use gfx::Gfx;
use menu::Menu;
use settings::{Settings, Store};
use sim::{World, OBSTACLE_BASE, SAND_COLORS};

/// Simulation rate. Presentation is vsynced; the sim advances at a fixed 60 Hz.
const STEP: f32 = 1.0 / 60.0;
const MOUSE_EXIT_DISTANCE: f64 = 12.0;
const MAX_INFLIGHT: usize = 4;
/// Integer resolution divider: the simulation grid is the screen resolution
/// divided by this (1 = full resolution, 2 = half, 3 = a third, ...). Each
/// cell is drawn as a SCALE x SCALE block of pixels.
const SCALE: u32 = 3;
const _: () = assert!(SCALE >= 1);
/// Input is ignored briefly after startup so launch-time key/mouse events don't exit immediately.
const INPUT_GRACE: Duration = Duration::from_millis(1500);

fn hsv(h: f32, s: f32, v: f32) -> u32 {
    let h = (h % 360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to8 = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    (to8(r) << 16) | (to8(g) << 8) | to8(b)
}

/// OKLCH to packed sRGB. Out-of-gamut colours are brought in by reducing
/// the chroma, which keeps lightness and hue.
fn oklch(l: f32, c: f32, h_deg: f32) -> u32 {
    let h = h_deg.to_radians();
    let linear = |c: f32| {
        let (a, b) = (c * h.cos(), c * h.sin());
        let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
        let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
        let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
        let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
        [
            4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_93 * s3,
            -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_4 * s3,
            -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3,
        ]
    };
    let inside = |rgb: &[f32; 3]| rgb.iter().all(|&v| (-0.001..=1.001).contains(&v));
    let (mut lo, mut hi) = (0.0, c);
    if !inside(&linear(c)) {
        for _ in 0..20 {
            let mid = (lo + hi) * 0.5;
            if inside(&linear(mid)) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
    } else {
        lo = c;
    }
    let encode = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round() as u32
    };
    let [r, g, b] = linear(lo);
    (encode(r) << 16) | (encode(g) << 8) | encode(b)
}

/// Palette: sand colors cycle around `hue_span` degrees from `hue_start`. A
/// range narrower than the whole wheel goes there and back so that the cycle
/// stays seamless.
fn build_lut(s: &Settings) -> [u32; 256] {
    let mut lut = [0u32; 256];
    lut[0] = 0x0A0B12;
    for i in 1..=SAND_COLORS as usize {
        let t = (i - 1) as f32 / SAND_COLORS as f32;
        let along = if s.hue_span >= 359.5 {
            t
        } else if t < 0.5 {
            2.0 * t
        } else {
            2.0 - 2.0 * t
        };
        lut[i] = oklch(s.sand_lightness, s.sand_chroma, s.hue_start + s.hue_span * along);
    }
    for id in OBSTACLE_BASE as usize..256 {
        let hue = 200.0 + ((id * 37) % 60) as f32;
        let val = 0.40 + 0.14 * (((id * 53) % 8) as f32 / 8.0);
        lut[id] = hsv(hue, 0.22, val);
    }
    lut
}

/// Runs the simulation on a background thread so it overlaps with rendering.
/// Finished frames come back as snapshots of the cell grid.
enum Msg {
    /// Advance the simulation this many steps and send back a snapshot.
    Steps(usize),
    Settings(Settings),
    /// Start over with a fresh world.
    Restart,
}

struct SimThread {
    w: usize,
    h: usize,
    cmd: Sender<Msg>,
    frames: Receiver<(Vec<u8>, usize)>,
    recycle: Sender<Vec<u8>>,
    /// Steps requested but not yet returned.
    inflight: usize,
}

impl SimThread {
    fn new(w: usize, h: usize, settings: Settings) -> Self {
        let (cmd, cmd_rx) = mpsc::channel::<Msg>();
        let (frames_tx, frames) = mpsc::channel::<(Vec<u8>, usize)>();
        let (recycle, recycle_rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut settings = settings;
            let mut world = World::new(w, h, settings);
            while let Ok(msg) = cmd_rx.recv() {
                let steps = match msg {
                    Msg::Steps(n) => n,
                    Msg::Settings(s) => {
                        settings = s;
                        world.apply_settings(s);
                        0
                    }
                    Msg::Restart => {
                        world = World::new(w, h, settings);
                        0
                    }
                };
                for _ in 0..steps {
                    world.update(STEP);
                }
                let mut buf = recycle_rx.try_recv().unwrap_or_default();
                buf.clear();
                buf.extend_from_slice(&world.cells);
                if frames_tx.send((buf, steps)).is_err() {
                    break;
                }
            }
        });
        SimThread {
            w,
            h,
            cmd,
            frames,
            recycle,
            inflight: 0,
        }
    }

    fn run(&mut self, steps: usize) {
        if self.cmd.send(Msg::Steps(steps)).is_ok() {
            self.inflight += steps;
        }
    }

    /// Tell the sim thread something other than "step"; it answers with a
    /// snapshot of zero steps.
    fn send(&self, msg: Msg) {
        let _ = self.cmd.send(msg);
    }
}

/// Panel pixel size for a window this tall.
fn ui_scale(height: u32) -> u32 {
    ((height as f32 / 360.0).round() as u32).clamp(1, 6)
}

fn translate(event: &KeyEvent) -> Option<menu::Key> {
    use menu::Key;
    if let WKey::Named(n) = &event.logical_key {
        match n {
            NamedKey::ArrowUp => return Some(Key::Up),
            NamedKey::ArrowDown => return Some(Key::Down),
            NamedKey::ArrowLeft => return Some(Key::Left),
            NamedKey::ArrowRight => return Some(Key::Right),
            NamedKey::Enter => return Some(Key::Enter),
            NamedKey::Escape => return Some(Key::Esc),
            NamedKey::Backspace => return Some(Key::Backspace),
            NamedKey::Tab => return Some(Key::Tab),
            _ => {}
        }
    }
    event.text.as_ref().and_then(|t| t.chars().find(|c| !c.is_control())).map(Key::Char)
}

struct App {
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    sim: Option<SimThread>,
    last_tick: Instant,
    started: Instant,
    accumulator: f32,
    mouse_origin: Option<PhysicalPosition<f64>>,
    settings: Settings,
    /// Present in settings mode only.
    menu: Option<Menu>,
    mult: f32,
    /// The menu image must be redrawn (it changed, or the window did).
    overlay_dirty: bool,
    overlay_for: (u32, u32),
}

impl App {
    fn new(configure: bool) -> Self {
        let store = Store::system();
        let settings = store.load_settings();
        App {
            window: None,
            gfx: None,
            sim: None,
            last_tick: Instant::now(),
            started: Instant::now(),
            accumulator: 0.0,
            mouse_origin: None,
            settings,
            menu: configure.then(|| Menu::new(settings, store)),
            mult: 1.0,
            overlay_dirty: true,
            overlay_for: (0, 0),
        }
    }

    /// React to what a menu key press asked for.
    fn apply(&mut self, event_loop: &ActiveEventLoop, fx: menu::Effect) {
        self.overlay_dirty = true;
        if fx.quit {
            if let Some(menu) = &self.menu {
                menu.save();
            }
            event_loop.exit();
            return;
        }
        if fx.changed {
            if let Some(menu) = &self.menu {
                self.settings = menu.settings;
            }
            if let Some(sim) = &self.sim {
                sim.send(Msg::Settings(self.settings));
            }
            if let Some(gfx) = &self.gfx {
                gfx.set_lut(&build_lut(&self.settings));
            }
        }
        if fx.restart {
            if let Some(sim) = &self.sim {
                sim.send(Msg::Restart);
            }
        }
    }

    fn config_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.apply(event_loop, menu::Effect { quit: true, ..Default::default() }),
            WindowEvent::ModifiersChanged(m) => {
                let m = m.state();
                self.mult = match (m.shift_key(), m.control_key()) {
                    (false, false) => 1.0,
                    (true, false) => 10.0,
                    (false, true) => 100.0,
                    (true, true) => 1000.0,
                };
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if let (Some(key), Some(menu)) = (translate(&event), &mut self.menu) {
                    let fx = menu.key(key, self.mult);
                    self.apply(event_loop, fx);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn redraw(&mut self) {
        let (Some(window), Some(gfx)) = (&self.window, &mut self.gfx) else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        if gfx.size() != (size.width, size.height) {
            gfx.resize(size.width, size.height);
        }
        if let Some(menu) = &self.menu {
            if self.overlay_dirty || self.overlay_for != (size.width, size.height) {
                if menu.visible {
                    let img = ui::render(&menu.view(), ui_scale(size.height), size.height);
                    gfx.set_overlay(Some((img.w, img.h, &img.rgba)));
                } else {
                    gfx.set_overlay(None);
                }
                self.overlay_dirty = false;
                self.overlay_for = (size.width, size.height);
            }
        }
        let (cw, ch) = gfx.cell_size();
        let (w, h) = (cw as usize, ch as usize);
        if self.sim.as_ref().map_or(true, |s| s.w != w || s.h != h) {
            self.sim = Some(SimThread::new(w, h, self.settings));
            self.accumulator = STEP; // kick off the first step immediately
        }
        let sim = self.sim.as_mut().unwrap();

        // Upload the newest snapshot the sim thread has finished.
        let mut latest: Option<Vec<u8>> = None;
        while let Ok((buf, steps)) = sim.frames.try_recv() {
            sim.inflight -= steps;
            if let Some(old) = latest.replace(buf) {
                let _ = sim.recycle.send(old);
            }
        }
        if let Some(buf) = latest {
            gfx.upload(&buf);
            let _ = sim.recycle.send(buf);
        }

        // Fixed-step simulation driven by wall-clock time. A couple of steps
        // may be queued so the sim thread never idles waiting for a frame.
        let now = Instant::now();
        let mut dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        if (dt - STEP).abs() < 0.002 {
            dt = STEP; // vsync jitter on a 60 Hz display: don't drop/double steps
        }
        self.accumulator = (self.accumulator + dt).min(4.0 * STEP);
        let due = (self.accumulator / STEP) as usize;
        let n = due.min(MAX_INFLIGHT.saturating_sub(sim.inflight));
        if n > 0 {
            sim.run(n);
            self.accumulator -= n as f32 * STEP;
        }
        gfx.present();
    }
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let configure = self.menu.is_some();
        let attrs = Window::default_attributes();
        let attrs = if configure {
            attrs.with_title("Falling Sand Settings").with_inner_size(LogicalSize::new(1280.0, 720.0))
        } else {
            attrs
                .with_title("Falling Sand")
                .with_decorations(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_fullscreen(Some(Fullscreen::Borderless(None)))
        };
        let window = Arc::new(event_loop.create_window(attrs).expect("failed to create window"));
        window.set_cursor_visible(configure);

        self.gfx = Some(pollster::block_on(Gfx::new(window.clone(), &build_lut(&self.settings), SCALE)));
        self.window = Some(window);
        self.last_tick = Instant::now();
        self.started = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if self.menu.is_some() {
            self.config_event(event_loop, event);
            return;
        }
        let armed = self.started.elapsed() >= INPUT_GRACE;
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }
                if !armed => {}
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                event_loop.exit()
            }
            WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => event_loop.exit(),
            WindowEvent::CursorMoved { position, .. } => match self.mouse_origin {
                _ if !armed => self.mouse_origin = Some(position),
                None => self.mouse_origin = Some(position),
                Some(o) => {
                    if (position.x - o.x).hypot(position.y - o.y) > MOUSE_EXIT_DISTANCE {
                        event_loop.exit();
                    }
                }
            },
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Poll);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

fn main() {
    // Windows screensaver protocol: /s = run, /c = configure, /p = preview.
    let mut configure = false;
    if let Some(arg) = std::env::args().nth(1) {
        let mode = arg.trim_start_matches(['/', '-']).to_lowercase();
        if mode.starts_with('p') {
            return;
        }
        configure = mode.starts_with('c');
    }

    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.run_app(&mut App::new(configure)).expect("event loop error");
}
