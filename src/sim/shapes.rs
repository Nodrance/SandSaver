use rayon::prelude::*;

use super::{GROW_TIME, OBSTACLE_BASE, SHRINK_TIME};

/// Vertical step used when scanning a column of a shape.
const SCAN_STEP: f32 = 2.0;

#[derive(Clone, Copy)]
pub enum Kind {
    Rect { hl: f32, ht: f32 },
    Circle { r: f32 },
    /// Parallelogram with vertical ends, unrotated: `|x| <= hl` and
    /// `|y - slope * x| <= vt`.
    Para { hl: f32, vt: f32, slope: f32 },
}

/// How much of a shape exists. Compact shapes (circles, squares) grow and
/// shrink uniformly: `hi` is the scale. Long shapes (slopes, holes, funnel
/// walls) are swept: the part between `lo` and `hi`, measured in pixels along
/// the long axis from the upper end, exists.
#[derive(Clone, Copy, PartialEq)]
pub struct Phase {
    pub lo: f32,
    pub hi: f32,
}

impl Phase {
    pub const NONE: Phase = Phase { lo: 0.0, hi: 0.0 };

    pub fn gone(&self) -> bool {
        self.hi <= self.lo
    }
}

#[derive(Clone, Copy)]
pub struct Shape {
    pub kind: Kind,
    cos: f32,
    sin: f32,
}

impl Shape {
    pub fn new(kind: Kind, angle: f32) -> Self {
        Shape {
            kind,
            cos: angle.cos(),
            sin: angle.sin(),
        }
    }

    pub fn radius(&self) -> f32 {
        match self.kind {
            Kind::Rect { hl, ht } => hl.hypot(ht),
            Kind::Circle { r } => r,
            Kind::Para { hl, vt, slope } => hl.hypot(hl * slope.abs() + vt),
        }
    }

    /// Half extents of the axis-aligned bounding box.
    pub fn extent(&self) -> (f32, f32) {
        let (c, s) = (self.cos.abs(), self.sin.abs());
        match self.kind {
            Kind::Rect { hl, ht } => (hl * c + ht * s, hl * s + ht * c),
            Kind::Circle { r } => (r, r),
            Kind::Para { hl, vt, slope } => (hl, hl * slope.abs() + vt),
        }
    }

    /// Half of the smallest dimension.
    pub fn thickness(&self) -> f32 {
        match self.kind {
            Kind::Rect { hl, ht } => hl.min(ht),
            Kind::Circle { r } => r,
            Kind::Para { vt, .. } => vt,
        }
    }

    /// Long thin shapes are swept in along their length; others are scaled.
    pub fn sweeps(&self) -> bool {
        match self.kind {
            Kind::Rect { hl, ht } => hl >= 2.0 * ht,
            Kind::Circle { .. } => false,
            Kind::Para { .. } => true,
        }
    }

    fn half_len(&self) -> f32 {
        match self.kind {
            Kind::Rect { hl, .. } | Kind::Para { hl, .. } => hl,
            Kind::Circle { r } => r,
        }
    }

    /// Whether the upper end of the long axis is at positive local x.
    fn flipped(&self) -> bool {
        match self.kind {
            Kind::Rect { .. } => self.sin < 0.0,
            Kind::Para { slope, .. } => slope < 0.0,
            Kind::Circle { .. } => false,
        }
    }

    /// How fast (pixels per second along the axis) a swept shape grows so that
    /// its lower end descends at `speed`, the speed sand falls at.
    fn rate(&self, speed: f32) -> f32 {
        let v = match self.kind {
            Kind::Rect { .. } => self.sin.abs(),
            Kind::Para { slope, .. } => slope.abs(),
            Kind::Circle { .. } => 1.0,
        };
        speed / v.max(0.12)
    }

    /// How long the shape takes to disappear after its lifetime.
    pub fn out_time(&self, speed: f32) -> f32 {
        if self.sweeps() {
            2.0 * self.half_len() / self.rate(speed)
        } else {
            SHRINK_TIME
        }
    }

