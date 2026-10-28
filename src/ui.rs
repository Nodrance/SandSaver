//! Draws the menu into a premultiplied RGBA image with a built-in 5x7 font.

use crate::menu::{Body, View};

pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Width of the panel, in characters.
const COLS: usize = 44;

type Color = [u8; 4];

/// Premultiplied color from straight RGB and alpha.
const fn pm(r: u32, g: u32, b: u32, a: u32) -> Color {
    [(r * a / 255) as u8, (g * a / 255) as u8, (b * a / 255) as u8, a as u8]
}

const BG: Color = pm(16, 19, 32, 224);
const BORDER: Color = pm(86, 104, 160, 255);
const TEXT: Color = pm(214, 219, 234, 255);
const DIM: Color = pm(132, 142, 168, 255);
const ACCENT: Color = pm(255, 202, 96, 255);
const GOOD: Color = pm(150, 214, 160, 255);
const SELECTED: Color = pm(52, 66, 118, 255);
const WHITE: Color = pm(255, 255, 255, 255);
const TRACK: Color = pm(40, 46, 72, 255);

/// Rows of a glyph, top to bottom; bit 4 is the leftmost pixel.
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        ',' => [0, 0, 0, 0, 0b01100, 0b00100, 0b01000],
        ':' => [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '%' => [0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011],
        '>' => [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000],
        '<' => [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010],
        '=' => [0, 0, 0b11111, 0, 0b11111, 0, 0],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        '\'' => [0b00100, 0b00100, 0b01000, 0, 0, 0, 0],
        '*' => [0, 0b10101, 0b01110, 0b11111, 0b01110, 0b10101, 0],
        '#' => [0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010],
        _ => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111],
    }
}

struct Canvas {
    w: usize,
    h: usize,
    buf: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Canvas { w, h, buf: vec![0; w * h * 4] }
    }

    /// Overwrite a rectangle (clipped).
    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, c: Color) {
        for yy in y..(y + h).min(self.h) {
            for xx in x..(x + w).min(self.w) {
                let i = (yy * self.w + xx) * 4;
                self.buf[i..i + 4].copy_from_slice(&c);
            }
        }
    }

    /// Draw text with pixels `s` wide; returns its width.
    fn text(&mut self, x: usize, y: usize, s: usize, text: &str, c: Color) -> usize {
        let mut cx = x;
        for ch in text.chars() {
            for (r, bits) in glyph(ch).iter().enumerate() {
                for b in 0..5 {
                    if bits & (0b10000 >> b) != 0 {
                        self.fill(cx + b * s, y + r * s, s, s, c);
                    }
                }
            }
            cx += 6 * s;
        }
        cx - x
    }
}

