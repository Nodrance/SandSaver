use rayon::prelude::*;
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

use crate::rng::Rng;

const LOW7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
const HIGH: u64 = 0x8080_8080_8080_8080;

/// Cell values: 0 = empty, 1..=127 = sand (palette index), 128..=255 = obstacle id.
pub const OBSTACLE_BASE: u8 = 128;
pub const SAND_COLORS: u32 = 127;

const GROW_TIME: f32 = 1.2;
const SHRINK_TIME: f32 = 1.2;
const EMITTER_FADE: f32 = 0.6;
const HUE_SPEED: f32 = 3.0; // palette indices per second
const EMIT_DENSITY: f32 = 0.25; // grains per emitter cell per substep

// ---------------------------------------------------------------------------
// Obstacles
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Kind {
    Rect { hl: f32, ht: f32 },
    Circle { r: f32 },
    /// Lower half of an annulus (a cup).
    Bowl { r: f32, t: f32 },
    Tri { v: [(f32, f32); 3] },
}

#[derive(Clone, Copy)]
struct Shape {
    kind: Kind,
    cos: f32,
    sin: f32,
}

impl Shape {
    fn new(kind: Kind, angle: f32) -> Self {
        Shape {
            kind,
            cos: angle.cos(),
            sin: angle.sin(),
        }
    }

    fn radius(&self) -> f32 {
        match self.kind {
            Kind::Rect { hl, ht } => hl.hypot(ht),
            Kind::Circle { r } | Kind::Bowl { r, .. } => r,
            Kind::Tri { v } => v.iter().map(|p| p.0.hypot(p.1)).fold(0.0, f32::max),
        }
    }

    /// Half extents of the axis-aligned bounding box.
    fn extent(&self) -> (f32, f32) {
        let (c, s) = (self.cos.abs(), self.sin.abs());
        match self.kind {
            Kind::Rect { hl, ht } => (hl * c + ht * s, hl * s + ht * c),
            Kind::Circle { r } | Kind::Bowl { r, .. } => (r, r),
            Kind::Tri { v } => {
                let (mut ex, mut ey) = (0.0f32, 0.0f32);
                for (lx, ly) in v {
                    ex = ex.max((lx * self.cos - ly * self.sin).abs());
                    ey = ey.max((lx * self.sin + ly * self.cos).abs());
                }
                (ex, ey)
            }
        }
    }

    /// Whether the point (dx, dy), relative to the shape centre, lies inside
    /// the shape scaled by `s`.
    fn contains(&self, dx: f32, dy: f32, s: f32) -> bool {
        if s <= 0.0 {
            return false;
        }
        let inv = 1.0 / s;
        let lx = (dx * self.cos + dy * self.sin) * inv;
        let ly = (-dx * self.sin + dy * self.cos) * inv;
        match self.kind {
            Kind::Rect { hl, ht } => lx.abs() <= hl && ly.abs() <= ht,
            Kind::Circle { r } => lx * lx + ly * ly <= r * r,
            Kind::Bowl { r, t } => {
                let d2 = lx * lx + ly * ly;
                let ri = r - t;
                ly >= 0.0 && d2 <= r * r && d2 >= ri * ri
            }
            Kind::Tri { v } => {
                let edge = |a: (f32, f32), b: (f32, f32)| (lx - b.0) * (a.1 - b.1) - (a.0 - b.0) * (ly - b.1);
                let d1 = edge(v[0], v[1]);
                let d2 = edge(v[1], v[2]);
                let d3 = edge(v[2], v[0]);
                let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
                let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
                !(neg && pos)
            }
        }
    }
}

struct Obstacle {
    shape: Shape,
    cx: f32,
    cy: f32,
    radius: f32,
    ex: f32,
    ey: f32,
    id: u8,
    age: f32,
    life: f32,
    applied: f32,
}

impl Obstacle {
    fn scale(&self) -> f32 {
        let grow = (self.age / GROW_TIME).clamp(0.0, 1.0);
        let shrink = ((self.life + SHRINK_TIME - self.age) / SHRINK_TIME).clamp(0.0, 1.0);
        let t = grow.min(shrink);
        t * t * (3.0 - 2.0 * t)
    }

    fn finished(&self) -> bool {
        self.age >= self.life + SHRINK_TIME && self.applied <= 0.0
    }
}

/// Paint the obstacle at scale `s`. `prev` is the scale it was last painted
/// at; only the region covered by either scale needs visiting.
fn rasterize(cells: &mut [u8], w: usize, h: usize, o: &Obstacle, s: f32, prev: f32) {
    let m = s.max(prev).max(0.0);
    let x0 = (o.cx - o.ex * m - 2.0).floor().max(0.0) as usize;
    let x1 = ((o.cx + o.ex * m + 2.0).ceil().max(0.0) as usize).min(w);
    let y0 = (o.cy - o.ey * m - 2.0).floor().max(0.0) as usize;
    let y1 = ((o.cy + o.ey * m + 2.0).ceil().max(0.0) as usize).min(h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    cells[y0 * w..y1 * w]
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(i, row)| {
            let dy = (y0 + i) as f32 + 0.5 - o.cy;
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - o.cx;
                let cell = &mut row[x];
                if o.shape.contains(dx, dy, s) {
                    if *cell < OBSTACLE_BASE {
                        *cell = o.id;
                    }
                } else if *cell == o.id {
                    *cell = 0;
                }
            }
        });
}

