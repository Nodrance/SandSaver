use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

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
fn rasterize(cells: &mut [u8], track: &mut Track, w: usize, h: usize, o: &Obstacle, s: f32, prev: f32) {
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
    // The back buffer no longer matches the front one here.
    track.mark_dirty_rect(x0, x1, y0, y1);
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
    /// Back buffer. It equals `cells` except in the chunks flagged in `dirty`.
    scratch: Vec<u8>,
    track: Track,
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
            scratch: vec![0; w * h],
            track: Track::new(w, h),
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
            let seed = self.rng.next_u32();
            step(&self.cells, &mut self.scratch, &mut self.track, self.w, self.h, seed);
            std::mem::swap(&mut self.cells, &mut self.scratch);
        }
    }
    
    // -- obstacles ----------------------------------------------------------

    fn update_obstacles(&mut self, dt: f32) {
        let (w, h) = (self.w, self.h);
        for o in self.obstacles.iter_mut() {
            o.age += dt;
            let s = o.scale();
            if s != o.applied {
                rasterize(&mut self.cells, &mut self.track, w, h, o, s, o.applied);
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
                    self.track.mark_sand(x, 0);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Sand physics
// ---------------------------------------------------------------------------

const CHUNK: usize = 8;

fn chunk_count(w: usize) -> usize {
    w.div_ceil(CHUNK)
}

/// Bookkeeping that lets `step` visit only the parts of the grid that matter.
/// Everything is per 8-cell chunk, stored as one bit per chunk and row.
struct Track {
    nc: usize,
    wpr: usize,
    h: usize,
    /// Chunks where the back buffer may differ from the front buffer.
    dirty: Vec<u64>,
    /// Chunks that may change in the next step: they hold sand, or sand sits
    /// in the chunks above/diagonally above. Two sets, read and written
    /// alternately.
    cand: [Vec<AtomicU64>; 2],
    ci: usize,
    /// Per 64-cell strip, one word per row: bit set = the grain there falls
    /// straight down this step. Laid out strip-major so strips are independent.
    falls: Vec<u64>,
}

impl Track {
    fn new(w: usize, h: usize) -> Self {
        let nc = chunk_count(w);
        let wpr = nc.div_ceil(64);
        let bits = |n| (0..n).map(|_| AtomicU64::new(0)).collect::<Vec<_>>();
        Track {
            nc,
            wpr,
            h,
            dirty: vec![0; wpr * h],
            cand: [bits(wpr * h), bits(wpr * h)],
            ci: 0,
            falls: vec![0; w.div_ceil(64) * h],
        }
    }

    /// The back buffer is stale in this rectangle of cells.
    fn mark_dirty_rect(&mut self, x0: usize, x1: usize, y0: usize, y1: usize) {
        for y in y0..y1 {
            for c in x0 / CHUNK..=(x1 - 1) / CHUNK {
                self.dirty[y * self.wpr + (c >> 6)] |= 1u64 << (c & 63);
            }
        }
    }

    /// Sand was placed at (x, y) in the front buffer.
    fn mark_sand(&self, x: usize, y: usize) {
        mark_candidates(&self.cand[self.ci], self.wpr, self.nc, self.h, y, x / CHUNK);
    }
}

/// Flag chunk `c` of row `y` and the three chunks below it, which are the
/// ones whose cells sand in that chunk can move into.
fn mark_candidates(cand: &[AtomicU64], wpr: usize, nc: usize, h: usize, y: usize, c: usize) {
    let set = |y: usize, c: usize| {
        cand[y * wpr + (c >> 6)].fetch_or(1u64 << (c & 63), Ordering::Relaxed);
    };
    set(y, c);
    if y + 1 < h {
        for c2 in c.saturating_sub(1)..=(c + 1).min(nc - 1) {
            set(y + 1, c2);
        }
    }
}

/// One falling-sand step, double buffered: `dst` is computed purely from
/// `src`, so every output row is independent and rows run fully in parallel
/// with no waiting and no seams.
///
/// `dst` holds the state from two steps ago and is only repaired where needed
/// (see `Track`). On return `dst` is the new state and the roles flip: `src`
/// becomes the back buffer.
///
/// Each grain's intent depends only on `src`: fall straight down if that cell
/// is free, otherwise slide to a free lower diagonal. Conflicts over a target
/// cell are settled the same way from both sides: a straight fall beats a
/// diagonal, and two diagonals tie-break on a per-cell hash. A grain moves iff
/// it wins its target, and a target takes the grain that wins it, so grains are
/// conserved. Sand reaching the bottom row is removed on the following step.
fn step(src: &[u8], dst: &mut [u8], track: &mut Track, w: usize, h: usize, seed: u32) {
    let zeros = vec![0u8; w];
    let blocked = vec![255u8; w];
    let Track { nc, wpr, dirty, cand, ci, falls, .. } = track;
    let (nc, wpr) = (*nc, *wpr);
    let cand_cur = &cand[*ci][..];
    let cand_next = &cand[*ci ^ 1][..];
    compute_falls(src, falls, cand_cur, w, h, wpr, seed);
    let falls = &falls[..];
    dst.par_chunks_mut(w)
        .zip(dirty.par_chunks_mut(wpr))
        .enumerate()
        .with_min_len(8)
        .for_each(|(y, (out, flags))| {
            let cur = &src[y * w..(y + 1) * w];
            let up = if y > 0 { &src[(y - 1) * w..y * w] } else { &zeros[..] };
            let dn = if y + 1 < h { &src[(y + 1) * w..(y + 2) * w] } else { &blocked[..] };
            let hs_up = mix(seed, y.wrapping_sub(1) as u32);
            let hs = mix(seed, y as u32);
            let mc = FallRow { bits: falls, h, y };
            let mu = FallRow { bits: falls, h, y: y.wrapping_sub(1) };
            let row = Row { up, cur, dn, mc, mu, hs_up, hs, y, h, nc };
            process_row(&row, out, flags, &cand_cur[y * wpr..(y + 1) * wpr], cand_next, wpr);
        });
    *ci ^= 1;
}

#[derive(Clone, Copy)]
struct Row<'a> {
    up: &'a [u8],
    cur: &'a [u8],
    dn: &'a [u8],
    mc: FallRow<'a>,
    mu: FallRow<'a>,
    hs_up: u32,
    hs: u32,
    y: usize,
    h: usize,
    nc: usize,
}

const TIE: u32 = 0xA5A5_A5A5;

#[inline(always)]
fn is_sand(c: u8) -> bool {
    c != 0 && c < OBSTACLE_BASE
}

#[inline(always)]
fn mix(a: u32, b: u32) -> u32 {
    let mut z = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA6B);
    z ^= z >> 15;
    z = z.wrapping_mul(0x2C1B_3C6D);
    z ^= z >> 12;
    z = z.wrapping_mul(0x297A_2D39);
    z ^ (z >> 15)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Stay,
    Down,
    Left,
    Right,
}

/// One row of the per-step "falls straight down" bitmap (see `compute_falls`).
#[derive(Clone, Copy)]
struct FallRow<'a> {
    bits: &'a [u64],
    h: usize,
    y: usize,
}