/// Greedy word wrap.
fn wrap(text: &str, cols: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= cols => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// Draw `view` with pixels `scale` big, no taller than `max_h`.
pub fn render(view: &View, scale: u32, max_h: u32) -> Image {
    let s = scale.max(1) as usize;
    let (cw, lh, rh, pad, m) = (6 * s, 10 * s, 12 * s, 4 * s, 4 * s);
    let sep_h = (s / 2).max(1);
    let gap = 3 * s;
    let inner = COLS * cw;
    let pw = inner + 2 * pad;
    let tw = m + pw;
    let max_h = (max_h as usize).max(m + 24 * lh);
    let x0 = m + pad;

    let help = wrap(&view.help, COLS);
    let keys = wrap(&view.keys, COLS);
    let status_lines = usize::from(!view.status.is_empty());

    let mut c = Canvas::new(tw, max_h);
    c.fill(m, m, pw, max_h - m, BG);
    let mut y = m + pad;

    let crumbs: String = view.crumbs.chars().take(COLS).collect();
    c.text(x0, y, s, &crumbs, DIM);
    y += lh;
    c.text(x0, y, s, &format!("Preset: {}", view.preset), GOOD);
    y += lh;

    let sep = |c: &mut Canvas, y: &mut usize| {
        *y += gap;
        c.fill(x0, *y, inner, sep_h, BORDER);
        *y += sep_h + gap;
    };
    sep(&mut c, &mut y);

    match &view.body {
        Body::List { rows, sel } => {
            let n = rows.len();
            let fixed = m + 2 * pad + 2 * lh + 3 * (2 * gap + sep_h) + (help.len() + keys.len() + status_lines) * lh;
            let vis = (max_h.saturating_sub(fixed) / rh).clamp(3, n.max(3)).min(n);
            let start = (sel + 1).saturating_sub(vis).min(n - vis);
            let text_dy = (rh - 7 * s) / 2;
            for (k, row) in rows[start..start + vis].iter().enumerate() {
                let ry = y + k * rh;
                let selected = start + k == *sel;
                if selected {
                    c.fill(x0, ry, inner, rh, SELECTED);
                }
                let (label_c, value_c) = if selected { (WHITE, ACCENT) } else { (TEXT, ACCENT) };
                c.text(x0 + 2 * s, ry + text_dy, s, &row.label, label_c);
                let end = x0 + inner - 2 * cw;
                let vw = row.value.chars().count() * cw;
                c.text(end - vw, ry + text_dy, s, &row.value, value_c);
                if row.arrow {
                    c.text(x0 + inner - cw - s, ry + text_dy, s, ">", if selected { WHITE } else { DIM });
                }
            }
            if n > vis {
                let track = vis * rh;
                let thumb = (track * vis / n).max(2 * s);
                let top = (track - thumb) * start / (n - vis);
                c.fill(m + pw - 2 * s, y, s, track, TRACK);
                c.fill(m + pw - 2 * s, y + top, s, thumb, DIM);
            }
            y += vis * rh;
        }
        Body::Adjust { name, value, frac, min, max } => {
            c.text(x0, y, s, name, TEXT);
            y += lh + 2 * s;
            let big = 2 * s;
            let vw = value.chars().count() * 6 * big;
            c.text(x0 + (inner.saturating_sub(vw)) / 2, y, big, value, ACCENT);
            y += 7 * big + 3 * s;
            let bar_h = 6 * s;
            c.fill(x0, y, inner, bar_h, TRACK);
            let filled = ((inner as f32) * frac).round() as usize;
            c.fill(x0, y, filled, bar_h, ACCENT);
            y += bar_h + 2 * s;
            c.text(x0, y, s, min, DIM);
            let mw = max.chars().count() * cw;
            c.text(x0 + inner - mw, y, s, max, DIM);
            y += lh;
        }
        Body::Naming { text } => {
            c.text(x0, y, s, "Name:", DIM);
            y += lh + s;
            c.fill(x0, y, inner, rh, SELECTED);
            let shown = format!("{text}_");
            c.text(x0 + 2 * s, y + (rh - 7 * s) / 2, s, &shown, WHITE);
            y += rh;
        }
    }

    if !help.is_empty() || status_lines > 0 {
        sep(&mut c, &mut y);
    }
    for line in &help {
        c.text(x0, y, s, line, TEXT);
        y += lh;
    }
    if status_lines > 0 {
        c.text(x0, y, s, &view.status, ACCENT);
        y += lh;
    }
    sep(&mut c, &mut y);
    for line in &keys {
        c.text(x0, y, s, line, DIM);
        y += lh;
    }
    y += pad;

    // Crop to what was used and frame it.
    let h = y.min(max_h);
    c.buf.truncate(tw * h * 4);
    c.h = h;
    let t = (s / 2).max(1);
    c.fill(m, m, pw, t, BORDER);
    c.fill(m, h - t, pw, t, BORDER);
    c.fill(m, m, t, h - m, BORDER);
    c.fill(m + pw - t, m, t, h - m, BORDER);

    Image { w: tw as u32, h: h as u32, rgba: c.buf }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::Menu;
    use crate::settings::{Settings, Store};

    #[test]
    fn wraps_words() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("", 5), Vec::<String>::new());
    }

    #[test]
    fn renders_inside_its_bounds() {
        let menu = Menu::new(Settings::default(), Store::at(std::env::temp_dir().join("sand_ui_test_unused")));
        for scale in 1..=6 {
            let img = render(&menu.view(), scale, 700);
            assert_eq!(img.rgba.len(), (img.w * img.h * 4) as usize);
            assert!(img.h <= 700.max(4 * 6 + 24 * 10 * scale));
            // Premultiplied: no channel exceeds alpha.
            assert!(img.rgba.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
            assert!(img.rgba.chunks(4).any(|p| p[3] > 0));
        }
    }
}
