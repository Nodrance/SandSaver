use super::*;

use std::f32::consts::FRAC_PI_2;

/// Most streams followed (emitters plus the streams falling off obstacles).
const MAX_FLOWS: usize = 48;
/// Most obstacles a stream is followed down through.
const MAX_DEPTH: u8 = 5;

/// A stream of sand, real (from the top) or predicted (falling off an obstacle).
#[derive(Clone)]
pub(super) struct Flow {
    pub x: f32,
    /// Where it starts falling.
    pub y: f32,
    pub width: f32,
    /// Grains per second, roughly (density times width).
    pub weight: f32,
    /// Direction it was travelling when it left an obstacle (-1, 1), or 0 from the top.
    pub dir: f32,
    /// Obstacle it falls off.
    pub src: Option<u8>,
    pub depth: u8,
    /// First obstacle it lands on, and the height it does so at.
    pub hit: Option<(u8, f32)>,
}

/// A place a stream recently stopped existing.
pub(super) struct Vanished {
    pub x: f32,
    pub y: f32,
    pub ttl: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Pick {
    Slope,
    Diamond,
    Circle,
    Funnel,
    LongHole,
    CircleHole,
}

struct Piece {
    shape: Shape,
    cx: f32,
    cy: f32,
}

/// An obstacle to be added: one piece, or two for a funnel.
struct Cand {
    pieces: Vec<Piece>,
    /// Where a stream is expected to land: piece index and position.
    landings: Vec<(usize, f32, f32)>,
}

/// Where sand landing on `o` at (x, y) leaves it again: edge x, edge y,
/// direction and share of the sand.
fn outflow(o: &Obstacle, x: f32, y: f32, k: f32) -> Vec<(f32, f32, f32, f32)> {
    let step = (4.0 * k).clamp(2.0, 8.0);
    let surf = |x: f32, y: f32| o.first_solid(x, y - 6.0 * step);
    let Some(here) = surf(x, y) else {
        return Vec::new();
    };
    let level = |v: Option<f32>| v.map_or(true, |v| v >= here - 0.5 * step);
    let ok = [level(surf(x - 2.0 * step, here)), level(surf(x + 2.0 * step, here))];
    let n = ok.iter().filter(|&&b| b).count();
    let mut out = Vec::new();
    for (d, go) in [(-1.0f32, ok[0]), (1.0, ok[1])] {
        if !go {
            continue;
        }
        let (mut px, mut py) = (x, here);
        for _ in 0..256 {
            let nx = px + d * step;
            match surf(nx, py) {
                // Past the end, or a sheer drop.
                None => {
                    out.push((px, py, d, 1.0 / n as f32));
                    break;
                }
                Some(ny) if ny > py + 5.0 * step => {
                    out.push((px, py, d, 1.0 / n as f32));
                    break;
                }
                // Uphill: the sand pools instead.
                Some(ny) if ny < py - 0.75 * step => break,
                Some(ny) => {
                    px = nx;
                    py = ny;
                }
            }
        }
    }
    out
}

/// Width of stream `i` together with the streams touching it, relative to
/// `WIDE_STREAM`.
fn wideness(flows: &[Flow], i: usize, k: f32) -> f32 {
    let f = &flows[i];
    let (mut lo, mut hi) = (f.x - f.width / 2.0, f.x + f.width / 2.0);
    for (j, g) in flows.iter().enumerate() {
        if j != i && (g.y - f.y).abs() < 120.0 * k && (g.x - f.x).abs() <= (f.width + g.width) / 2.0 + 25.0 * k {
            lo = lo.min(g.x - g.width / 2.0);
            hi = hi.max(g.x + g.width / 2.0);
        }
    }
    (hi - lo) / (WIDE_STREAM * k)
}

/// How many other streams are close to stream `i`.
fn crowd(flows: &[Flow], i: usize, k: f32) -> usize {
    flows
        .iter()
        .enumerate()
        .filter(|&(j, g)| j != i && (g.x - flows[i].x).abs() < 350.0 * k)
        .count()
}

impl World {
    /// Predict where sand flows: from each emitter down to the first obstacle,
    /// along it to its edge, and down again.
    pub(super) fn trace_flows(&self) -> Vec<Flow> {
        let k = self.unit;
        let live: Vec<&Obstacle> = self.obstacles.iter().filter(|o| o.is_live()).collect();
        let mut flows: Vec<Flow> = self
            .emitters
            .iter()
            .filter(|e| e.age < e.life - EMITTER_FADE)
            .map(|e| Flow {
                x: e.x,
                y: 0.0,
                width: e.half_w * 2.0,
                weight: e.density * e.half_w * 2.0,
                dir: 0.0,
                src: None,
                depth: 0,
                hit: None,
            })
            .collect();
        let mut i = 0;
        while i < flows.len() {
            let f = flows[i].clone();
            let mut best: Option<(&Obstacle, f32, f32)> = None;
            for o in &live {
                if f.src == Some(o.id) {
                    continue;
                }
                for dx in [0.0, -0.4, 0.4] {
                    let x = f.x + dx * f.width;
                    if let Some(y) = o.first_solid(x, f.y + 2.0) {
                        if best.map_or(true, |b| y < b.2) {
                            best = Some((o, x, y));
                        }
                    }
                }
            }
            if let Some((o, hx, hy)) = best {
                flows[i].hit = Some((o.id, hy));
                if f.depth < MAX_DEPTH {
                    for (ex, ey, d, frac) in outflow(o, hx, hy, k) {
                        if flows.len() >= MAX_FLOWS {
                            break;
                        }
                        let weight = f.weight * frac;
                        let width = (weight / 0.7).clamp(6.0 * k, 160.0 * k);
                        flows.push(Flow {
                            x: ex + d * width * 0.5,
                            y: ey,
                            width,
                            weight,
                            dir: d,
                            src: Some(o.id),
                            depth: f.depth + 1,
                            hit: None,
                        });
                    }
                }
            }
            i += 1;
        }
        flows
    }

