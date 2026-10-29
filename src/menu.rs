//! The settings menu: a stack of screens navigated with the arrow keys.
//!
//! Root -> folder -> adjust bar for one item, plus a preset list. The menu
//! knows nothing about drawing; `view()` describes what to show.

use crate::settings::{self, Preset, Settings, Store, ITEMS, MAX_NAME};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
    Backspace,
    Tab,
    Char(char),
}

/// What the caller has to do after a key.
#[derive(Default, Debug, PartialEq)]
pub struct Effect {
    /// `Menu::settings` changed; send them to the simulation.
    pub changed: bool,
    /// Start the simulation over.
    pub restart: bool,
    /// Save and exit.
    pub quit: bool,
}

#[derive(Clone, PartialEq, Debug)]
enum Screen {
    Root,
    Folder(&'static str),
    Adjust(usize),
    Presets,
    /// Actions for the preset at this index (built-ins first, then custom).
    PresetMenu(usize),
    Naming { index: usize, text: String },
}

struct Level {
    screen: Screen,
    sel: usize,
}

pub struct Row {
    pub label: String,
    pub value: String,
    pub arrow: bool,
}

pub enum Body {
    List { rows: Vec<Row>, sel: usize },
    Adjust { name: String, value: String, frac: f32, min: String, max: String },
    Naming { text: String },
}

pub struct View {
    pub crumbs: String,
    pub preset: String,
    pub body: Body,
    pub help: String,
    pub keys: String,
    pub status: String,
}

pub struct Menu {
    pub settings: Settings,
    pub visible: bool,
    store: Store,
    builtin: Vec<Preset>,
    custom: Vec<Preset>,
    /// Name of the preset last loaded or saved.
    current: Option<String>,
    stack: Vec<Level>,
    confirm: bool,
    status: String,
}

impl Menu {
    pub fn new(settings: Settings, store: Store) -> Menu {
        let builtin = settings::builtin();
        let custom = store.load_presets();
        let current = builtin
            .iter()
            .chain(custom.iter())
            .find(|p| p.settings == settings)
            .map(|p| p.name.clone());
        Menu {
            settings,
            visible: true,
            store,
            builtin,
            custom,
            current,
            stack: vec![Level { screen: Screen::Root, sel: 0 }],
            confirm: false,
            status: String::new(),
        }
    }

    /// Write the current settings to disk.
    pub fn save(&self) {
        self.store.save_settings(&self.settings);
    }

    // -- presets ------------------------------------------------------------

    fn preset_count(&self) -> usize {
        self.builtin.len() + self.custom.len()
    }

    fn preset(&self, i: usize) -> &Preset {
        if i < self.builtin.len() {
            &self.builtin[i]
        } else {
            &self.custom[i - self.builtin.len()]
        }
    }

    fn is_custom(&self, i: usize) -> bool {
        i >= self.builtin.len()
    }

    fn names_except(&self, skip: Option<usize>) -> Vec<String> {
        (0..self.preset_count())
            .filter(|&i| Some(i) != skip)
            .map(|i| self.preset(i).name.clone())
            .collect()
    }

    fn unique(&self, base: &str, skip: Option<usize>) -> String {
        let names = self.names_except(skip);
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        settings::unique_name(base, &refs)
    }

    /// Add a custom preset; returns its index.
    fn add_custom(&mut self, name: &str, settings: Settings) -> usize {
        let name = self.unique(name, None);
        self.custom.push(Preset { name, settings });
        self.store.save_presets(&self.custom);
        self.preset_count() - 1
    }

    fn load(&mut self, i: usize) {
        let p = self.preset(i).clone();
        self.settings = p.settings;
        self.status = format!("Loaded {}", p.name);
        self.current = Some(p.name);
    }

    fn preset_label(&self) -> String {
        let found = self
            .current
            .as_ref()
            .and_then(|n| (0..self.preset_count()).map(|i| self.preset(i)).find(|p| &p.name == n));
        match found {
            Some(p) if p.settings == self.settings => p.name.clone(),
            Some(p) => format!("{} (modified)", p.name),
            None => "Unsaved".to_string(),
        }
    }