    /// What exists `age` seconds after creation, for a shape that holds for
    /// `life` seconds.
    pub fn phase(&self, age: f32, life: f32, speed: f32) -> Phase {
        if self.sweeps() {
            let r = self.rate(speed);
            let len = 2.0 * self.half_len();
            // Exactly gone once the time is up, whatever the rounding.
            let lo = if age >= life + len / r { len } else { ((age - life) * r).clamp(0.0, len) };
            Phase {
                lo,
                hi: (age * r).clamp(0.0, len),
            }
        } else {
            Phase {
                lo: 0.0,
                hi: envelope(age, life),
            }
        }
    }

    /// Whether the point (dx, dy), relative to the shape centre, lies inside
    /// the part of the shape that exists in phase `ph`.
    pub fn covers(&self, dx: f32, dy: f32, ph: Phase) -> bool {
        if ph.gone() {
            return false;
        }
        if !self.sweeps() {
            return self.contains(dx, dy, ph.hi);
        }
        if !self.contains(dx, dy, 1.0) {
            return false;
        }
        let lx = dx * self.cos + dy * self.sin;
        let hl = self.half_len();
        let u = if self.flipped() { hl - lx } else { hl + lx };
        u >= ph.lo && u <= ph.hi
    }

    /// Bounding box of what changes between two phases of a shape at (cx, cy).
    pub fn change_region(&self, cx: f32, cy: f32, old: Phase, new: Phase, w: usize, h: usize) -> Option<Region> {
        if !self.sweeps() {
            let (ex, ey) = self.extent();
            let m = old.hi.max(new.hi).max(0.0);
            return region(cx, cy, ex * m, ey * m, w, h);
        }
        let mut out = None;
        for (a, b) in [(old.hi, new.hi), (old.lo, new.lo)] {
            let (a, b) = (a.min(b), a.max(b));
            if b > a {
                if let Some(r) = self.span_region(cx, cy, a, b, w, h) {
                    out = Some(union(out, r));
                }
            }
        }
        out
    }

    /// Bounding box of the part of a swept shape between `u0` and `u1`.
    fn span_region(&self, cx: f32, cy: f32, u0: f32, u1: f32, w: usize, h: usize) -> Option<Region> {
        let hl = self.half_len();
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for u in [u0, u1] {
            let lx = if self.flipped() { hl - u } else { u - hl };
            let (a, b) = match self.kind {
                Kind::Rect { ht, .. } => (-ht, ht),
                Kind::Para { vt, slope, .. } => (slope * lx - vt, slope * lx + vt),
                Kind::Circle { .. } => (0.0, 0.0),
            };
            for ly in [a, b] {
                let dx = lx * self.cos - ly * self.sin;
                let dy = lx * self.sin + ly * self.cos;
                x0 = x0.min(dx);
                x1 = x1.max(dx);
                y0 = y0.min(dy);
                y1 = y1.max(dy);
            }
        }
        region(cx + (x0 + x1) * 0.5, cy + (y0 + y1) * 0.5, (x1 - x0) * 0.5, (y1 - y0) * 0.5, w, h)
    }

    /// Whether the point (dx, dy), relative to the shape centre, lies inside
    /// the whole shape scaled by `s`.
    pub fn contains(&self, dx: f32, dy: f32, s: f32) -> bool {
        if s <= 0.0 {
            return false;
        }
        let inv = 1.0 / s;
        let lx = (dx * self.cos + dy * self.sin) * inv;
        let ly = (-dx * self.sin + dy * self.cos) * inv;
        match self.kind {
            Kind::Rect { hl, ht } => lx.abs() <= hl && ly.abs() <= ht,
            Kind::Circle { r } => lx * lx + ly * ly <= r * r,
            Kind::Para { hl, vt, slope } => lx.abs() <= hl && (ly - slope * lx).abs() <= vt,
        }
    }
}