    fn is_loop(&self, x: f32, y: f32) -> bool {
        let k = self.unit;
        self.vanished
            .iter()
            .any(|v| (v.x - x).abs() < 50.0 * k && (v.y - y).abs() < 160.0 * k)
    }

    /// Whether a rule applies this time, given how strongly it is wanted.
    fn guided(&mut self, influence: f32) -> bool {
        self.rng.chance(influence * (1.0 - WEIRD_PLACEMENT))
    }

    fn weighted(&mut self, w: &[f32]) -> Option<usize> {
        let total: f32 = w.iter().sum();
        if total <= 0.0 {
            return None;
        }
        let mut r = self.rng.f32() * total;
        for (i, &x) in w.iter().enumerate() {
            if r < x {
                return Some(i);
            }
            r -= x;
        }
        w.iter().rposition(|&x| x > 0.0)
    }

    /// A stream worth building under, favouring heavy ones that land on nothing.
    fn pick_flow(&mut self, flows: &[Flow], bias: impl Fn(usize, &Flow) -> f32) -> Option<usize> {
        let (w, h) = (self.w as f32, self.h as f32);
        let weights: Vec<f32> = flows
            .iter()
            .enumerate()
            .map(|(i, f)| {
                if f.y > h * 0.8 || f.x < 0.0 || f.x > w {
                    return 0.0;
                }
                let free = if f.hit.is_none() { 1.0 } else { 0.25 };
                free * f.weight.max(0.5).sqrt() * bias(i, f)
            })
            .collect();
        self.weighted(&weights)
    }

    // -- emitters -----------------------------------------------------------

    /// Where a new stream starts: often above an obstacle, never right where
    /// one just vanished.
    pub(super) fn emitter_x(&mut self) -> f32 {
        let w = self.w as f32;
        let margin = w * 0.02;
        let mut x = self.rng.range(margin, w - margin);
        for _ in 0..8 {
            x = self.rng.range(margin, w - margin);
            if self.guided(OBSTACLES_ATTRACT_STREAMS) {
                if let Some(v) = self.sky_above_obstacle() {
                    x = v.clamp(margin, w - margin);
                }
            }
            if !self.is_loop(x, 0.0) || self.rng.chance(WEIRD_LOOPS) {
                break;
            }
        }
        x
    }