impl FallRow<'_> {
    #[inline(always)]
    fn get(&self, x: usize) -> bool {
        (self.bits[(x >> 6) * self.h + self.y] >> (x & 63)) & 1 != 0
    }

    /// The eight cells of chunk `c`, one bit each.
    #[inline(always)]
    fn chunk(&self, c: usize) -> u8 {
        (self.bits[(c >> 3) * self.h + self.y] >> ((c & 7) * 8)) as u8
    }
}

/// Work out which grains fall straight down this step. A grain falls if the
/// cell below is empty or holds a grain that is certain to move away (falls,
/// slides, or is swept off the bottom row), so a whole column of sand above a
/// gap drops as one block, as it would in a bottom-up in-place update.
/// Columns are independent, so 64-cell strips run in parallel and each walks
/// its rows bottom-up. Only chunks that can hold sand are looked at.
///
/// A slide only counts as certain when nothing can contest its target (see
/// `slide_ok`); `resolve` then agrees. Uncertain slides just make the grain
/// above wait a step.
fn compute_falls(
    src: &[u8],
    falls: &mut [u64],
    cand: &[AtomicU64],
    w: usize,
    h: usize,
    wpr: usize,
    seed: u32,
) {
    let rd = |r: &[u8], i: usize| u64::from_le_bytes(r[i..i + 8].try_into().unwrap());
    let sandm = |v: u64| (((v & LOW7).wrapping_add(LOW7)) & !v) & HIGH;
    let nz = |v: u64| (((v & LOW7).wrapping_add(LOW7)) | v) & HIGH;
    let mask8 = |v: u64| ((v & HIGH) >> 7).wrapping_mul(0x0102_0408_1020_4080) >> 56;
    falls.par_chunks_mut(h).enumerate().for_each(|(s, col)| {
        let (kw, sh) = (s >> 3, (s & 7) * 8);
        // Cells of the row below that are certain to move away.
        let mut below = 0u64;
        for y in (0..h).rev() {
            let (mut fword, mut mword) = (0u64, 0u64);
            let mut bits = (cand[y * wpr + kw].load(Ordering::Relaxed) >> sh) & 0xff;
            let cur = &src[y * w..(y + 1) * w];
            let bottom = y + 1 == h;
            let dn = if bottom { cur } else { &src[(y + 1) * w..(y + 2) * w] };
            let hs = mix(seed, y as u32);
            while bits != 0 {
                let i = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let x = (s * 8 + i) * 8;
                if x >= w {
                    continue;
                }
                let lane = (below >> (i * 8)) & 0xff;
                let (mut f, mut m) = (0u64, 0u64);
                if bottom {
                    for l in 0..(w - x).min(8) {
                        if is_sand(cur[x + l]) {
                            m |= 1 << l; // swept off the grid
                        }
                    }
                } else if x >= 1 && x + 9 <= w {
                    let sand = mask8(sandm(rd(cur, x)));
                    let fall = sand & (mask8(!nz(rd(dn, x))) | lane);
                    let free = mask8(!nz(rd(dn, x - 1))) | mask8(!nz(rd(dn, x + 1)));
                    let mut cand = sand & !fall & free;
                    let mut slide = 0u64;
                    while cand != 0 {
                        let l = cand.trailing_zeros() as usize;
                        cand &= cand - 1;
                        if slide_ok(cur, dn, x + l, hs) {
                            slide |= 1 << l;
                        }
                    }
                    f = fall;
                    m = fall | slide;
                } else {
                    for l in 0..(w - x).min(8) {
                        if !is_sand(cur[x + l]) {
                            continue;
                        }
                        if dn[x + l] == 0 || (lane >> l) & 1 != 0 {
                            f |= 1 << l;
                            m |= 1 << l;
                        } else if slide_ok(cur, dn, x + l, hs) {
                            m |= 1 << l;
                        }
                    }
                }
                fword |= f << (i * 8);
                mword |= m << (i * 8);
            }
            col[y] = fword;
            below = mword;
        }
    });
}

