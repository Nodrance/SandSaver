//! Tunable parameters, their description for the menu, presets and storage.
//!
//! Every `*_variance` is a fraction: a value is drawn uniformly from
//! `mean * (1 - variance) ..= mean * (1 + variance)`, so 0 means no variation
//! and 0.5 means plus or minus 50%. Sizes are in pixels of a 1080p screen and
//! scale with the resolution.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Unit {
    Int,
    /// Zero means automatic.
    Auto,
    Dec1,
    Dec2,
    Dec3,
    Percent,
    Px,
    Secs,
    Deg,
}

impl Unit {
    pub fn format(self, v: f32) -> String {
        match self {
            Unit::Int => format!("{v:.0}"),
            Unit::Auto if v < 0.5 => "auto".to_string(),
            Unit::Auto => format!("{v:.0}"),
            Unit::Dec1 => format!("{v:.1}"),
            Unit::Dec2 => format!("{v:.2}"),
            Unit::Dec3 => format!("{v:.3}"),
            Unit::Percent => format!("{:.0}%", v * 100.0),
            Unit::Px => format!("{v:.0} px"),
            Unit::Secs => format!("{v:.1} s"),
            Unit::Deg => format!("{v:.0} deg"),
        }
    }
}

/// One adjustable setting as the menu sees it.
pub struct Item {
    pub folder: &'static str,
    pub key: &'static str,
    pub name: &'static str,
    pub help: &'static str,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub unit: Unit,
    pub get: fn(&Settings) -> f32,
    pub set: fn(&mut Settings, f32),
}

impl Item {
    pub fn clamp(&self, v: f32) -> f32 {
        v.clamp(self.min, self.max)
    }

    pub fn text(&self, s: &Settings) -> String {
        self.unit.format((self.get)(s))
    }

    /// Position of the value between the minimum and maximum, 0 to 1.
    pub fn fraction(&self, s: &Settings) -> f32 {
        ((self.get)(s) - self.min) / (self.max - self.min)
    }
}

macro_rules! settings {
    ($($folder:literal {
        $($field:ident: $name:literal, $def:expr, $min:expr, $max:expr, $step:expr, $unit:ident, $help:literal;)*
    })*) => {
        #[derive(Clone, Copy, PartialEq, Debug)]
        pub struct Settings {
            $($(pub $field: f32,)*)*
        }

        impl Default for Settings {
            fn default() -> Self {
                Settings { $($($field: $def,)*)* }
            }
        }

        pub static ITEMS: &[Item] = &[
            $($(Item {
                folder: $folder,
                key: stringify!($field),
                name: $name,
                help: $help,
                min: $min,
                max: $max,
                step: $step,
                unit: Unit::$unit,
                get: |s| s.$field,
                set: |s, v| s.$field = v,
            },)*)*
        ];
    };
}