    /// An x above the high end of an obstacle with nothing over it.
    fn sky_above_obstacle(&mut self) -> Option<f32> {
        let live: Vec<usize> = (0..self.obstacles.len()).filter(|&i| self.obstacles[i].is_live()).collect();
        if live.is_empty() {
            return None;
        }
        let mut best: Option<(f32, f32)> = None;
        for _ in 0..8 {
            let idx = live[self.rng.below(live.len() as u32) as usize];
            let r = self.rng.range(-1.0, 1.0);
            let o = &self.obstacles[idx];
            let x = o.cx + r * o.ex;
            let Some(top) = o.first_solid(x, -1.0e9) else {
                continue;
            };
            let clear = self
                .obstacles
                .iter()
                .all(|p| p.id == o.id || !p.is_live() || p.first_solid(x, -1.0e9).map_or(true, |t| t >= top));
            if clear && best.map_or(true, |b| top < b.0) {
                best = Some((top, x));
            }
        }
        best.map(|b| b.1)
    }

    // -- obstacles ----------------------------------------------------------

    pub(super) fn spawn_obstacle(&mut self) {
        if self.free_ids.len() < 2 {
            return;
        }
        self.flows = self.trace_flows();
        let flows = self.flows.clone();
        let pick = self.pick_kind(&flows);
        if matches!(pick, Pick::LongHole | Pick::CircleHole) && self.spawn_cut(pick, &flows) {
            return;
        }
        let pick = if matches!(pick, Pick::LongHole | Pick::CircleHole) {
            Pick::Slope
        } else {
            pick
        };
        self.spawn_solid(pick, &flows);
    }

    fn pick_kind(&mut self, flows: &[Flow]) -> Pick {
        let (k, h) = (self.unit, self.h as f32);
        let att = STREAMS_ATTRACT_OBSTACLES * (1.0 - WEIRD_KIND);
        let usable: Vec<usize> = (0..flows.len()).filter(|&i| flows[i].y < h * 0.8).collect();
        let b = |on: bool| if on { 1.0 } else { 0.0 };
        let free = usable.iter().any(|&i| flows[i].hit.is_none());
        let hit = usable.iter().any(|&i| flows[i].hit.is_some());
        let wide = usable.iter().any(|&i| wideness(flows, i, k) > 1.0);
        let crowded = usable.iter().any(|&i| crowd(flows, i, k) >= 1);
        let cuttable = self.obstacles.iter().any(|o| o.is_live() && o.holes.len() < 3);
        let thick = self
            .obstacles
            .iter()
            .any(|o| o.is_live() && o.holes.len() < 3 && o.shape.thickness() >= 14.0 * k);
        let mut weights = [
            SLOPE_WEIGHT * (1.0 + 2.0 * att * b(free)),
            DIAMOND_WEIGHT * (1.0 + 2.5 * att * b(wide)),
            CIRCLE_WEIGHT * (1.0 + 2.5 * att * b(wide)),
            FUNNEL_WEIGHT * (1.0 + 3.0 * att * b(crowded)),
            if cuttable { LONG_HOLE_WEIGHT * (1.0 + 1.5 * att * b(hit)) } else { 0.0 },
            if thick { CIRCLE_HOLE_WEIGHT * (1.0 + 1.5 * att * b(hit)) } else { 0.0 },
        ];
        if self.rng.chance(WEIRD_KIND * 0.5) {
            // Ignore the situation altogether.
            weights = [
                SLOPE_WEIGHT,
                DIAMOND_WEIGHT,
                CIRCLE_WEIGHT,
                FUNNEL_WEIGHT,
                if cuttable { LONG_HOLE_WEIGHT } else { 0.0 },
                if thick { CIRCLE_HOLE_WEIGHT } else { 0.0 },
            ];
        }
        let kinds = [
            Pick::Slope,
            Pick::Diamond,
            Pick::Circle,
            Pick::Funnel,
            Pick::LongHole,
            Pick::CircleHole,
        ];
        self.weighted(&weights).map_or(Pick::Slope, |i| kinds[i])
    }