/// The grain at `x` (not falling) is certain to slide diagonally: it has a free
/// lower diagonal and nothing can contest the target, neither a grain directly
/// above the target nor one that would slide in from the other side.
#[inline(always)]
fn slide_ok(cur: &[u8], dn: &[u8], x: usize, hs: u32) -> bool {
    let w = cur.len();
    let left = x > 0 && dn[x - 1] == 0;
    let right = x + 1 < w && dn[x + 1] == 0;
    let go_left = match (left, right) {
        (true, true) => (mix(x as u32, hs) >> 16) & 1 == 0,
        (true, false) => true,
        (false, true) => false,
        (false, false) => return false,
    };
    if go_left {
        !is_sand(cur[x - 1]) && (x < 2 || !is_sand(cur[x - 2]))
    } else {
        !is_sand(cur[x + 1]) && (x + 2 >= w || !is_sand(cur[x + 2]))
    }
}

/// Where the grain at `x` would like to go, ignoring competition.
#[inline(always)]
fn want(cur: &[u8], dn: &[u8], m: FallRow, x: usize, hs: u32) -> Dir {
    if !is_sand(cur[x]) {
        return Dir::Stay;
    }
    if m.get(x) {
        return Dir::Down;
    }
    let left = x > 0 && dn[x - 1] == 0;
    let right = x + 1 < dn.len() && dn[x + 1] == 0;
    match (left, right) {
        (true, true) => {
            if (mix(x as u32, hs) >> 16) & 1 == 0 {
                Dir::Left
            } else {
                Dir::Right
            }
        }
        (true, false) => Dir::Left,
        (false, true) => Dir::Right,
        (false, false) => Dir::Stay,
    }
}

/// Where the grain at `x` actually goes after losing to any competitor.
#[inline(always)]
fn resolve(cur: &[u8], dn: &[u8], m: FallRow, x: usize, hs: u32) -> Dir {
    match want(cur, dn, m, x, hs) {
        Dir::Left => {
            let tx = x - 1;
            if is_sand(cur[tx]) {
                return Dir::Stay; // the grain above the target falls straight in
            }
            if x >= 2
                && want(cur, dn, m, x - 2, hs) == Dir::Right
                && (mix(tx as u32, hs ^ TIE) >> 16) & 1 == 0
            {
                return Dir::Stay; // tie: bit 0 favours the grain from the left
            }
            Dir::Left
        }
        Dir::Right => {
            let tx = x + 1;
            if is_sand(cur[tx]) {
                return Dir::Stay;
            }
            if tx + 1 < cur.len()
                && want(cur, dn, m, tx + 1, hs) == Dir::Left
                && (mix(tx as u32, hs ^ TIE) >> 16) & 1 == 1
            {
                return Dir::Stay; // tie: bit 1 favours the grain from the right
            }
            Dir::Right
        }
        d => d,
    }
}

