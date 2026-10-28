use rayon::prelude::*;
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

use crate::rng::Rng;

mod generate;
mod shapes;

use generate::{Flow, Vanished};
use shapes::{Hole, Kind, Obstacle, Shape};

const LOW7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
const HIGH: u64 = 0x8080_8080_8080_8080;

/// Cell values: 0 = empty, 1..=127 = sand (palette index), 128..=255 = obstacle id.
pub const OBSTACLE_BASE: u8 = 128;
pub const SAND_COLORS: u32 = 127;

const GROW_TIME: f32 = 1.2;
const SHRINK_TIME: f32 = 1.2;
const EMITTER_FADE: f32 = 0.6;
const HUE_SPEED: f32 = 8.0; // palette indices per second

// ---------------------------------------------------------------------------
// Tunable parameters
//
// Every `*_VARIANCE` is a fraction: a value is drawn uniformly from
// `mean * (1 - variance) ..= mean * (1 + variance)`, so 0.0 means no variation
// and 0.5 means plus or minus 50%. Sizes are in pixels of a 1080p screen and
// scale with the resolution.
// ---------------------------------------------------------------------------

/// Average number of obstacles on screen. `None` picks a number from the screen
/// area (roughly one per 130 000 pixels at 1080p), clamped to 3..=120.
const OBSTACLE_COUNT: Option<usize> = None;
/// How much the number of obstacles wanted drifts over time.
const OBSTACLE_COUNT_VARIANCE: f32 = 0.25;
/// Average time in seconds an obstacle stays fully grown.
const OBSTACLE_DURATION: f32 = 15.0;
const OBSTACLE_DURATION_VARIANCE: f32 = 0.33;

/// Average number of sand streams. `None` picks a number from the screen width,
/// clamped to 3..=8.
const STREAM_COUNT: Option<usize> = Some(8);
/// How much the number of streams wanted drifts over time.
const STREAM_COUNT_VARIANCE: f32 = 0.5;
/// Average grains per stream cell per substep (0.0 to 1.0).
const STREAM_DENSITY: f32 = 0.50;
const STREAM_DENSITY_VARIANCE: f32 = 1.00;
/// Average stream width in pixels.
const STREAM_WIDTH: f32 = 30.0;
const STREAM_WIDTH_VARIANCE: f32 = 0.7;
/// Average time in seconds a stream flows.
const STREAM_DURATION: f32 = 3.0;
const STREAM_DURATION_VARIANCE: f32 = 0.8;

/// The wanted obstacle and stream counts are redrawn this often (seconds,
/// min and max).
const COUNT_REDRAW: (f32, f32) = (6.0, 14.0);

// --- What obstacles are made of ---------------------------------------------
//
// Relative frequencies of each kind. Holes cut into existing obstacles, so
// they only appear once there is something to cut; they count towards the
// obstacle count. A funnel counts as one obstacle.
const SLOPE_WEIGHT: f32 = 3.0;
const DIAMOND_WEIGHT: f32 = 1.2;
const CIRCLE_WEIGHT: f32 = 1.4;
const FUNNEL_WEIGHT: f32 = 1.0;
const LONG_HOLE_WEIGHT: f32 = 1.2;
const CIRCLE_HOLE_WEIGHT: f32 = 1.2;
/// Average time in seconds a hole stays open (never longer than its obstacle).
const HOLE_DURATION: f32 = 10.0;
const HOLE_DURATION_VARIANCE: f32 = 0.4;

// --- How streams and obstacles influence each other (0.0 to 1.0) --------------
//
/// How strongly streams pull obstacles towards them: which kind is built,
/// and building it under a stream rather than anywhere. 0.0 ignores streams.
const STREAMS_ATTRACT_OBSTACLES: f32 = 0.8;
/// How strongly obstacles pull streams towards them: new streams start above
/// obstacles rather than anywhere. 0.0 ignores obstacles.
const OBSTACLES_ATTRACT_STREAMS: f32 = 0.6;
/// How strongly a new obstacle is placed so that the stream it makes falls
/// onto another obstacle, rather than being the first acceptable spot.
const OBSTACLE_CHAINING: f32 = 0.7;
/// Streams wider than this (pixels, counting touching streams) are "wide".
const WIDE_STREAM: f32 = 60.0;