    fn spawn_solid(&mut self, pick: Pick, flows: &[Flow]) {
        let loops_ok = self.rng.chance(WEIRD_LOOPS);
        let overlap_ok = self.rng.chance(WEIRD_OVERLAP);
        let chaining = self.rng.chance(OBSTACLE_CHAINING);
        let mut best: Option<(f32, Cand)> = None;
        let mut valid = 0;
        for _ in 0..40 {
            let Some(c) = self.candidate(pick, flows) else {
                continue;
            };
            if !c.pieces.iter().all(|p| self.fits(p)) {
                continue;
            }
            if !overlap_ok && c.pieces.iter().any(|p| self.overlaps(p)) {
                continue;
            }
            let (chain, looped) = self.evaluate(&c);
            if looped && !loops_ok {
                continue;
            }
            valid += 1;
            let better = best.as_ref().map_or(true, |b| chain > b.0);
            if better {
                best = Some((chain, c));
            }
            if !chaining || valid >= 8 {
                break;
            }
        }
        if let Some((_, c)) = best {
            let life = vary(&mut self.rng, OBSTACLE_DURATION, OBSTACLE_DURATION_VARIANCE).max(1.0);
            for (i, p) in c.pieces.into_iter().enumerate() {
                let id = self.free_ids.pop().unwrap();
                let mut o = Obstacle::new(p.shape, p.cx, p.cy, id, life);
                o.part = i > 0;
                self.obstacles.push(o);
            }
        }
    }

    fn fits(&self, p: &Piece) -> bool {
        let (ex, ey) = p.shape.extent();
        let (w, h) = (self.w as f32, self.h as f32);
        p.cx - ex > -0.3 * ex && p.cx + ex < w + 0.3 * ex && p.cy - ey > h * 0.03 && p.cy + ey < h * 0.97
    }

    fn overlaps(&self, p: &Piece) -> bool {
        let (ex, ey) = p.shape.extent();
        let step = (5.0 * self.unit).max(3.0);
        for o in &self.obstacles {
            if o.scale() < 0.05 || (o.cx - p.cx).abs() > o.ex + ex || (o.cy - p.cy).abs() > o.ey + ey {
                continue;
            }
            let (x0, x1) = ((p.cx - ex).max(o.cx - o.ex), (p.cx + ex).min(o.cx + o.ex));
            let (y0, y1) = ((p.cy - ey).max(o.cy - o.ey), (p.cy + ey).min(o.cy + o.ey));
            let mut y = y0;
            while y <= y1 {
                let mut x = x0;
                while x <= x1 {
                    if p.shape.contains(x - p.cx, y - p.cy, 1.1) && o.shape.contains(x - o.cx, y - o.cy, 1.1) {
                        return true;
                    }
                    x += step;
                }
                y += step;
            }
        }
        false
    }

    /// How well a candidate chains (the share of the streams it creates that
    /// land on another obstacle) and whether it recreates a vanished stream.
    fn evaluate(&self, c: &Cand) -> (f32, bool) {
        let k = self.unit;
        let (mut chain, mut total, mut looped) = (0.0, 0.0, false);
        for &(i, lx, ly) in &c.landings {
            let p = &c.pieces[i];
            let temp = Obstacle::new(p.shape, p.cx, p.cy, 0, 1.0);
            for (ex, ey, d, _) in outflow(&temp, lx, ly, k) {
                total += 1.0;
                looped |= self.is_loop(ex, ey);
                let x = ex + d * 8.0 * k;
                if self
                    .obstacles
                    .iter()
                    .any(|o| o.is_live() && o.first_solid(x, ey + 4.0).is_some())
                {
                    chain += 1.0;
                }
            }
        }
        (if total > 0.0 { chain / total } else { 0.0 }, looped)
    }

    fn candidate(&mut self, pick: Pick, flows: &[Flow]) -> Option<Cand> {
        match pick {
            Pick::Slope => self.slope_candidate(flows),
            Pick::Diamond => self.diamond_candidate(flows),
            Pick::Circle => self.circle_candidate(flows),
            _ => self.funnel_candidate(flows),
        }
    }

    /// A stream to build under, and the height to build at.
    fn target_flow(&mut self, flows: &[Flow]) -> Option<(usize, f32)> {
        if !self.guided(STREAMS_ATTRACT_OBSTACLES) {
            return None;
        }
        let i = self.pick_flow(flows, |_, _| 1.0)?;
        let k = self.unit;
        let f = &flows[i];
        let mut gap = self.rng.range(60.0, 220.0) * k;
        if let Some((_, hy)) = f.hit {
            gap = gap.min((hy - f.y) * 0.7);
            if gap < 40.0 * k {
                return None;
            }
        }
        Some((i, f.y + gap))
    }

