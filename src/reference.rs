//! Test-only helpers: a plain serial in-place falling-sand step, which is the
//! physics the real simulation aims to look like, plus a small scene builder and
//! PNG writer for eyeballing results (`cargo test --release -- --nocapture`).

use crate::rng::Rng;
use crate::sim::OBSTACLE_BASE;

/// Serial bottom-up in-place step: each grain falls if it can, otherwise slides
/// to a free lower diagonal. Sand reaching the bottom row is removed.
pub fn step_serial(cells: &mut [u8], w: usize, h: usize, frame: usize, rng: &mut Rng) {
    for c in &mut cells[(h - 1) * w..] {
        if *c < OBSTACLE_BASE {
            *c = 0;
        }
    }
    for y in (0..h - 1).rev() {
        let rev = (frame + y) & 1 == 1;
        for k in 0..w {
            let x = if rev { w - 1 - k } else { k };
            let i = y * w + x;
            let c = cells[i];
            if c == 0 || c >= OBSTACLE_BASE {
                continue;
            }
            let below = i + w;
            if cells[below] == 0 {
                cells[below] = c;
                cells[i] = 0;
                continue;
            }
            let l = x > 0 && cells[below - 1] == 0;
            let r = x + 1 < w && cells[below + 1] == 0;
            let t = match (l, r) {
                (true, true) => {
                    if rng.next_u32() & 1 == 0 {
                        below - 1
                    } else {
                        below + 1
                    }
                }
                (true, false) => below - 1,
                (false, true) => below + 1,
                _ => continue,
            };
            cells[t] = c;
            cells[i] = 0;
        }
    }
}

pub const W: usize = 640;
pub const H: usize = 360;

fn line(cells: &mut [u8], x0: f32, y0: f32, x1: f32, y1: f32, thick: f32, id: u8) {
    let n = ((x1 - x0).abs().max((y1 - y0).abs()) * 2.0) as usize + 1;
    for s in 0..=n {
        let t = s as f32 / n as f32;
        let (cx, cy) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
        let r = thick as i32;
        for dy in -r..=r {
            for dx in -r..=r {
                let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                if x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < H {
                    cells[y as usize * W + x as usize] = id;
                }
            }
        }
    }
}

/// A few ramps, a box and a disc under three emitters.
pub fn scene() -> Vec<u8> {
    let mut c = vec![0u8; W * H];
    line(&mut c, 40.0, 90.0, 200.0, 170.0, 2.0, 200);
    line(&mut c, 600.0, 110.0, 440.0, 200.0, 2.0, 201);
    line(&mut c, 260.0, 230.0, 400.0, 250.0, 2.0, 202);
    for y in 150..180 {
        for x in 300..340 {
            c[y * W + x] = 203;
        }
    }
    for y in 0..H {
        for x in 0..W {
            let (dx, dy) = (x as f32 - 120.0, y as f32 - 290.0);
            if dx * dx + dy * dy < 40.0 * 40.0 {
                c[y * W + x] = 204;
            }
        }
    }
    c
}

/// Emit sand on three columns' worth of the top row.
pub fn emit(cells: &mut [u8], rng: &mut Rng, step: usize) {
    for (x0, x1) in [(100usize, 130usize), (300, 330), (500, 520)] {
        for x in x0..x1 {
            if cells[x] == 0 && rng.f32() < 0.25 {
                cells[x] = 1 + ((step / 8 + x / 8) % 100) as u8;
            }
        }
    }
}

pub fn render(cells: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(W * H * 3);
    for &c in cells {
        let px: [u8; 3] = if c == 0 {
            [10, 11, 18]
        } else if c >= OBSTACLE_BASE {
            [90, 100, 130]
        } else {
            let hue = c as f32 * 3.0;
            let (r, g, b) = match ((hue / 60.0) as u32) % 6 {
                0 => (1.0, (hue % 60.0) / 60.0, 0.0),
                1 => (1.0 - (hue % 60.0) / 60.0, 1.0, 0.0),
                2 => (0.0, 1.0, (hue % 60.0) / 60.0),
                3 => (0.0, 1.0 - (hue % 60.0) / 60.0, 1.0),
                4 => ((hue % 60.0) / 60.0, 0.0, 1.0),
                _ => (1.0, 0.0, 1.0 - (hue % 60.0) / 60.0),
            };
            [(r * 230.0) as u8 + 20, (g * 230.0) as u8 + 20, (b * 230.0) as u8 + 20]
        };
        rgb.extend_from_slice(&px);
    }
    rgb
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (!(crc & 1)).wrapping_add(1));
        }
    }
    !crc
}

/// Minimal uncompressed PNG writer.
pub fn write_png(path: &str, w: usize, h: usize, rgb: &[u8]) {
    let mut raw = Vec::with_capacity((w * 3 + 1) * h);
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgb[y * w * 3..(y + 1) * w * 3]);
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(ch) = chunks.next() {
        z.push(chunks.peek().is_none() as u8);
        z.extend_from_slice(&(ch.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(ch.len() as u16)).to_le_bytes());
        z.extend_from_slice(ch);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &v in &raw {
        a = (a + v as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut put = |tag: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = tag.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    put(b"IHDR", &ihdr);
    put(b"IDAT", &z);
    put(b"IEND", &[]);
    std::fs::write(path, out).unwrap();
}

#[test]
fn reference_picture() {
    let mut rng = Rng::seeded(1);
    let mut cells = scene();
    for f in 0..2500 {
        emit(&mut cells, &mut rng, f);
        step_serial(&mut cells, W, H, f, &mut rng);
    }
    write_png("target/ref.png", W, H, &render(&cells));
}