// --- Weirdness (0.0 follows the rules strictly, 1.0 lets chance take over) ---
//
/// Chance the kind of obstacle ignores what the streams suggest.
const WEIRD_KIND: f32 = 0.15;
/// Chance an obstacle or stream is placed anywhere instead of by the rules.
const WEIRD_PLACEMENT: f32 = 0.15;
/// Chance of unusual angles and proportions: slopes outside 30-60 degrees,
/// lopsided funnels, craters instead of clean cuts.
const WEIRD_SHAPE: f32 = 0.15;
/// Chance an obstacle may overlap others.
const WEIRD_OVERLAP: f32 = 0.10;
/// Chance a slope redirects a stream forwards instead of backwards.
const WEIRD_REDIRECT: f32 = 0.15;
/// Chance something may recreate a stream that has only just vanished.
const WEIRD_LOOPS: f32 = 0.10;
/// How long (seconds) a vanished stream is remembered.
const LOOP_MEMORY: f32 = 8.0;

/// `mean` varied by up to plus or minus the fraction `variance`.
fn vary(rng: &mut Rng, mean: f32, variance: f32) -> f32 {
    mean * (1.0 + variance * rng.range(-1.0, 1.0))
}

// ---------------------------------------------------------------------------
// Emitters
// ---------------------------------------------------------------------------

struct Emitter {
    x: f32,
    half_w: f32,
    density: f32,
    age: f32,
    life: f32,
    hue: f32,
}