    fn slope_candidate(&mut self, flows: &[Flow]) -> Option<Cand> {
        let (k, w, h) = (self.unit, self.w as f32, self.h as f32);
        let hl = self.rng.range(110.0, 260.0) * k;
        let ht = self.rng.range(5.0, 9.0) * k;
        let (lo, hi) = if self.rng.chance(WEIRD_SHAPE) { (8.0, 80.0) } else { (31.0, 59.0) };
        let mag = self.rng.range(lo, hi).to_radians();
        let (cos, sin) = (mag.cos(), mag.sin());
        // `dd` is the way the slope goes down: 1 is to the right.
        let (cx, cy, dd, land) = if let Some((i, y_land)) = self.target_flow(flows) {
            let f = &flows[i];
            // Redirect the stream backwards compared to how it arrived.
            let mut dd = if f.dir != 0.0 { -f.dir } else { self.rng.sign() };
            if self.rng.chance(WEIRD_REDIRECT) {
                dd = -dd;
            }
            let (x_land, frac) = if wideness(flows, i, k) > 1.0 && self.rng.chance(0.5) {
                // Sit at the edge of a wide stream and split it.
                dd = self.rng.sign();
                (f.x + dd * f.width * 0.3, self.rng.range(0.0, 0.1))
            } else {
                (f.x + self.rng.range(-0.25, 0.25) * f.width, self.rng.range(0.0, 0.3))
            };
            let cx = x_land + dd * hl * cos * (1.0 - 2.0 * frac);
            let cy = y_land + hl * sin * (1.0 - 2.0 * frac) + ht / cos;
            (cx, cy, dd, (x_land, y_land))
        } else {
            let dd = self.rng.sign();
            let cx = self.rng.range(0.0, w);
            let cy = self.rng.range(0.1 * h, 0.9 * h);
            (cx, cy, dd, (cx - dd * hl * cos * 0.4, cy - hl * sin * 0.4 - ht / cos))
        };
        let shape = Shape::new(Kind::Rect { hl, ht }, dd * mag);
        Some(Cand {
            pieces: vec![Piece { shape, cx, cy }],
            landings: vec![(0, land.0, land.1)],
        })
    }

    fn diamond_candidate(&mut self, flows: &[Flow]) -> Option<Cand> {
        let (k, w, h) = (self.unit, self.w as f32, self.h as f32);
        let q = self.rng.range(30.0, 75.0) * k;
        let (lo, hi) = if self.rng.chance(WEIRD_SHAPE) { (2.0, 88.0) } else { (31.0, 59.0) };
        let theta = self.rng.sign() * self.rng.range(lo, hi).to_radians();
        let shape = Shape::new(Kind::Rect { hl: q, ht: q }, theta);
        let (_, ey) = shape.extent();
        let (c, s) = (theta.cos(), theta.sin());
        // Horizontal offset of the top corner from the centre.
        let corner = q * (c.signum() * s - s.signum() * c);
        let (x_land, y_land) = if let Some((i, y_land)) = self.target_flow(flows) {
            let f = &flows[i];
            let jitter = if wideness(flows, i, k) > 1.0 {
                self.rng.range(-0.1, 0.1) * f.width
            } else {
                self.rng.range(-0.8, 0.8) * q
            };
            (f.x + jitter, y_land)
        } else {
            (self.rng.range(0.0, w), self.rng.range(0.12 * h, 0.85 * h))
        };
        Some(Cand {
            pieces: vec![Piece {
                shape,
                cx: x_land - corner,
                cy: y_land + ey,
            }],
            landings: vec![(0, x_land, y_land)],
        })
    }