/// Grow in, hold for `life` seconds, shrink out; smoothed.
fn envelope(age: f32, life: f32) -> f32 {
    let grow = (age / GROW_TIME).clamp(0.0, 1.0);
    let shrink = ((life + SHRINK_TIME - age) / SHRINK_TIME).clamp(0.0, 1.0);
    let t = grow.min(shrink);
    t * t * (3.0 - 2.0 * t)
}

/// Pixel rectangle `x0, x1, y0, y1` (end-exclusive).
pub type Region = (usize, usize, usize, usize);

fn region(cx: f32, cy: f32, ex: f32, ey: f32, w: usize, h: usize) -> Option<Region> {
    let x0 = (cx - ex - 2.0).floor().max(0.0) as usize;
    let x1 = ((cx + ex + 2.0).ceil().max(0.0) as usize).min(w);
    let y0 = (cy - ey - 2.0).floor().max(0.0) as usize;
    let y1 = ((cy + ey + 2.0).ceil().max(0.0) as usize).min(h);
    (x0 < x1 && y0 < y1).then_some((x0, x1, y0, y1))
}

/// A cut-out of an obstacle, in world coordinates.
#[derive(Clone)]
pub struct Hole {
    pub shape: Shape,
    pub cx: f32,
    pub cy: f32,
    pub age: f32,
    pub life: f32,
    /// Fall speed (pixels per second) it is swept in and out at.
    speed: f32,
    out: f32,
    /// Phase it was last painted at.
    applied: Phase,
    /// Counts towards the obstacle count (one copy per hole).
    pub primary: bool,
}

impl Hole {
    pub fn new(shape: Shape, cx: f32, cy: f32, life: f32, primary: bool, speed: f32) -> Self {
        Hole {
            shape,
            cx,
            cy,
            age: 0.0,
            life,
            speed,
            out: shape.out_time(speed),
            applied: Phase::NONE,
            primary,
        }
    }

    pub fn phase(&self) -> Phase {
        self.shape.phase(self.age, self.life, self.speed)
    }

    fn finished(&self) -> bool {
        self.age >= self.life + self.out && self.applied.gone()
    }
}

pub struct Obstacle {
    pub shape: Shape,
    pub cx: f32,
    pub cy: f32,
    pub radius: f32,
    pub ex: f32,
    pub ey: f32,
    pub id: u8,
    pub age: f32,
    pub life: f32,
    /// Fall speed (pixels per second) it is swept in and out at.
    speed: f32,
    out: f32,
    applied: Phase,
    pub holes: Vec<Hole>,
    /// Second half of a funnel; not counted separately.
    pub part: bool,
    /// Its outflow has been noted as vanishing.
    pub retired: bool,
}

impl Obstacle {
    pub fn new(shape: Shape, cx: f32, cy: f32, id: u8, life: f32, speed: f32) -> Self {
        let (ex, ey) = shape.extent();
        Obstacle {
            shape,
            cx,
            cy,
            radius: shape.radius(),
            ex,
            ey,
            id,
            age: 0.0,
            life,
            speed,
            out: shape.out_time(speed),
            applied: Phase::NONE,
            holes: Vec::new(),
            part: false,
            retired: false,
        }
    }

    pub fn phase(&self) -> Phase {
        self.shape.phase(self.age, self.life, self.speed)
    }

    pub fn finished(&self) -> bool {
        self.age >= self.life + self.out && self.applied.gone()
    }

    /// Present and not yet retiring (still growing counts: it will be there).
    pub fn is_live(&self) -> bool {
        self.age < self.life
    }

    /// Start leaving now, the way it would at the end of its life.
    pub fn retire(&mut self) {
        self.life = self.life.min(self.age);
    }

    pub fn bounds(&self, w: usize, h: usize) -> Option<Region> {
        region(self.cx, self.cy, self.ex, self.ey, w, h)
    }