settings! {
    "Obstacles" {
        obstacle_count: "Count", 0.0, 0.0, 120.0, 1.0, Auto,
            "Obstacles on screen on average. Auto picks a number from the screen area.";
        obstacle_count_variance: "Count drift", 0.25, 0.0, 1.0, 0.05, Percent,
            "How far the wanted number of obstacles wanders over time.";
        obstacle_scale: "Scale", 1.0, 0.25, 3.0, 0.05, Percent,
            "Size of obstacles along their long direction. Circles and diamonds scale in every direction.";
        obstacle_scale_variance: "Scale variation", 0.0, 0.0, 1.0, 0.05, Percent,
            "How much scales differ from one obstacle to the next, on top of the usual variety.";
        obstacle_thickness: "Thickness", 14.0, 2.0, 100.0, 1.0, Px,
            "Thickness of slopes and funnel walls. Slots are cut one and a half times as thick.";
        obstacle_thickness_variance: "Thickness variation", 0.3, 0.0, 1.0, 0.05, Percent,
            "How much thicknesses differ from one obstacle to the next.";
        obstacle_duration: "Lifetime", 15.0, 2.0, 120.0, 1.0, Secs,
            "How long an obstacle stays fully formed.";
        obstacle_duration_variance: "Lifetime variation", 0.35, 0.0, 1.0, 0.05, Percent,
            "How much lifetimes differ from one obstacle to the next.";
        obstacle_clearance: "Spacing", 28.0, 0.0, 120.0, 2.0, Px,
            "Gap kept between a new obstacle and the others.";
        hole_duration: "Hole lifetime", 10.0, 2.0, 60.0, 1.0, Secs,
            "How long a hole cut in an obstacle stays open.";
        hole_duration_variance: "Hole variation", 0.4, 0.0, 1.0, 0.05, Percent,
            "How much hole lifetimes differ.";
    }
    "Obstacle kinds" {
        slope_weight: "Slopes", 3.0, 0.0, 10.0, 0.1, Dec1,
            "How often long sloped bars are built.";
        diamond_weight: "Diamonds", 1.2, 0.0, 10.0, 0.1, Dec1,
            "How often tilted squares are built.";
        circle_weight: "Circles", 1.4, 0.0, 10.0, 0.1, Dec1,
            "How often discs are built.";
        funnel_weight: "Funnels", 1.0, 0.0, 10.0, 0.1, Dec1,
            "How often pairs of slopes that gather sand are built.";
        long_hole_weight: "Slots", 1.2, 0.0, 10.0, 0.1, Dec1,
            "How often slots are cut through existing obstacles.";
        circle_hole_weight: "Round holes", 1.2, 0.0, 10.0, 0.1, Dec1,
            "How often round holes are cut in thick obstacles.";
    }
    "Streams" {
        stream_count: "Count", 8.0, 0.0, 32.0, 1.0, Auto,
            "Streams falling from the top on average. Auto picks a number from the screen width.";
        stream_count_variance: "Count drift", 0.5, 0.0, 1.0, 0.05, Percent,
            "How far the wanted number of streams wanders over time.";
        stream_density: "Density", 0.5, 0.02, 1.0, 0.02, Percent,
            "How thickly sand is released across the width of a stream.";
        stream_density_variance: "Density variation", 1.0, 0.0, 1.0, 0.05, Percent,
            "How much densities differ from one stream to the next.";
        stream_width: "Width", 30.0, 2.0, 3840.0, 2.0, Px,
            "Average stream width. Hold Shift to adjust faster.";
        stream_width_variance: "Width variation", 0.7, 0.0, 1.0, 0.05, Percent,
            "How much widths differ from one stream to the next.";
        stream_duration: "Duration", 3.0, 0.5, 600.0, 0.5, Secs,
            "How long a stream flows before it stops.";
        stream_duration_variance: "Duration variation", 0.8, 0.0, 1.0, 0.05, Percent,
            "How much durations differ from one stream to the next.";
    }
    "Influence" {
        streams_attract_obstacles: "Streams pull obstacles", 0.8, 0.0, 1.0, 0.05, Percent,
            "How strongly obstacles are built under streams, in a shape that suits them.";
        obstacles_attract_streams: "Obstacles pull streams", 0.6, 0.0, 1.0, 0.05, Percent,
            "How strongly new streams start above existing obstacles.";
        obstacle_chaining: "Chaining", 0.7, 0.0, 1.0, 0.05, Percent,
            "How strongly a new obstacle is placed so its runoff lands on another obstacle.";
        wide_stream: "Wide stream", 60.0, 10.0, 400.0, 5.0, Px,
            "Streams wider than this get split or blocked rather than redirected.";
    }
    "Weirdness" {
        weird_kind: "Odd kinds", 0.15, 0.0, 1.0, 0.05, Percent,
            "Chance the kind of obstacle ignores what streams suggest.";
        weird_placement: "Odd placement", 0.15, 0.0, 1.0, 0.05, Percent,
            "Chance something is placed anywhere instead of by the rules.";
        weird_shape: "Odd shapes", 0.15, 0.0, 1.0, 0.05, Percent,
            "Chance of unusual angles and proportions.";
        weird_overlap: "Overlap", 0.10, 0.0, 1.0, 0.05, Percent,
            "Chance an obstacle may overlap others.";
        weird_redirect: "Odd redirect", 0.15, 0.0, 1.0, 0.05, Percent,
            "Chance a slope sends sand forwards instead of backwards.";
        weird_loops: "Loops", 0.10, 0.0, 1.0, 0.05, Percent,
            "Chance something recreates a stream that just vanished.";
        loop_memory: "Loop memory", 8.0, 0.0, 30.0, 0.5, Secs,
            "How long a vanished stream is remembered.";
    }
    "Colors" {
        sand_lightness: "Lightness", 0.80, 0.30, 0.95, 0.01, Dec2,
            "Perceived brightness of the sand.";
        sand_chroma: "Chroma", 0.11, 0.0, 0.20, 0.005, Dec3,
            "How colorful the sand is. Very high values clip.";
        hue_start: "Start hue", 0.0, 0.0, 355.0, 1.0, Deg,
            "Where on the color wheel the palette starts.";
        hue_span: "Hue range", 360.0, 20.0, 360.0, 1.0, Deg,
            "How much of the color wheel the palette covers.";
        hue_speed: "Color speed", 15.0, 0.0, 60.0, 1.0, Int,
            "How fast streams cycle through the palette.";
    }
}