    fn circle_candidate(&mut self, flows: &[Flow]) -> Option<Cand> {
        let (k, w, h) = (self.unit, self.w as f32, self.h as f32);
        let r = self.rng.range(35.0, 95.0) * k;
        let (x_land, y_land) = if let Some((i, y_land)) = self.target_flow(flows) {
            let f = &flows[i];
            let jitter = if wideness(flows, i, k) > 1.0 {
                self.rng.range(-0.1, 0.1) * f.width
            } else {
                self.rng.range(-0.6, 0.6) * r
            };
            (f.x + jitter, y_land)
        } else {
            (self.rng.range(0.0, w), self.rng.range(0.12 * h, 0.85 * h))
        };
        Some(Cand {
            pieces: vec![Piece {
                shape: Shape::new(Kind::Circle { r }, 0.0),
                cx: x_land,
                cy: y_land + r,
            }],
            landings: vec![(0, x_land, y_land)],
        })
    }

    fn funnel_candidate(&mut self, flows: &[Flow]) -> Option<Cand> {
        let (k, w, h) = (self.unit, self.w as f32, self.h as f32);
        let hl = self.rng.range(55.0, 120.0) * k;
        let gap = if self.rng.chance(0.15) {
            self.rng.range(80.0, 160.0)
        } else {
            self.rng.range(14.0, 70.0)
        } * k;
        let weird = self.rng.chance(WEIRD_SHAPE);
        let a_l = self.funnel_angle(weird);
        let a_r = if weird && self.rng.chance(0.5) { self.funnel_angle(true) } else { a_l };
        let off = if self.rng.chance(0.7) {
            self.rng.range(-12.0, 12.0)
        } else {
            self.rng.range(-100.0, 100.0)
        } * k;

        // Gather streams that are close together.
        let picked = if self.guided(STREAMS_ATTRACT_OBSTACLES) {
            self.pick_flow(flows, |i, _| 1.0 + 2.0 * crowd(flows, i, k) as f32)
        } else {
            None
        };
        let (cx, y_rim) = if let Some(i) = picked {
            let f = &flows[i];
            let near = || flows.iter().filter(|g| (g.x - f.x).abs() < 350.0 * k && g.y < h * 0.8);
            let total: f32 = near().map(|g| g.weight).sum::<f32>().max(1e-3);
            let cx = near().map(|g| g.x * g.weight).sum::<f32>() / total;
            let mut y = near().map(|g| g.y).fold(f.y, f32::max);
            let mut drop = self.rng.range(70.0, 200.0) * k;
            if let Some((_, hy)) = f.hit {
                drop = drop.min((hy - y) * 0.7);
                if drop < 40.0 * k {
                    return None;
                }
            }
            y += drop;
            (cx, y)
        } else {
            (self.rng.range(0.1 * w, 0.9 * w), self.rng.range(0.12 * h, 0.7 * h))
        };

        let ht = self.rng.range(5.0, 9.0) * k;
        let (vt_l, vt_r) = (ht / a_l.cos(), ht / a_r.cos());
        let (m_l, m_r) = (a_l.tan(), a_r.tan());
        let cy_l = y_rim + m_l * hl + vt_l;
        let cy_r = y_rim + off + m_r * hl + vt_r;
        let cx_l = cx - gap / 2.0 - hl;
        let cx_r = cx + gap / 2.0 + hl;
        let left = Shape::new(Kind::Para { hl, vt: vt_l, slope: m_l }, 0.0);
        let right = Shape::new(Kind::Para { hl, vt: vt_r, slope: -m_r }, 0.0);
        Some(Cand {
            pieces: vec![
                Piece { shape: left, cx: cx_l, cy: cy_l },
                Piece { shape: right, cx: cx_r, cy: cy_r },
            ],
            landings: vec![
                (0, cx_l - hl * 0.5, cy_l - m_l * hl * 0.5 - vt_l),
                (1, cx_r + hl * 0.5, cy_r - m_r * hl * 0.5 - vt_r),
            ],
        })
    }

    /// A funnel wall angle in radians: usually 45 degrees, else 30 to 60.
    fn funnel_angle(&mut self, weird: bool) -> f32 {
        let deg = if weird {
            self.rng.range(12.0, 78.0)
        } else if self.rng.chance(0.7) {
            self.rng.range(42.0, 48.0)
        } else {
            self.rng.range(30.0, 60.0)
        };
        deg.to_radians()
    }

    // -- holes --------------------------------------------------------------

