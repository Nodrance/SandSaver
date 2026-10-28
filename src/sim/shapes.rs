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

    /// Whether the point (dx, dy), relative to the shape centre, lies inside
    /// the shape scaled by `s`.
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
    ex: f32,
    ey: f32,
    pub age: f32,
    pub life: f32,
    /// Scale it was last painted at.
    pub applied: f32,
    /// Counts towards the obstacle count (one copy per hole).
    pub primary: bool,
}

impl Hole {
    pub fn new(shape: Shape, cx: f32, cy: f32, life: f32, primary: bool) -> Self {
        let (ex, ey) = shape.extent();
        Hole {
            shape,
            cx,
            cy,
            ex,
            ey,
            age: 0.0,
            life,
            applied: -1.0,
            primary,
        }
    }

    pub fn scale(&self) -> f32 {
        envelope(self.age, self.life)
    }

    fn finished(&self) -> bool {
        self.age >= self.life + SHRINK_TIME && self.applied <= 0.0
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
    pub applied: f32,
    pub holes: Vec<Hole>,
    /// Second half of a funnel; not counted separately.
    pub part: bool,
    /// Its outflow has been noted as vanishing.
    pub retired: bool,
}

impl Obstacle {
    pub fn new(shape: Shape, cx: f32, cy: f32, id: u8, life: f32) -> Self {
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
            applied: -1.0,
            holes: Vec::new(),
            part: false,
            retired: false,
        }
    }

    pub fn scale(&self) -> f32 {
        envelope(self.age, self.life)
    }

    pub fn finished(&self) -> bool {
        self.age >= self.life + SHRINK_TIME && self.applied <= 0.0
    }

    /// Fully grown and not yet shrinking.
    pub fn is_live(&self) -> bool {
        self.age < self.life && self.scale() > 0.5
    }

    pub fn bounds(&self, scale: f32, w: usize, h: usize) -> Option<Region> {
        let m = scale.max(0.0);
        region(self.cx, self.cy, self.ex * m, self.ey * m, w, h)
    }

    /// Solid at full size, with the holes as they are now.
    pub fn solid(&self, x: f32, y: f32) -> bool {
        self.shape.contains(x - self.cx, y - self.cy, 1.0)
            && !self
                .holes
                .iter()
                .any(|h| h.shape.contains(x - h.cx, y - h.cy, h.scale()))
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

/// Paint the obstacle at scale `s` inside `reg`. Cells owned by the obstacle
/// that it no longer covers are cleared; sand in the way is overwritten.
pub fn paint(cells: &mut [u8], w: usize, o: &Obstacle, s: f32, reg: Region) {
    let (x0, x1, y0, y1) = reg;
    let holes: Vec<(&Hole, f32)> = o.holes.iter().map(|h| (h, h.scale())).collect();
    cells[y0 * w..y1 * w].par_chunks_mut(w).enumerate().for_each(|(i, row)| {
        let dy = (y0 + i) as f32 + 0.5 - o.cy;
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - o.cx;
            let cell = &mut row[x];
            let inside = o.shape.contains(dx, dy, s)
                && !holes
                    .iter()
                    .any(|(h, hs)| h.shape.contains(x as f32 + 0.5 - h.cx, (y0 + i) as f32 + 0.5 - h.cy, *hs));
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
    let s = o.scale();
    if s != o.applied {
        if let Some(reg) = o.bounds(s.max(o.applied), w, h) {
            paint(cells, w, o, s, reg);
        }
        o.applied = s;
        for hole in &mut o.holes {
            hole.applied = hole.scale();
        }
    } else {
        let mut dirty: Option<Region> = None;
        for hole in &o.holes {
            let hs = hole.scale();
            if hs != hole.applied {
                let m = hs.max(hole.applied).max(0.0);
                if let Some(r) = region(hole.cx, hole.cy, hole.ex * m, hole.ey * m, w, h) {
                    dirty = Some(union(dirty, r));
                }
            }
        }
        if let Some(d) = dirty {
            if let Some(b) = o.bounds(s, w, h) {
                let r = (d.0.max(b.0), d.1.min(b.1), d.2.max(b.2), d.3.min(b.3));
                if r.0 < r.1 && r.2 < r.3 {
                    paint(cells, w, o, s, r);
                }
            }
            for hole in &mut o.holes {
                hole.applied = hole.scale();
            }
        }
    }
    o.holes.retain(|hole| !hole.finished());
}