impl Settings {
    /// Set item `i`, clamped to its range.
    pub fn set_item(&mut self, i: usize, v: f32) {
        let item = &ITEMS[i];
        (item.set)(self, item.clamp(v));
    }

    /// Move item `i` by `dir` steps (times `mult`), snapped to its step grid.
    pub fn adjust(&mut self, i: usize, dir: f32, mult: f32) {
        let item = &ITEMS[i];
        let v = (item.get)(self) + dir * mult * item.step;
        let snapped = item.min + ((v - item.min) / item.step).round() * item.step;
        // Strip float noise such as 0.30000001.
        self.set_item(i, (snapped * 10_000.0).round() / 10_000.0);
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for item in ITEMS {
            let _ = writeln!(out, "{}={}", item.key, (item.get)(self));
        }
        out
    }

    /// Apply a `key=value` line. Unknown keys and bad values are ignored.
    fn apply_line(&mut self, line: &str) {
        let Some((key, value)) = line.split_once('=') else {
            return;
        };
        let Some(i) = ITEMS.iter().position(|it| it.key == key.trim()) else {
            return;
        };
        if let Some(v) = value.trim().parse::<f32>().ok().filter(|v| v.is_finite()) {
            self.set_item(i, v);
        }
    }

    pub fn from_text(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            s.apply_line(line);
        }
        s
    }
}

/// Distinct folder names, in order.
pub fn folders() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for item in ITEMS {
        if !out.contains(&item.folder) {
            out.push(item.folder);
        }
    }
    out
}

/// Indices into `ITEMS` of the items in `folder`.
pub fn items_in(folder: &str) -> Vec<usize> {
    (0..ITEMS.len()).filter(|&i| ITEMS[i].folder == folder).collect()
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

pub const MAX_NAME: usize = 20;

#[derive(Clone, Debug)]
pub struct Preset {
    pub name: String,
    pub settings: Settings,
}

pub fn name_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric() || " -_.".contains(c)
}

/// A name made of allowed characters only, never empty.
pub fn clean_name(name: &str) -> String {
    let s: String = name.chars().filter(|&c| name_char_ok(c)).take(MAX_NAME).collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        "Preset".to_string()
    } else {
        s
    }
}

/// `base`, with a number added if another name already matches it.
pub fn unique_name(base: &str, taken: &[&str]) -> String {
    let base = clean_name(base);
    let is_taken = |n: &str| taken.iter().any(|t| t.eq_ignore_ascii_case(n));
    if !is_taken(&base) {
        return base;
    }
    for n in 2.. {
        let suffix = format!(" {n}");
        let keep = MAX_NAME.saturating_sub(suffix.len());
        let name = format!("{}{suffix}", base.chars().take(keep).collect::<String>().trim_end());
        if !is_taken(&name) {
            return name;
        }
    }
    unreachable!()
}