impl Emitter {
    fn strength(&self) -> f32 {
        let fade_in = self.age / EMITTER_FADE;
        let fade_out = (self.life - self.age) / EMITTER_FADE;
        fade_in.min(fade_out).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// World
// ---------------------------------------------------------------------------

pub struct World {
    pub w: usize,
    pub h: usize,
    pub cells: Vec<u8>,
    /// Per row: non-zero if the row may contain sand.
    row_sand: Vec<AtomicU8>,
    rng: Rng,
    obstacles: Vec<Obstacle>,
    free_ids: Vec<u8>,
    emitters: Vec<Emitter>,
    spawn_timer: f32,
    /// Predicted streams, refreshed regularly, and where streams recently
    /// vanished.
    flows: Vec<Flow>,
    flow_timer: f32,
    vanished: Vec<Vanished>,
    /// Scale factor relative to a 1080p screen.
    unit: f32,
    substeps: usize,
    /// Average wanted counts, and the currently wanted counts drawn from them.
    base_obstacles: f32,
    base_emitters: f32,
    target_obstacles: usize,
    target_emitters: usize,
    count_timer: f32,
    hue: f32,
}

impl World {
    pub fn new(w: usize, h: usize) -> Self {
        let unit = h as f32 / 1080.0;
        let base_obstacles = OBSTACLE_COUNT.map_or_else(
            || ((w * h) as f32 / (unit * unit * 130_000.0)).round().clamp(3.0, 120.0),
            |n| n as f32,
        );
        let base_emitters =
            STREAM_COUNT.map_or_else(|| (w as f32 / 560.0).round().clamp(3.0, 8.0), |n| n as f32);
        let substeps = (h as f32 / 360.0).round().clamp(2.0, 4.0) as usize;

        let mut world = World {
            w,
            h,
            cells: vec![0; w * h],
            row_sand: (0..h).map(|_| AtomicU8::new(0)).collect(),
            rng: Rng::new(),
            obstacles: Vec::new(),
            free_ids: (OBSTACLE_BASE..=255).rev().collect(),
            emitters: Vec::new(),
            spawn_timer: 0.0,
            flows: Vec::new(),
            flow_timer: 0.0,
            vanished: Vec::new(),
            unit,
            substeps,
            base_obstacles,
            base_emitters,
            target_obstacles: 0,
            target_emitters: 0,
            count_timer: 0.0,
            hue: 0.0,
        };
        world.redraw_counts();
        let target_obstacles = world.target_obstacles;
        let target_emitters = world.target_emitters;
        // Streams first, so the obstacles have something to react to; the
        // rest of the streams then react to the obstacles.
        for _ in 0..target_emitters.div_ceil(2) {
            world.spawn_emitter(0.0);
        }
        for _ in 0..target_obstacles * 3 {
            if world.obstacle_count() >= target_obstacles {
                break;
            }
            world.spawn_obstacle();
        }
        for _ in world.emitters.len()..target_emitters {
            world.spawn_emitter(0.0);
        }
        world.flows = world.trace_flows();
        world
    }

    /// Obstacles and holes, counting a funnel once.
    fn obstacle_count(&self) -> usize {
        let holes = self.obstacles.iter().flat_map(|o| &o.holes).filter(|h| h.primary).count();
        self.obstacles.iter().filter(|o| !o.part).count() + holes
    }

    /// Draw new wanted obstacle and stream counts. Extra obstacles and streams
    /// are not removed; they just are not replaced when they expire.
    fn redraw_counts(&mut self) {
        let o = vary(&mut self.rng, self.base_obstacles, OBSTACLE_COUNT_VARIANCE);
        let e = vary(&mut self.rng, self.base_emitters, STREAM_COUNT_VARIANCE);
        self.target_obstacles = (o.round().max(0.0) as usize).min(120);
        self.target_emitters = e.round().max(0.0) as usize;
        self.count_timer = self.rng.range(COUNT_REDRAW.0, COUNT_REDRAW.1);
    }

    pub fn update(&mut self, dt: f32) {
        self.hue = (self.hue + dt * HUE_SPEED) % SAND_COLORS as f32;
        self.count_timer -= dt;
        if self.count_timer <= 0.0 {
            self.redraw_counts();
        }
        self.update_obstacles(dt);
        self.update_emitters(dt);
        self.flow_timer -= dt;
        if self.flow_timer <= 0.0 {
            self.flows = self.trace_flows();
            self.flow_timer = 0.5;
        }
        for v in &mut self.vanished {
            v.ttl -= dt;
        }
        self.vanished.retain(|v| v.ttl > 0.0);
        for _ in 0..self.substeps {
            self.emit();
            let bits = self.rng.next_u32();
            step(&mut self.cells, &self.row_sand, self.w, self.h, bits);
        }
    }

    // -- obstacles ----------------------------------------------------------

    fn update_obstacles(&mut self, dt: f32) {
        let (w, h) = (self.w, self.h);
        for o in self.obstacles.iter_mut() {
            shapes::update(&mut self.cells, w, h, o, dt);
            if !o.retired && o.age >= o.life {
                // The streams falling off it are about to vanish.
                o.retired = true;
                for f in self.flows.iter().filter(|f| f.src == Some(o.id)) {
                    self.vanished.push(Vanished { x: f.x, y: f.y, ttl: LOOP_MEMORY });
                }
            }
        }
        let free_ids = &mut self.free_ids;
        self.obstacles.retain(|o| {
            if o.finished() {
                free_ids.push(o.id);
                false
            } else {
                true
            }
        });

        if self.obstacle_count() < self.target_obstacles {
            self.spawn_timer -= dt;
            if self.spawn_timer <= 0.0 {
                self.spawn_obstacle();
                self.spawn_timer = self.rng.range(0.2, 1.0);
            }
        }
    }

    // -- emitters -----------------------------------------------------------

    fn spawn_emitter(&mut self, age: f32) {
        let k = self.unit;
        let x = self.emitter_x();
        let half_w = (vary(&mut self.rng, STREAM_WIDTH, STREAM_WIDTH_VARIANCE) * 0.5 * k).max(0.5);
        let density = vary(&mut self.rng, STREAM_DENSITY, STREAM_DENSITY_VARIANCE).clamp(0.0, 1.0);
        let life = vary(&mut self.rng, STREAM_DURATION, STREAM_DURATION_VARIANCE).max(2.0 * EMITTER_FADE);
        let hue = self.rng.range(0.0, SAND_COLORS as f32);
        self.emitters.push(Emitter {
            x,
            half_w,
            density,
            age: age.min(life * 0.5),
            life,
            hue,
        });
    }

    fn update_emitters(&mut self, dt: f32) {
        for e in self.emitters.iter_mut() {
            e.age += dt;
            e.hue = (e.hue + dt * HUE_SPEED) % SAND_COLORS as f32;
        }
        for e in self.emitters.iter().filter(|e| e.age >= e.life) {
            self.vanished.push(Vanished { x: e.x, y: 0.0, ttl: LOOP_MEMORY });
        }
        self.emitters.retain(|e| e.age < e.life);
        while self.emitters.len() < self.target_emitters {
            self.spawn_emitter(0.0);
        }
    }

    fn emit(&mut self) {
        let w = self.w;
        // A column that is solid with sand a little below the emitter is
        // jammed (a pile has grown up to it); feeding it only builds a spire.
        let probe = (((40.0 * self.unit) as usize).max(4)).min(self.h - 1);
        for e in &self.emitters {
            let density = e.density * e.strength();
            if density <= 0.0 {
                continue;
            }
            let lo = (e.x - e.half_w).max(0.0) as usize;
            let hi = ((e.x + e.half_w) as usize).min(w);
            for x in lo..hi {
                let jammed = (probe - 3..=probe).all(|y| is_sand(self.cells[y * w + x]));
                if !jammed && self.cells[x] == 0 && self.rng.chance(density) {
                    let idx = (e.hue as u32 + self.rng.below(4)) % SAND_COLORS;
                    self.cells[x] = 1 + idx as u8;
                    self.row_sand[0].store(1, Relaxed);
                }
            }
        }
    }
}


// ---------------------------------------------------------------------------
// Sand physics
// ---------------------------------------------------------------------------
//
// The grid is cut into column strips that run in parallel. Each strip does an
// ordinary serial bottom-up scan, in place: a grain falls if the cell below is
// empty, otherwise it slides to a free lower diagonal. Because rows are done
// bottom-up, a hole left by one grain is filled by the grain above in the same
// step, so stacks fall and heaps slump the way a single-threaded scan does.
//
// Grains cannot cross a strip boundary during a step. The boundaries move to a
// random place every step, so nothing can pile up against one.
//
// `rows[y]` is non-zero if row `y` may contain sand; empty rows are skipped.

const STRIP: usize = 128;
/// Counts steps; the scan direction alternates with it, and between pairs of
/// rows (alternating every row leaves 1-pixel stripes in a stream leaving a slope).
static FRAME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[inline(always)]
fn is_sand(v: u8) -> bool {
    v.wrapping_sub(1) < OBSTACLE_BASE - 1
}

/// 0x80 in every byte lane holding sand (1..=127).
#[inline(always)]
fn sandm(v: u64) -> u64 {
    (((v & LOW7).wrapping_add(LOW7)) & !v) & HIGH
}

#[inline(always)]
fn rd(s: &[u8]) -> u64 {
    u64::from_le_bytes(s.try_into().unwrap())
}

/// Xorshift coin flip.
#[inline(always)]
fn flip(s: &mut u64) -> bool {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 63 == 1
}

/// Move grains of `top` into `bot` (the row below). Returns whether any grain
/// stayed in `top`, and whether any landed in `bot`.
fn move_row(top: &mut [u8], bot: &mut [u8], rev: bool, rng: &mut u64) -> (bool, bool) {
    let n = top.len();
    let (mut stay, mut land) = (false, false);
    let chunks = n.div_ceil(8);
    for ci in 0..chunks {
        let c = if rev { chunks - 1 - ci } else { ci };
        let a = c * 8;
        let e = (a + 8).min(n);
        if e - a == 8 && sandm(rd(&top[a..e])) == 0 {
            continue;
        }
        for k in 0..e - a {
            let x = if rev { e - 1 - k } else { a + k };
            let v = top[x];
            if !is_sand(v) {
                continue;
            }
            let t = if bot[x] == 0 {
                x
            } else {
                let l = x > 0 && bot[x - 1] == 0;
                let r = x + 1 < n && bot[x + 1] == 0;
                match (l, r) {
                    (true, true) => {
                        if flip(rng) {
                            x - 1
                        } else {
                            x + 1
                        }
                    }
                    (true, false) => x - 1,
                    (false, true) => x + 1,
                    _ => {
                        stay = true;
                        continue;
                    }
                }
            };
            bot[t] = v;
            top[x] = 0;
            land = true;
        }
    }
    (stay, land)
}

/// Raw pointer to the grid, shared by the strips. Each strip only touches its
/// own columns.
struct Grid(*mut u8);
unsafe impl Send for Grid {}
unsafe impl Sync for Grid {}

/// Bottom-up scan of columns `x0..x1`. Returns which rows hold sand afterwards.
fn sweep(grid: &Grid, rows: &[u8], w: usize, x0: usize, x1: usize, seed: u64, odd: bool) -> Vec<u8> {
    let h = rows.len();
    let mut out = vec![0u8; h];
    let mut rng = seed | 1;
    for y in (0..h - 1).rev() {
        if rows[y] == 0 {
            continue;
        }
        // SAFETY: strips cover disjoint column ranges, and the two slices
        // are different rows, so nothing else aliases these bytes.
        let (top, bot) = unsafe {
            (
                std::slice::from_raw_parts_mut(grid.0.add(y * w + x0), x1 - x0),
                std::slice::from_raw_parts_mut(grid.0.add((y + 1) * w + x0), x1 - x0),
            )
        };
        let rev = ((y >> 1) & 1 == 1) == odd;
        let (stay, land) = move_row(top, bot, rev, &mut rng);
        out[y] |= stay as u8;
        out[y + 1] |= land as u8;
    }
    out
}

/// One simulation step; `bits` supplies the random strip boundaries.
pub(crate) fn step(cells: &mut [u8], rows: &[AtomicU8], w: usize, h: usize, bits: u32) {
    // Sand reaching the bottom disappears.
    if rows[h - 1].load(Relaxed) != 0 {
        for c in &mut cells[(h - 1) * w..] {
            if is_sand(*c) {
                *c = 0;
            }
        }
        rows[h - 1].store(0, Relaxed);
    }
    let cur: Vec<u8> = rows.iter().map(|r| r.load(Relaxed)).collect();

    let mut edges = vec![0];
    let mut x = bits as usize % STRIP;
    if x == 0 {
        x = STRIP;
    }
    while x < w {
        edges.push(x);
        x += STRIP;
    }
    edges.push(w);

    let grid = Grid(cells.as_mut_ptr());
    let grid = &grid;
    let base = (bits as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let odd = FRAME.fetch_add(1, Relaxed) & 1 == 1;
    let outs: Vec<Vec<u8>> = (0..edges.len() - 1)
        .into_par_iter()
        .map(|k| {
            let seed = base ^ (k as u64 + 1).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            sweep(grid, &cur, w, edges[k], edges[k + 1], seed, odd)
        })
        .collect();
    for (y, r) in rows.iter().enumerate() {
        r.store(outs.iter().any(|o| o[y] != 0) as u8, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference as r;

    fn flags(h: usize, v: u8) -> Vec<AtomicU8> {
        (0..h).map(|_| AtomicU8::new(v)).collect()
    }

    #[test]
    fn block_picture() {
        let mut rng = Rng::seeded(1);
        let mut cells = r::scene();
        let rows = flags(r::H, 0);
        let mut grains = Vec::new();
        for f in 0..2500 {
            r::emit(&mut cells, &mut rng, f);
            rows[0].store(1, Relaxed);
            step(&mut cells, &rows, r::W, r::H, rng.next_u32());
            if f % 500 == 499 {
                grains.push(cells.iter().filter(|&&v| is_sand(v)).count());
            }
        }
        r::write_png("target/new.png", r::W, r::H, &r::render(&cells));
        println!("NEW grains {grains:?}\n{}", r::ascii(&cells, r::W, r::H, 4, 6));
    }

    #[test]
    fn conserves_sand() {
        let (w, h) = (203, 101);
        let mut rng = Rng::seeded(7);
        let mut cells = vec![0u8; w * h];
        for c in &mut cells[(h - 1) * w..] {
            *c = 200;
        }
        for i in 0..w * (h - 1) {
            if rng.f32() < 0.2 {
                cells[i] = 1 + rng.below(100) as u8;
            }
        }
        let rows = flags(h, 1);
        let count = |c: &[u8]| c.iter().filter(|&&v| is_sand(v)).count();
        let n0 = count(&cells);
        for f in 0..400 {
            step(&mut cells, &rows, w, h, rng.next_u32());
            assert_eq!(count(&cells), n0, "step {f}");
        }
    }

    /// A stream falling on a 45 degree ramp; prints what falls off its end.
    fn ramp_run(serial: bool) {
        let (w, h) = (300usize, 260usize);
        let mut cells = vec![0u8; w * h];
        for i in 0..110 {
            for t in 0..3 {
                cells[(60 + i + t) * w + 60 + i] = 200;
            }
        }
        let rows = flags(h, 0);
        let mut rng = Rng::seeded(5);
        for f in 0..700 {
            for x in 55..75 {
                if cells[x] == 0 && rng.f32() < 0.3 {
                    cells[x] = 1 + (f / 20 % 100) as u8;
                    rows[0].store(1, Relaxed);
                }
            }
            if serial {
                r::step_serial(&mut cells, w, h, f, &mut rng);
            } else {
                step(&mut cells, &rows, w, h, rng.next_u32());
            }
        }
        println!("RAMP serial={serial} sand={}", cells.iter().filter(|&&v| is_sand(v)).count());
        let mut pairs = 0;
        for y in 180..250 {
            for x in 150..230 {
                if is_sand(cells[y * w + x]) && is_sand(cells[y * w + x + 1]) {
                    pairs += 1;
                }
            }
        }
        println!("STRIPE metric serial={serial} pairs={pairs}");
        if !serial {
            assert!(pairs < 20, "stream below a slope forms stripes: {pairs}");
        }
        for y in 135..200 {
            let s: String = (110..190)
                .map(|x| if is_sand(cells[y * w + x]) { '#' } else if cells[y * w + x] != 0 { '/' } else { '.' })
                .collect();
            println!("R{s}");
        }
    }

    #[test]
    fn ramp_picture() {
        ramp_run(false);
    }

    #[test]
    fn world_picture() {
        let mut world = World::new(1280, 720);
        for f in 1..=1800 {
            world.update(1.0 / 60.0);
            if f % 600 == 0 {
                r::write_png(&format!("target/world_{f}.png"), 1280, 720, &r::render(&world.cells));
            }
        }
        println!("WORLD\n{}", r::ascii(&world.cells, 1280, 720, 8, 12));
    }

    #[test]
    fn bench_4k() {
        let mut world = World::new(3840, 2160);
        for _ in 0..600 {
            world.update(1.0 / 60.0);
        }
        let t = std::time::Instant::now();
        for _ in 0..300 {
            world.update(1.0 / 60.0);
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / 300.0;
        let sand = world.cells.iter().filter(|&&v| is_sand(v)).count();
        println!("4K: {ms:.2} ms/frame, {sand} grains");
    }
}
