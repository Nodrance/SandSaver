#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gfx;
mod rng;
#[cfg(test)]
mod reference;
mod sim;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Fullscreen, Window, WindowId, WindowLevel};

use gfx::Gfx;
use sim::{World, OBSTACLE_BASE, SAND_COLORS};

/// Simulation rate. Presentation is vsynced; the sim advances at a fixed 60 Hz.
const STEP: f32 = 1.0 / 60.0;
const MOUSE_EXIT_DISTANCE: f64 = 12.0;
const MAX_INFLIGHT: usize = 4;
/// Integer resolution divider: the simulation grid is the screen resolution
/// divided by this (1 = full resolution, 2 = half, 3 = a third, ...). Each
/// cell is drawn as a SCALE x SCALE block of pixels.
const SCALE: u32 = 8;
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

fn build_lut() -> [u32; 256] {
    let mut lut = [0u32; 256];
    lut[0] = 0x0A0B12;
    for i in 1..=SAND_COLORS as usize {
        let hue = (i - 1) as f32 / SAND_COLORS as f32 * 360.0;
        lut[i] = hsv(hue, 0.55, 0.97);
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
struct SimThread {
    w: usize,
    h: usize,
    cmd: Sender<usize>,
    frames: Receiver<(Vec<u8>, usize)>,
    recycle: Sender<Vec<u8>>,
    /// Steps requested but not yet returned.
    inflight: usize,
}

impl SimThread {
    fn new(w: usize, h: usize) -> Self {
        let (cmd, cmd_rx) = mpsc::channel::<usize>();
        let (frames_tx, frames) = mpsc::channel::<(Vec<u8>, usize)>();
        let (recycle, recycle_rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut world = World::new(w, h);
            while let Ok(steps) = cmd_rx.recv() {
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
        if self.cmd.send(steps).is_ok() {
            self.inflight += steps;
        }
    }
}

struct App {
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    sim: Option<SimThread>,
    last_tick: Instant,
    started: Instant,
    accumulator: f32,
    mouse_origin: Option<PhysicalPosition<f64>>,
}

impl App {
    fn new() -> Self {
        App {
            window: None,
            gfx: None,
            sim: None,
            last_tick: Instant::now(),
            started: Instant::now(),
            accumulator: 0.0,
            mouse_origin: None,
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
        let (cw, ch) = gfx.cell_size();
        let (w, h) = (cw as usize, ch as usize);
        if self.sim.as_ref().map_or(true, |s| s.w != w || s.h != h) {
            self.sim = Some(SimThread::new(w, h));
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
        let attrs = Window::default_attributes()
            .with_title("Falling Sand")
            .with_decorations(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_fullscreen(Some(Fullscreen::Borderless(None)));
        let window = Arc::new(event_loop.create_window(attrs).expect("failed to create window"));
        window.set_cursor_visible(false);

        self.gfx = Some(pollster::block_on(Gfx::new(window.clone(), &build_lut(), SCALE)));
        self.window = Some(window);
        self.last_tick = Instant::now();
        self.started = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
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
    if let Some(arg) = std::env::args().nth(1) {
        let mode = arg.trim_start_matches(['/', '-']).to_lowercase();
        if mode.starts_with('c') || mode.starts_with('p') {
            return;
        }
    }

    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.run_app(&mut App::new()).expect("event loop error");
}