/// The grain arriving at cell `x` of `cur` (the row below `up`), or 0. `mu` is
/// the fall bitmap of `up`. Diagonal arrivals only happen into empty cells.
#[inline(always)]
fn incoming(up: &[u8], cur: &[u8], mu: FallRow, x: usize, hs_up: u32) -> u8 {
    let a = up[x];
    if is_sand(a) && mu.get(x) {
        return a;
    }
    if cur[x] != 0 {
        return 0;
    }
    if x > 0 {
        let l = up[x - 1];
        if is_sand(l) && resolve(up, cur, mu, x - 1, hs_up) == Dir::Right {
            return l;
        }
    }
    if x + 1 < up.len() {
        let r = up[x + 1];
        if is_sand(r) && resolve(up, cur, mu, x + 1, hs_up) == Dir::Left {
            return r;
        }
    }
    0
}

fn process_row(
    r: &Row,
    out: &mut [u8],
    dirty: &mut [u64],
    cand_cur: &[AtomicU64],
    cand_next: &[AtomicU64],
    wpr: usize,
) {
    let Row { up, cur, dn, mc, mu, hs_up, hs, y, h, nc } = *r;
    let w = cur.len();
    let bottom = y + 1 == h;
    let rd = |r: &[u8], i: usize| u64::from_le_bytes(r[i..i + 8].try_into().unwrap());
    // High bit of each byte set for sand / for any non-empty cell.
    let sandm = |v: u64| (((v & LOW7).wrapping_add(LOW7)) & !v) & HIGH;
    let nz = |v: u64| (((v & LOW7).wrapping_add(LOW7)) | v) & HIGH;

    for k in 0..wpr {
        let was_dirty = dirty[k];
        let mut visit = cand_cur[k].swap(0, Ordering::Relaxed) | was_dirty;
        let mut now_dirty = 0u64;
        while visit != 0 {
            let b = visit.trailing_zeros() as usize;
            visit &= visit - 1;
            let ci = k * 64 + b;
            let x = ci * CHUNK;
            if x >= w {
                continue;
            }
            let end = (x + CHUNK).min(w);

            let mut skip = false;
            let mut had_sand = false;
            if x >= 1 && x + 9 <= w {
                let cu = rd(cur, x);
                let sand_c = sandm(cu);
                let sand_u = sandm(rd(up, x)) | sandm(rd(up, x - 1)) | sandm(rd(up, x + 1));
                if sand_c | sand_u == 0 {
                    // Nothing here and nothing arriving: the chunk is unchanged.
                    skip = true;
                } else if !bottom && nz(cu) == HIGH {
                    // No empty cell to receive anything and every grain is fully
                    // supported (straight down and both diagonals occupied).
                    let blocked = nz(rd(dn, x)) & nz(rd(dn, x - 1)) & nz(rd(dn, x + 1));
                    skip = mc.chunk(ci) == 0 && sand_c & !blocked == 0;
                    had_sand = true;
                }
            }

            let has_sand;
            if skip {
                // Unchanged by this step; only repair a stale back buffer.
                if was_dirty >> b & 1 != 0 {
                    out[x..end].copy_from_slice(&cur[x..end]);
                }
                has_sand = had_sand;
            } else {
                now_dirty |= 1u64 << b;
                out[x..end].copy_from_slice(&cur[x..end]);
                for i in x..end {
                    let c = cur[i];
                    if c == 0 {
                        let g = incoming(up, cur, mu, i, hs_up);
                        if g != 0 {
                            out[i] = g;
                        }
                    } else if c < OBSTACLE_BASE {
                        // The grain leaves (falls, slides, or is swept off the
                        // bottom); the grain above may drop into its place.
                        if bottom || resolve(cur, dn, mc, i, hs) != Dir::Stay {
                            let a = up[i];
                            out[i] = if is_sand(a) && mu.get(i) { a } else { 0 };
                        }
                    }
                }
                has_sand = out[x..end].iter().any(|&c| is_sand(c));
            }
            if has_sand {
                mark_candidates(cand_next, wpr, nc, h, y, ci);
            }
        }
        dirty[k] = now_dirty;
    }
}