// ---------------------------------------------------------------------------
// Emitters
// ---------------------------------------------------------------------------

struct Emitter {
    x: f32,
    half_w: f32,
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
    /// Scale factor relative to a 1080p screen.
    unit: f32,
    substeps: usize,
    target_obstacles: usize,
    target_emitters: usize,
    hue: f32,
}

impl World {
    pub fn new(w: usize, h: usize) -> Self {
        let unit = h as f32 / 1080.0;
        let target_obstacles =
            ((w * h) as f32 / (unit * unit * 130_000.0)).round().clamp(3.0, 120.0) as usize;
        let target_emitters = (w as f32 / 560.0).round().clamp(3.0, 8.0) as usize;
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
            unit,
            substeps,
            target_obstacles,
            target_emitters,
            hue: 0.0,
        };
        for _ in 0..target_obstacles * 3 {
            if world.obstacles.len() >= target_obstacles {
                break;
            }
            world.spawn_obstacle();
        }
        for _ in 0..target_emitters {
            world.spawn_emitter(0.0);
        }
        world
    }

    pub fn update(&mut self, dt: f32) {
        self.hue = (self.hue + dt * HUE_SPEED) % SAND_COLORS as f32;
        self.update_obstacles(dt);
        self.update_emitters(dt);
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
            o.age += dt;
            let s = o.scale();
            if s != o.applied {
                rasterize(&mut self.cells, w, h, o, s, o.applied);
                o.applied = s;
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

        if self.obstacles.len() < self.target_obstacles {
            self.spawn_timer -= dt;
            if self.spawn_timer <= 0.0 {
                self.spawn_obstacle();
                self.spawn_timer = self.rng.range(0.2, 1.0);
            }
        }
    }

    fn random_shape(&mut self) -> Shape {
        let k = self.unit;
        let rng = &mut self.rng;
        match rng.below(100) {
            // Ramp
            0..=39 => {
                let angle = rng.sign() * rng.range(25.0, 55.0).to_radians();
                Shape::new(
                    Kind::Rect {
                        hl: rng.range(120.0, 280.0) * k,
                        ht: rng.range(6.0, 10.0) * k,
                    },
                    angle,
                )
            }
            // Circle
            40..=54 => Shape::new(Kind::Circle { r: rng.range(40.0, 110.0) * k }, 0.0),
            // Bowl
            55..=69 => Shape::new(
                Kind::Bowl {
                    r: rng.range(70.0, 140.0) * k,
                    t: rng.range(8.0, 14.0) * k,
                },
                rng.sign() * rng.range(0.0, 35.0).to_radians(),
            ),
            // Triangle
            70..=84 => {
                let bw = rng.range(50.0, 120.0) * k;
                let hh = rng.range(60.0, 150.0) * k;
                Shape::new(
                    Kind::Tri {
                        v: [(-bw, hh * 0.5), (bw, hh * 0.5), (0.0, -hh * 0.5)],
                    },
                    rng.range(0.0, std::f32::consts::TAU),
                )
            }
            // Box
            _ => Shape::new(
                Kind::Rect {
                    hl: rng.range(35.0, 110.0) * k,
                    ht: rng.range(30.0, 70.0) * k,
                },
                rng.range(0.0, std::f32::consts::PI),
            ),
        }
    }

    fn spawn_obstacle(&mut self) {
        if self.free_ids.is_empty() {
            return;
        }
        let shape = self.random_shape();
        let radius = shape.radius();
        let (ex, ey) = shape.extent();
        let (w, h) = (self.w as f32, self.h as f32);

        let x_lo = radius * 0.5;
        let x_hi = (w - radius * 0.5).max(x_lo + 1.0);
        let y_lo = h * 0.10 + radius * 0.7;
        let y_hi = (h - radius * 0.7).max(y_lo + 1.0);

        for _ in 0..30 {
            let cx = self.rng.range(x_lo, x_hi);
            let cy = self.rng.range(y_lo, y_hi);
            let clear = self.obstacles.iter().all(|o| {
                let d = (o.cx - cx).hypot(o.cy - cy);
                d >= (o.radius + radius) * 0.6
            });
            if clear {
                let id = self.free_ids.pop().unwrap();
                let life = self.rng.range(10.0, 26.0);
                self.obstacles.push(Obstacle {
                    shape,
                    cx,
                    cy,
                    radius,
                    ex,
                    ey,
                    id,
                    age: 0.0,
                    life,
                    applied: -1.0,
                });
                return;
            }
        }
    }

    // -- emitters -----------------------------------------------------------

    fn spawn_emitter(&mut self, age: f32) {
        let k = self.unit;
        let margin = self.w as f32 * 0.02;
        let x = self.rng.range(margin, self.w as f32 - margin);
        let half_w = self.rng.range(6.0, 24.0) * k;
        let life = self.rng.range(4.0, 10.0);
        let hue = self.rng.range(0.0, SAND_COLORS as f32);
        self.emitters.push(Emitter {
            x,
            half_w,
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
            let density = EMIT_DENSITY * e.strength();
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
/// Counts steps; the scan direction of each row alternates with it.
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
    *s & 1 == 1
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
        let rev = (y & 1 == 1) == odd;
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