    /// Cut a hole into an existing obstacle. Returns false if there was
    /// nothing suitable to cut.
    fn spawn_cut(&mut self, pick: Pick, flows: &[Flow]) -> bool {
        let k = self.unit;
        let thick_only = pick == Pick::CircleHole;
        let ok: Vec<usize> = (0..self.obstacles.len())
            .filter(|&i| {
                let o = &self.obstacles[i];
                o.is_live()
                    && o.holes.len() < 3
                    && o.life - o.age > 3.0
                    && (!thick_only || o.shape.thickness() >= 14.0 * k)
            })
            .collect();
        if ok.is_empty() {
            return false;
        }

        // Under a stream if possible, so it falls through.
        let mut target = None;
        let mut x = 0.0;
        if self.guided(STREAMS_ATTRACT_OBSTACLES) {
            let ids: Vec<u8> = ok.iter().map(|&i| self.obstacles[i].id).collect();
            let jitter = self.rng.range(-0.3, 0.3);
            let own = |f: &Flow| f.hit.map_or(false, |(id, _)| ids.contains(&id));
            if let Some(i) = self.pick_flow(flows, |_, f| if own(f) { 1.0 } else { 0.0 }) {
                let f = &flows[i];
                if let Some((id, _)) = f.hit {
                    target = ok.iter().copied().find(|&j| self.obstacles[j].id == id);
                    x = f.x + jitter * f.width;
                }
            }
        }
        let t = match target {
            Some(t) => t,
            None => {
                let t = ok[self.rng.below(ok.len() as u32) as usize];
                x = self.obstacles[t].cx + self.rng.range(-0.4, 0.4) * self.obstacles[t].ex;
                t
            }
        };

        let o = &self.obstacles[t];
        let (Some(top), Some(bot)) = (o.first_solid(x, -1.0e9), o.last_solid(x)) else {
            return false;
        };
        let thickness = o.shape.thickness();
        let span = bot - top;
        let weird = WEIRD_SHAPE;

        let (shape, hx, hy) = if pick == Pick::LongHole {
            let a = if self.rng.chance(weird) {
                self.rng.range(-80.0, 80.0)
            } else {
                self.rng.range(-45.0, 45.0)
            }
            .to_radians();
            let theta = FRAC_PI_2 + a;
            let half_w = self.rng.range(6.0, 15.0) * k;
            let (lh, hx, hy) = if !self.rng.chance(0.2 + 0.6 * weird) {
                // Cut clean through.
                let need = span * 0.5 * self.rng.range(1.2, 1.6) / a.cos().max(0.5) + 10.0 * k;
                (need.max(self.rng.range(50.0, 110.0) * k), x, (top + bot) * 0.5)
            } else {
                // Only a crater.
                let lh = (span * self.rng.range(0.25, 0.7)).max(20.0 * k);
                (lh, x + theta.cos() * lh * 0.6, top + theta.sin() * lh * 0.6)
            };
            (Shape::new(Kind::Rect { hl: lh, ht: half_w }, theta), hx, hy)
        } else if self.rng.chance(0.12 + 0.5 * weird) {
            // A cup that goes all the way through.
            let r = (span * 0.5 * self.rng.range(1.05, 1.3)).max(10.0 * k);
            (Shape::new(Kind::Circle { r }, 0.0), x, (top + bot) * 0.5)
        } else {
            let r = (thickness * self.rng.range(0.3, 0.55)).clamp(10.0 * k, 70.0 * k);
            (Shape::new(Kind::Circle { r }, 0.0), x, top + r * self.rng.range(-0.15, 0.35))
        };

        let life = vary(&mut self.rng, HOLE_DURATION, HOLE_DURATION_VARIANCE);
        let reach = shape.radius();
        for (j, o) in self.obstacles.iter_mut().enumerate() {
            let primary = j == t;
            if !primary
                && (!o.is_live()
                    || o.holes.len() >= 4
                    || (o.cx - hx).hypot(o.cy - hy) > o.radius + reach)
            {
                continue;
            }
            let life = life.min(o.life - o.age - SHRINK_TIME);
            if life >= 1.5 {
                o.holes.push(Hole::new(shape, hx, hy, life, primary));
            }
        }
        true
    }
}