pub fn builtin() -> Vec<Preset> {
    let d = Settings::default();
    let p = |name: &str, settings: Settings| Preset { name: name.to_string(), settings };
    vec![
        // Template: every field at its default value. Copy it, rename it and
        // change what you want; fields left out fall back to `..d`.
        //
        // p(
        //     "Example",
        //     Settings {
        //         // Obstacles
        //         obstacle_count: 0.0, // 0 = auto
        //         obstacle_count_variance: 0.25,
        //         obstacle_scale: 1.0,
        //         obstacle_scale_variance: 0.0,
        //         obstacle_thickness: 14.0,
        //         obstacle_thickness_variance: 0.3,
        //         obstacle_duration: 15.0,
        //         obstacle_duration_variance: 0.35,
        //         obstacle_clearance: 28.0,
        //         hole_duration: 10.0,
        //         hole_duration_variance: 0.4,
        //         // Obstacle kinds
        //         slope_weight: 3.0,
        //         diamond_weight: 1.2,
        //         circle_weight: 1.4,
        //         funnel_weight: 1.0,
        //         long_hole_weight: 1.2,
        //         circle_hole_weight: 1.2,
        //         // Streams
        //         stream_count: 8.0, // 0 = auto
        //         stream_count_variance: 0.5,
        //         stream_density: 0.5,
        //         stream_density_variance: 1.0,
        //         stream_width: 30.0,
        //         stream_width_variance: 0.7,
        //         stream_duration: 3.0,
        //         stream_duration_variance: 0.8,
        //         // Influence
        //         streams_attract_obstacles: 0.8,
        //         obstacles_attract_streams: 0.6,
        //         obstacle_chaining: 0.7,
        //         wide_stream: 60.0,
        //         // Weirdness
        //         weird_kind: 0.15,
        //         weird_placement: 0.15,
        //         weird_shape: 0.15,
        //         weird_overlap: 0.10,
        //         weird_redirect: 0.15,
        //         weird_loops: 0.10,
        //         loop_memory: 8.0,
        //         // Colors
        //         sand_lightness: 0.80,
        //         sand_chroma: 0.11,
        //         hue_start: 0.0,
        //         hue_span: 360.0,
        //         hue_speed: 15.0,
        //     },
        // ),
        p(
            "Default",
            Settings {
                ..d
            },
        ),
        p(
            "Slow",
            Settings {
                obstacle_duration: 60.0,
                obstacle_duration_variance: 0.6,
                stream_width: 5.0,
                stream_density: 0.8,
                stream_duration: 60.0,
                stream_duration_variance: 0.6,
                hue_speed: 6.0,
                weird_kind: 0.0,
                weird_placement: 0.0,
                weird_shape: 0.0,
                ..d
            },
        ),
        p(
            "Cascade",
            Settings {
                obstacle_count: 80.0,
                obstacle_count_variance: 0.2,
                obstacle_thickness: 5.0,
                obstacle_thickness_variance: 0.0,
                obstacle_scale: 0.3,
                obstacle_scale_variance: 1.0,
                obstacle_duration: 10.0,
                obstacle_duration_variance: 0.5,
                stream_count: 4.0,
                stream_count_variance: 0.5,
                stream_width: 15.0,
                stream_density: 0.8,
                stream_duration: 10.0,
                streams_attract_obstacles: 0.8,
                obstacles_attract_streams: 0.2,
                obstacle_chaining: 0.8,
                slope_weight: 8.0,
                long_hole_weight: 0.0,
                circle_hole_weight: 0.0,
                weird_kind: 0.0,
                weird_placement: 0.0,
                weird_shape: 0.0,
                weird_loops: 0.9,
                ..d
            },
        ),
        p(
            "Boulders",
            Settings {
                slope_weight: 0.0,
                funnel_weight: 0.0,
                diamond_weight: 3.0,
                circle_weight: 4.0,
                circle_hole_weight: 2.0,
                long_hole_weight: 0.0,
                obstacle_count: 20.0,
                obstacle_clearance: 40.0,
                stream_width: 120.0,
                stream_density: 0.85,
                stream_density_variance: 0.05,
                obstacle_scale: 1.2,
                obstacle_scale_variance: 0.5,
                weird_overlap: 0.0,
                ..d
            },
        ),
        p(
            "Hourglass",
            Settings {
                funnel_weight: 8.0,
                slope_weight: 0.5,
                diamond_weight: 0.0,
                circle_weight: 0.0,
                long_hole_weight: 0.5,
                circle_hole_weight: 0.0,
                obstacle_count: 20.0,
                obstacle_scale_variance: 0.6,
                stream_count: 30.0,
                stream_width: 5.0,
                weird_shape: 0.05,
                ..d
            },
        ),
        p(
            "Chaos",
            Settings {
                obstacle_count: 40.0,
                obstacle_count_variance: 0.8,
                obstacle_duration: 8.0,
                obstacle_duration_variance: 0.8,
                stream_count: 12.0,
                stream_count_variance: 0.8,
                long_hole_weight: 3.0,
                circle_hole_weight: 3.0,
                weird_kind: 0.9,
                weird_placement: 0.9,
                weird_shape: 0.9,
                weird_overlap: 0.4,
                weird_redirect: 0.5,
                weird_loops: 0.6,
                hue_speed: 30.0,
                ..d
            },
        ),
        p(
            "Rain",
            Settings {
                obstacle_count_variance: 0.5,
                obstacle_duration: 10.0,
                obstacle_duration_variance: 0.5,
                obstacle_clearance: 20.0,
                stream_count: 1.0,
                stream_count_variance: 0.0,
                stream_density: 0.08,
                stream_density_variance: 0.0,
                stream_width: 3840.0,
                stream_width_variance: 0.0,
                stream_duration: 600.0,
                stream_duration_variance: 0.0,
                long_hole_weight: 3.2,
                obstacle_chaining: 0.9,
                weird_kind: 0.0,
                weird_placement: 0.0,
                weird_shape: 0.0,
                weird_overlap: 0.05,
                weird_redirect: 0.1,
                weird_loops: 1.0,
                loop_memory: 0.0,
                hue_start: 220.0,
                hue_span: 50.0,
                sand_lightness: 0.55,
                sand_chroma: 0.2,
                hue_speed: 2.0,
                ..d
            },
        ),
        p(
            "Magma",
            Settings {
                stream_count: 30.0,
                stream_width: 5.0,
                stream_width_variance: 0.0,
                stream_density: 1.0,
                stream_duration: 0.2,
                stream_duration_variance: 1.0,
                hue_start: 18.0,
                hue_span: 27.0,
                sand_lightness: 0.53,
                sand_chroma: 0.2,
                hue_speed: 60.0,
                ..d
            },
        ),
        p(
            "Eden's Favourite",
            Settings {
                obstacle_count_variance: 0.5,
                obstacle_duration: 10.0,
                obstacle_duration_variance: 0.5,
                obstacle_clearance: 20.0,
                obstacle_scale: 0.5,
                stream_count: 4.0,
                stream_count_variance: 0.5,
                stream_density: 0.08,
                stream_density_variance: 0.5,
                stream_width: 1000.0,
                stream_width_variance: 0.8,
                stream_duration: 10.0,
                stream_duration_variance: 1.0,
                funnel_weight: 2.0,
                long_hole_weight: 3.2,
                obstacle_chaining: 0.3,
                streams_attract_obstacles: 0.5,
                weird_kind: 0.1,
                weird_placement: 0.1,
                weird_shape: 0.1,
                weird_overlap: 0.05,
                weird_redirect: 0.1,
                weird_loops: 1.0,
                loop_memory: 0.0,
                sand_lightness: 0.75,
                sand_chroma: 0.15,
                hue_speed: 10.0,
                ..d
            },
        ),
    ]
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// Where the current settings and the custom presets are kept.
pub struct Store {
    dir: Option<PathBuf>,
}

impl Store {
    /// `%APPDATA%\screensaver_sand` (or `$HOME/.config/screensaver_sand`).
    pub fn system() -> Store {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        Store { dir: base.map(|b| b.join("screensaver_sand")) }
    }

    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Store {
        Store { dir: Some(dir) }
    }

    fn read(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.dir.as_ref()?.join(file)).ok()
    }

    fn write(&self, file: &str, text: &str) {
        let Some(dir) = &self.dir else {
            return;
        };
        let _ = fs::create_dir_all(dir);
        let _ = fs::write(dir.join(file), text);
    }

    pub fn load_settings(&self) -> Settings {
        self.read("settings.txt").map_or_else(Settings::default, |t| Settings::from_text(&t))
    }

    pub fn save_settings(&self, s: &Settings) {
        self.write("settings.txt", &s.to_text());
    }

    pub fn load_presets(&self) -> Vec<Preset> {
        let Some(text) = self.read("presets.txt") else {
            return Vec::new();
        };
        let mut out: Vec<Preset> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                out.push(Preset { name: clean_name(name), settings: Settings::default() });
            } else if let Some(p) = out.last_mut() {
                p.settings.apply_line(line);
            }
        }
        out
    }

    pub fn save_presets(&self, presets: &[Preset]) {
        let mut text = String::new();
        for p in presets {
            let _ = writeln!(text, "[{}]", p.name);
            text.push_str(&p.settings.to_text());
        }
        self.write("presets.txt", &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_inside_their_ranges() {
        let d = Settings::default();
        for item in ITEMS {
            let v = (item.get)(&d);
            assert!(v >= item.min && v <= item.max, "{} = {v}", item.key);
        }
    }

    #[test]
    fn text_round_trip_and_clamping() {
        let mut s = Settings::default();
        s.stream_width = 123.0;
        s.hue_start = 90.0;
        assert_eq!(Settings::from_text(&s.to_text()), s);

        let t = Settings::from_text("stream_width=99999\nhue_start=-5\nbogus=1\nstream_count=abc\nloop_memory=NaN");
        assert_eq!(t.stream_width, 3840.0);
        assert_eq!(t.hue_start, 0.0);
        assert_eq!(t.stream_count, Settings::default().stream_count);
        assert_eq!(t.loop_memory, Settings::default().loop_memory);
    }

    #[test]
    fn adjust_steps_and_stops_at_the_ends() {
        let i = ITEMS.iter().position(|it| it.key == "weird_kind").unwrap();
        let mut s = Settings::default();
        s.adjust(i, 1.0, 1.0);
        assert_eq!(s.weird_kind, 0.2);
        s.adjust(i, 1.0, 10.0);
        assert_eq!(s.weird_kind, 0.7);
        s.adjust(i, 1.0, 100.0);
        assert_eq!(s.weird_kind, 1.0);
        s.adjust(i, -1.0, 100.0);
        assert_eq!(s.weird_kind, 0.0);
    }

    #[test]
    fn builtins_have_unique_names() {
        let names: Vec<String> = builtin().into_iter().map(|p| p.name).collect();
        for (i, n) in names.iter().enumerate() {
            assert!(!names[..i].contains(n), "{n}");
            assert_eq!(&clean_name(n), n);
        }
    }

    #[test]
    fn names_are_cleaned_and_made_unique() {
        assert_eq!(clean_name("  a[b]c/d  "), "abcd");
        assert_eq!(clean_name("[]"), "Preset");
        assert_eq!(unique_name("Mine", &["Mine", "mine 2"]), "Mine 3");
        assert!(unique_name("12345678901234567890", &["12345678901234567890"]).len() <= MAX_NAME);
    }

    #[test]
    fn store_round_trip() {
        let dir = std::env::temp_dir().join(format!("sand_store_test_{}", std::process::id()));
        let store = Store::at(dir.clone());
        assert_eq!(store.load_settings(), Settings::default());
        assert!(store.load_presets().is_empty());

        let mut s = Settings::default();
        s.stream_count = 3.0;
        store.save_settings(&s);
        assert_eq!(store.load_settings(), s);

        let presets = vec![
            Preset { name: "One".into(), settings: s },
            Preset { name: "Two words".into(), settings: Settings::default() },
        ];
        store.save_presets(&presets);
        let back = store.load_presets();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].name, "One");
        assert_eq!(back[0].settings, s);
        assert_eq!(back[1].name, "Two words");
        let _ = fs::remove_dir_all(dir);
    }
}