    // -- navigation ---------------------------------------------------------

    fn top(&self) -> &Level {
        self.stack.last().expect("menu stack is never empty")
    }

    fn push(&mut self, screen: Screen) {
        self.stack.push(Level { screen, sel: 0 });
    }

    fn pop(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    fn row_count(&self, screen: &Screen) -> usize {
        match screen {
            Screen::Root => settings::folders().len() + 3,
            Screen::Folder(f) => settings::items_in(f).len(),
            Screen::Presets => 1 + self.preset_count(),
            Screen::PresetMenu(i) => {
                if self.is_custom(*i) {
                    4
                } else {
                    2
                }
            }
            Screen::Adjust(_) | Screen::Naming { .. } => 0,
        }
    }

    fn move_sel(&mut self, delta: isize) {
        let n = self.row_count(&self.top().screen) as isize;
        if n == 0 {
            return;
        }
        let level = self.stack.last_mut().expect("menu stack is never empty");
        level.sel = (level.sel as isize + delta).rem_euclid(n) as usize;
    }

    /// Handle a key press (also key repeats). `mult` scales adjustments
    /// (1, 10, 100 or 1000 depending on Shift and Ctrl).
    pub fn key(&mut self, key: Key, mult: f32) -> Effect {
        let mut fx = Effect::default();
        if !self.visible {
            self.visible = true;
            return fx;
        }
        if matches!(self.top().screen, Screen::Naming { .. }) {
            self.key_naming(key);
            return fx;
        }
        if key == Key::Tab {
            self.visible = false;
            return fx;
        }
        if matches!(key, Key::Char('r' | 'R')) {
            fx.restart = true;
            self.status = "Restarted".to_string();
            return fx;
        }
        let confirmed = std::mem::take(&mut self.confirm);
        self.status.clear();
        let sel = self.top().sel;
        let enter = matches!(key, Key::Right | Key::Enter);
        let back = matches!(key, Key::Left | Key::Esc);
        match self.top().screen.clone() {
            Screen::Root => match key {
                Key::Up => self.move_sel(-1),
                Key::Down => self.move_sel(1),
                Key::Esc => fx.quit = true,
                _ if enter => self.activate_root(sel, &mut fx),
                _ => {}
            },
            Screen::Folder(f) => match key {
                Key::Up => self.move_sel(-1),
                Key::Down => self.move_sel(1),
                _ if enter => self.push(Screen::Adjust(settings::items_in(f)[sel])),
                _ if back => self.pop(),
                _ => {}
            },
            Screen::Adjust(i) => {
                let before = self.settings;
                match key {
                    Key::Up => self.settings.adjust(i, 1.0, mult),
                    Key::Down => self.settings.adjust(i, -1.0, mult),
                    Key::Backspace => {
                        let d = (ITEMS[i].get)(&Settings::default());
                        self.settings.set_item(i, d);
                        self.status = "Reset to default".to_string();
                    }
                    Key::Enter => self.pop(),
                    _ if back => self.pop(),
                    _ => {}
                }
                fx.changed = self.settings != before;
            }
            Screen::Presets => match key {
                Key::Up => self.move_sel(-1),
                Key::Down => self.move_sel(1),
                _ if back => self.pop(),
                Key::Enter if sel > 0 => {
                    self.load(sel - 1);
                    fx.changed = true;
                }
                _ if enter && sel == 0 => {
                    let index = self.add_custom("My preset", self.settings);
                    let name = self.preset(index).name.clone();
                    self.current = Some(name.clone());
                    self.stack.last_mut().expect("menu stack is never empty").sel = index + 1;
                    self.push(Screen::Naming { index, text: name });
                }
                _ if enter => self.push(Screen::PresetMenu(sel - 1)),
                _ => {}
            },
            Screen::PresetMenu(i) => match key {
                Key::Up => self.move_sel(-1),
                Key::Down => self.move_sel(1),
                _ if back => self.pop(),
                _ if enter => self.preset_action(i, sel, confirmed, &mut fx),
                _ => {}
            },
            Screen::Naming { .. } => {}
        }
        fx
    }

    fn activate_root(&mut self, sel: usize, fx: &mut Effect) {
        let folders = settings::folders();
        if sel == 0 {
            self.push(Screen::Presets);
        } else if sel <= folders.len() {
            self.push(Screen::Folder(folders[sel - 1]));
        } else if sel == folders.len() + 1 {
            fx.restart = true;
            self.status = "Restarted".to_string();
        } else {
            fx.quit = true;
        }
    }

    fn preset_action(&mut self, i: usize, row: usize, confirmed: bool, fx: &mut Effect) {
        let custom = self.is_custom(i);
        match row {
            0 => {
                self.load(i);
                fx.changed = true;
                self.pop();
            }
            1 if custom => {
                let ci = i - self.builtin.len();
                self.custom[ci].settings = self.settings;
                self.store.save_presets(&self.custom);
                self.current = Some(self.custom[ci].name.clone());
                self.status = "Saved".to_string();
            }
            1 => {
                let p = self.preset(i).clone();
                let index = self.add_custom(&format!("{} copy", p.name), p.settings);
                self.pop();
                self.stack.last_mut().expect("menu stack is never empty").sel = index + 1;
                self.status = format!("Copied as {}", self.preset(index).name);
            }
            2 => {
                let text = self.preset(i).name.clone();
                self.push(Screen::Naming { index: i, text });
            }
            _ if confirmed => {
                let ci = i - self.builtin.len();
                let gone = self.custom.remove(ci);
                self.store.save_presets(&self.custom);
                if self.current.as_ref() == Some(&gone.name) {
                    self.current = None;
                }
                self.pop();
                let last = self.preset_count();
                let level = self.stack.last_mut().expect("menu stack is never empty");
                level.sel = level.sel.min(last);
                self.status = format!("Deleted {}", gone.name);
            }
            _ => {
                self.confirm = true;
                self.status = "Press again to delete".to_string();
            }
        }
    }

    fn key_naming(&mut self, key: Key) {
        let Screen::Naming { index, text } = self.top().screen.clone() else {
            return;
        };
        match key {
            Key::Char(c) if settings::name_char_ok(c) => {
                if text.chars().count() < MAX_NAME {
                    self.set_naming(text + &c.to_string());
                }
            }
            Key::Backspace => {
                let mut t = text;
                t.pop();
                self.set_naming(t);
            }
            Key::Enter => {
                let ci = index - self.builtin.len();
                let old = self.custom[ci].name.clone();
                let name = self.unique(&text, Some(index));
                self.custom[ci].name = name.clone();
                if self.current.as_ref() == Some(&old) {
                    self.current = Some(name);
                }
                self.store.save_presets(&self.custom);
                self.pop();
            }
            Key::Esc => self.pop(),
            _ => {}
        }
    }

    fn set_naming(&mut self, new_text: String) {
        if let Screen::Naming { text, .. } = &mut self.stack.last_mut().expect("menu stack is never empty").screen {
            *text = new_text;
        }
    }

    // -- description for drawing --------------------------------------------

    pub fn view(&self) -> View {
        let level = self.top();
        let folders = settings::folders();
        let mut crumbs = "Settings".to_string();
        let mut help = String::new();
        let mut keys = "Up/Down move|Right open|Left back".to_string();

        let body = match &level.screen {
            Screen::Root => {
                keys = "Up/Down move|Right open|Esc exit|Tab hide|R restart".to_string();
                let mut rows = vec![Row {
                    label: "Presets".into(),
                    value: self.preset_label(),
                    arrow: true,
                }];
                rows.extend(folders.iter().map(|f| Row {
                    label: (*f).into(),
                    value: String::new(),
                    arrow: true,
                }));
                rows.push(Row { label: "Restart simulation".into(), value: String::new(), arrow: false });
                rows.push(Row { label: "Save and exit".into(), value: String::new(), arrow: false });
                help = match level.sel {
                    0 => "Load, save and manage presets.".to_string(),
                    n if n <= folders.len() => {
                        format!("{} settings.", folders[n - 1])
                    }
                    _ => String::new(),
                };
                Body::List { rows, sel: level.sel }
            }
            Screen::Folder(f) => {
                crumbs = format!("Settings > {f}");
                let items = settings::items_in(f);
                help = ITEMS[items[level.sel]].help.to_string();
                let rows = items
                    .iter()
                    .map(|&i| Row {
                        label: ITEMS[i].name.into(),
                        value: ITEMS[i].text(&self.settings),
                        arrow: true,
                    })
                    .collect();
                Body::List { rows, sel: level.sel }
            }
            Screen::Adjust(i) => {
                let item = &ITEMS[*i];
                crumbs = format!("Settings > {} > {}", item.folder, item.name);
                help = item.help.to_string();
                keys = "Up/Down adjust|Shift x10|Ctrl x100|Both x1000|Bksp reset|Left back".to_string();
                Body::Adjust {
                    name: item.name.to_string(),
                    value: item.text(&self.settings),
                    frac: item.fraction(&self.settings).clamp(0.0, 1.0),
                    min: item.unit.format(item.min),
                    max: item.unit.format(item.max),
                }
            }
            Screen::Presets => {
                crumbs = "Settings > Presets".to_string();
                keys = "Up/Down move|Enter load|Right more|Left back".to_string();
                let mut rows = vec![Row {
                    label: "+ Save current as new".into(),
                    value: String::new(),
                    arrow: false,
                }];
                rows.extend((0..self.preset_count()).map(|i| {
                    let p = self.preset(i);
                    let active = self.current.as_ref() == Some(&p.name) && p.settings == self.settings;
                    Row {
                        label: p.name.clone(),
                        value: if active { "active".into() } else if self.is_custom(i) { "custom".into() } else { String::new() },
                        arrow: true,
                    }
                }));
                help = match level.sel {
                    0 => "Stores the current settings as a new custom preset.".to_string(),
                    n if self.is_custom(n - 1) => "Custom preset. Enter loads it; Right can rename or delete it.".to_string(),
                    _ => "Built-in preset. Enter loads it; Right can copy it.".to_string(),
                };
                Body::List { rows, sel: level.sel }
            }
            Screen::PresetMenu(i) => {
                crumbs = format!("Settings > Presets > {}", self.preset(*i).name);
                keys = "Up/Down move|Enter or Right select|Left back".to_string();
                let labels: Vec<&str> = if self.is_custom(*i) {
                    vec![
                        "Load",
                        "Overwrite with current",
                        "Rename",
                        if self.confirm { "Really delete?" } else { "Delete" },
                    ]
                } else {
                    vec!["Load", "Copy to custom"]
                };
                let rows = labels
                    .into_iter()
                    .map(|l| Row { label: l.into(), value: String::new(), arrow: false })
                    .collect();
                Body::List { rows, sel: level.sel }
            }
            Screen::Naming { text, .. } => {
                crumbs = "Settings > Presets > Name".to_string();
                keys = "Type a name|Enter ok|Esc cancel".to_string();
                Body::Naming { text: text.clone() }
            }
        };
        View { crumbs, preset: self.preset_label(), body, help, keys, status: self.status.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(tag: &str) -> (Menu, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("sand_menu_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Menu::new(Settings::default(), Store::at(dir.clone())), dir)
    }

    fn press(m: &mut Menu, keys: &[Key]) -> Effect {
        let mut last = Effect::default();
        for &k in keys {
            last = m.key(k, 1.0);
        }
        last
    }

    fn item(key: &str) -> usize {
        ITEMS.iter().position(|i| i.key == key).unwrap()
    }

    #[test]
    fn adjusting_an_item_through_the_folders() {
        let (mut m, dir) = menu("adjust");
        // Root: Presets, Obstacles, Obstacle kinds, Streams, ... -> Streams.
        press(&mut m, &[Key::Down, Key::Down, Key::Down, Key::Right]);
        assert_eq!(m.view().crumbs, "Settings > Streams");
        // Streams: Count, Count drift, Density, ...
        press(&mut m, &[Key::Down, Key::Down, Key::Right]);
        assert!(m.view().crumbs.ends_with("Density"));
        let before = m.settings.stream_density;
        let fx = press(&mut m, &[Key::Up]);
        assert!(fx.changed);
        assert!(m.settings.stream_density > before);
        let fx = m.key(Key::Down, 10.0);
        assert!(fx.changed);
        assert!(m.settings.stream_density < before);
        m.key(Key::Backspace, 1.0);
        assert_eq!(m.settings.stream_density, Settings::default().stream_density);
        // Back out; Esc at the root quits.
        press(&mut m, &[Key::Left, Key::Left]);
        assert_eq!(m.view().crumbs, "Settings");
        assert!(!press(&mut m, &[Key::Left]).quit);
        assert!(press(&mut m, &[Key::Esc]).quit);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hidden_menu_ignores_the_first_key() {
        let (mut m, dir) = menu("hide");
        m.key(Key::Tab, 1.0);
        assert!(!m.visible);
        m.key(Key::Down, 1.0);
        assert!(m.visible);
        assert_eq!(m.top().sel, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn presets_load_save_rename_delete() {
        let (mut m, dir) = menu("presets");
        assert_eq!(m.view().preset, "Default");

        // Load a built-in: Presets > row 2 (Drizzle).
        press(&mut m, &[Key::Right, Key::Down, Key::Down]);
        let fx = m.key(Key::Enter, 1.0);
        assert!(fx.changed);
        assert_eq!(m.settings, m.builtin[1].settings);
        assert_eq!(m.view().preset, "Drizzle");

        // Modify it, then save as a new preset and rename it.
        m.settings.stream_width = 100.0;
        assert_eq!(m.view().preset, "Drizzle (modified)");
        press(&mut m, &[Key::Up, Key::Up, Key::Enter]);
        assert!(matches!(m.top().screen, Screen::Naming { .. }));
        for _ in 0..20 {
            m.key(Key::Backspace, 1.0);
        }
        press(&mut m, &[Key::Char('H'), Key::Char('i'), Key::Char('['), Key::Enter]);
        assert_eq!(m.custom.len(), 1);
        assert_eq!(m.custom[0].name, "Hi");
        assert_eq!(m.custom[0].settings.stream_width, 100.0);
        assert_eq!(m.view().preset, "Hi");

        // It was written to disk.
        let again = Menu::new(m.settings, Store::at(dir.clone()));
        assert_eq!(again.custom.len(), 1);
        assert_eq!(again.view().preset, "Hi");

        // Delete it: needs a confirmation.
        let n = m.builtin.len();
        assert_eq!(m.top().sel, n + 1);
        press(&mut m, &[Key::Right, Key::Down, Key::Down, Key::Down, Key::Right]);
        assert_eq!(m.custom.len(), 1);
        press(&mut m, &[Key::Right]);
        assert!(m.custom.is_empty());
        assert_eq!(m.view().preset, "Unsaved");
        assert!(Menu::new(m.settings, Store::at(dir.clone())).custom.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn copying_a_builtin_and_unique_names() {
        let (mut m, dir) = menu("copy");
        // Presets > Default > Copy to custom.
        press(&mut m, &[Key::Right, Key::Down, Key::Right, Key::Down, Key::Right]);
        assert_eq!(m.custom.len(), 1);
        assert_eq!(m.custom[0].name, "Default copy");
        // Save current as new, twice: names stay unique. The selection sits on
        // the last row, so Down wraps to the first.
        press(&mut m, &[Key::Down, Key::Right, Key::Esc]);
        assert_eq!(m.custom.len(), 2);
        press(&mut m, &[Key::Down, Key::Right, Key::Esc]);
        let names: Vec<&str> = m.custom.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names.len(), 3);
        assert_ne!(names[1], names[2]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn item_lookup_matches_keys() {
        assert_eq!(ITEMS[item("stream_width")].name, "Width");
    }
}