    /// Solid at full size, with the holes as they are now.
    pub fn solid(&self, x: f32, y: f32) -> bool {
        self.shape.contains(x - self.cx, y - self.cy, 1.0)
            && !self
                .holes
                .iter()
                .any(|h| h.shape.covers(x - h.cx, y - h.cy, h.phase()))
    }

    /// Height of the first solid point in column `x` at or below `y0`.
    pub fn first_solid(&self, x: f32, y0: f32) -> Option<f32> {
        if (x - self.cx).abs() > self.ex {
            return None;
        }
        let bottom = self.cy + self.ey;
        let mut y = y0.max(self.cy - self.ey);
        while y <= bottom {
            if self.solid(x, y) {
                return Some(y);
            }
            y += SCAN_STEP;
        }
        None
    }

    /// Height of the lowest solid point in column `x`.
    pub fn last_solid(&self, x: f32) -> Option<f32> {
        if (x - self.cx).abs() > self.ex {
            return None;
        }
        let top = self.cy - self.ey;
        let mut y = self.cy + self.ey;
        while y >= top {
            if self.solid(x, y) {
                return Some(y);
            }
            y -= SCAN_STEP;
        }
        None
    }
}

/// Paint the obstacle in phase `ph` inside `reg`. Cells owned by the obstacle
/// that it no longer covers are cleared; sand in the way is overwritten.
pub fn paint(cells: &mut [u8], w: usize, o: &Obstacle, ph: Phase, reg: Region) {
    let (x0, x1, y0, y1) = reg;
    let holes: Vec<(&Hole, Phase)> = o.holes.iter().map(|h| (h, h.phase())).collect();
    cells[y0 * w..y1 * w].par_chunks_mut(w).enumerate().for_each(|(i, row)| {
        let dy = (y0 + i) as f32 + 0.5 - o.cy;
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - o.cx;
            let cell = &mut row[x];
            let inside = o.shape.covers(dx, dy, ph)
                && !holes
                    .iter()
                    .any(|(h, hp)| h.shape.covers(x as f32 + 0.5 - h.cx, (y0 + i) as f32 + 0.5 - h.cy, *hp));
            if inside {
                if *cell < OBSTACLE_BASE {
                    *cell = o.id;
                }
            } else if *cell == o.id {
                *cell = 0;
            }
        }
    });
}

fn union(a: Option<Region>, b: Region) -> Region {
    match a {
        None => b,
        Some(a) => (a.0.min(b.0), a.1.max(b.1), a.2.min(b.2), a.3.max(b.3)),
    }
}

/// Advance an obstacle by `dt` and repaint whatever changed.
pub fn update(cells: &mut [u8], w: usize, h: usize, o: &mut Obstacle, dt: f32) {
    o.age += dt;
    for hole in &mut o.holes {
        hole.age += dt;
    }
    let ph = o.phase();
    let mut dirty: Option<Region> = None;
    if ph != o.applied {
        if let Some(r) = o.shape.change_region(o.cx, o.cy, o.applied, ph, w, h) {
            dirty = Some(union(dirty, r));
        }
    }
    for hole in &o.holes {
        let hp = hole.phase();
        if hp != hole.applied {
            if let Some(r) = hole.shape.change_region(hole.cx, hole.cy, hole.applied, hp, w, h) {
                dirty = Some(union(dirty, r));
            }
        }
    }
    if let (Some(d), Some(b)) = (dirty, o.bounds(w, h)) {
        let r = (d.0.max(b.0), d.1.min(b.1), d.2.max(b.2), d.3.min(b.3));
        if r.0 < r.1 && r.2 < r.3 {
            paint(cells, w, o, ph, r);
        }
    }
    o.applied = ph;
    for hole in &mut o.holes {
        hole.applied = hole.phase();
    }
    o.holes.retain(|hole| !hole.finished());
}

