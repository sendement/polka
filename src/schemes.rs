//! Colour schemes for Polka itself (not the icon theme): the classic
//! palettes, some for the holidays, and any the user drops in as JSON
//! files in ~/.config/polka/schemes/.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use egui::{Color32, Stroke, Visuals};

#[derive(Clone, Debug, PartialEq)]
pub struct Scheme {
    /// "nord", or "file:<name>" for one of the user's.
    pub id: String,
    /// Shown as it is; built-in holiday names go through tr().
    pub name: String,
    pub holiday: bool,
    pub dark: bool,
    /// Panels and windows.
    pub background: Color32,
    /// Buttons and other controls.
    pub surface: Color32,
    /// Text fields, lists' backdrop.
    pub field: Color32,
    pub text: Color32,
    pub border: Color32,
    /// The highlight, unless one of the user's own is chosen.
    pub accent: Color32,
}

/// (id, name, holiday, dark, background, surface, field, text, border, accent)
type Row = (&'static str, &'static str, bool, bool, u32, u32, u32, u32, u32, u32);

const BUILT_IN: &[Row] = &[
    ("nord", "Nord", false, true, 0x2e3440, 0x3b4252, 0x242933, 0xeceff4, 0x4c566a, 0x88c0d0),
    ("dracula", "Dracula", false, true, 0x282a36, 0x44475a, 0x1e1f29, 0xf8f8f2, 0x6272a4, 0xbd93f9),
    ("gruvbox-dark", "Gruvbox Dark", false, true, 0x282828, 0x3c3836, 0x1d2021, 0xebdbb2, 0x504945, 0xfe8019),
    ("gruvbox-light", "Gruvbox Light", false, false, 0xfbf1c7, 0xebdbb2, 0xf9f5d7, 0x3c3836, 0xd5c4a1, 0xaf3a03),
    ("solarized-dark", "Solarized Dark", false, true, 0x002b36, 0x073642, 0x00212b, 0x93a1a1, 0x2a4f58, 0x268bd2),
    ("solarized-light", "Solarized Light", false, false, 0xfdf6e3, 0xeee8d5, 0xfffbf0, 0x586e75, 0xd6cfb8, 0x268bd2),
    ("catppuccin-mocha", "Catppuccin Mocha", false, true, 0x1e1e2e, 0x313244, 0x181825, 0xcdd6f4, 0x45475a, 0xcba6f7),
    ("catppuccin-latte", "Catppuccin Latte", false, false, 0xeff1f5, 0xccd0da, 0xe6e9ef, 0x4c4f69, 0xbcc0cc, 0x8839ef),
    ("tokyo-night", "Tokyo Night", false, true, 0x1a1b26, 0x292e42, 0x16161e, 0xc0caf5, 0x3b4261, 0x7aa2f7),
    ("one-dark", "One Dark", false, true, 0x282c34, 0x3e4451, 0x21252b, 0xabb2bf, 0x4b5263, 0x61afef),
    ("monokai", "Monokai", false, true, 0x272822, 0x3e3d32, 0x1e1f1c, 0xf8f8f2, 0x575849, 0xa6e22e),
    ("rose-pine", "Rosé Pine", false, true, 0x191724, 0x26233a, 0x1f1d2e, 0xe0def4, 0x403d52, 0xebbcba),
    ("new-year", "New Year", true, true, 0x0f1a2e, 0x1c2c48, 0x0a1322, 0xe8f0ff, 0x2e4268, 0xe0b84f),
    ("christmas", "Christmas", true, true, 0x12261c, 0x1f3b2c, 0x0c1a13, 0xf3efe6, 0x2f5640, 0xd63a3a),
    ("lunar-new-year", "Lunar New Year", true, true, 0x2a0e0e, 0x45181a, 0x1d0909, 0xfbe9d0, 0x63262a, 0xf2c14e),
    ("valentine", "Valentine's Day", true, true, 0x2b1220, 0x45203a, 0x1f0c17, 0xffe4ef, 0x5e2c4c, 0xff5c8a),
    ("st-patrick", "St. Patrick's Day", true, true, 0x0f2416, 0x1a3a24, 0x0a1a0f, 0xe6f5e9, 0x2b5636, 0x43c463),
    ("easter", "Easter", true, false, 0xfbf7ef, 0xece3f5, 0xffffff, 0x4a4458, 0xd9cfe6, 0x9b7fd1),
    ("halloween", "Halloween", true, true, 0x1b1420, 0x2d2133, 0x120d16, 0xf5e6d3, 0x44304d, 0xff7a1a),
];

fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// The built-in ones, classic first.
pub fn built_in() -> Vec<Scheme> {
    BUILT_IN
        .iter()
        .map(|&(id, name, holiday, dark, background, surface, field, text, border, accent)| Scheme {
            id: id.into(),
            name: name.into(),
            holiday,
            dark,
            background: hex(background),
            surface: hex(surface),
            field: hex(field),
            text: hex(text),
            border: hex(border),
            accent: hex(accent),
        })
        .collect()
}

/// Where the user's own go.
pub fn dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("polka/schemes"))
}

/// Make the folder for one's own, with an example in it when it's new.
pub fn prepare(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if std::fs::read_dir(dir)?.next().is_none() {
        std::fs::write(dir.join("example.json.off"), EXAMPLE)?;
    }
    Ok(())
}

/// What a scheme file looks like (".off": not picked up until renamed).
pub const EXAMPLE: &str = r##"{
  "name": "My scheme",
  "dark": true,
  "background": "#1e1e2e",
  "surface": "#313244",
  "field": "#181825",
  "text": "#cdd6f4",
  "border": "#45475a",
  "accent": "#cba6f7"
}
"##;

/// All there are: the built-in ones, then the user's (by name).
pub fn all() -> Vec<Scheme> {
    let mut v = built_in();
    let mut own: Vec<Scheme> = dir()
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| match load(&p) {
            Ok(s) => Some(s),
            Err(e) => {
                crate::log::line(format!("colour scheme {}: {e}", p.display()));
                None
            }
        })
        .collect();
    own.sort_by_key(|s| s.name.to_lowercase());
    v.extend(own);
    v
}

/// One of the user's: background, surface and text are needed; the rest
/// follow from them when left out.
///
/// ```json
/// { "name": "Mine", "dark": true, "background": "#1e1e2e", "surface": "#313244",
///   "field": "#181825", "text": "#cdd6f4", "border": "#45475a", "accent": "#cba6f7" }
/// ```
pub fn load(path: &Path) -> Result<Scheme, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse(&text, path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
}

fn parse(text: &str, stem: String) -> Result<Scheme, String> {
    let j: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let color = |key: &str| -> Result<Option<Color32>, String> {
        match j.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(s)) => parse_color(s).map(Some).ok_or_else(|| format!("{key}: not a colour: {s}")),
            Some(_) => Err(format!("{key}: give it as \"#rrggbb\"")),
        }
    };
    let need = |key: &str| color(key)?.ok_or_else(|| format!("no \"{key}\""));
    let background = need("background")?;
    let surface = need("surface")?;
    let text = need("text")?;
    let dark = j.get("dark").and_then(|d| d.as_bool()).unwrap_or_else(|| luminance(background) < 0.5);
    let edge = if dark { Color32::BLACK } else { Color32::WHITE };
    Ok(Scheme {
        id: format!("file:{stem}"),
        name: j.get("name").and_then(|n| n.as_str()).map(str::to_string).unwrap_or(stem),
        holiday: false,
        dark,
        background,
        surface,
        field: color("field")?.unwrap_or_else(|| mix(background, edge, 0.3)),
        text,
        border: color("border")?.unwrap_or_else(|| mix(surface, text, 0.15)),
        accent: color("accent")?.unwrap_or(Color32::from_rgb(0x35, 0x84, 0xe4)),
    })
}

/// "#rrggbb", "#rgb" or "rrggbb".
fn parse_color(s: &str) -> Option<Color32> {
    let h = s.trim().trim_start_matches('#');
    let n = |a: &str| u8::from_str_radix(a, 16).ok();
    match h.len() {
        6 => Some(Color32::from_rgb(n(&h[0..2])?, n(&h[2..4])?, n(&h[4..6])?)),
        3 => {
            let d = |i: usize| n(&h[i..i + 1]).map(|v| v * 17);
            Some(Color32::from_rgb(d(0)?, d(1)?, d(2)?))
        }
        _ => None,
    }
}

fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

/// `a` moved towards `b` by `t`.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

impl Scheme {
    /// The name to show: holidays in the user's language.
    pub fn label(&self) -> String {
        match BUILT_IN.iter().find(|r| r.0 == self.id && r.2) {
            Some(r) => crate::i18n::tr(r.1).to_string(),
            None => self.name.clone(),
        }
    }

    /// egui's look in these colours.
    pub fn visuals(&self) -> Visuals {
        let mut v = if self.dark { Visuals::dark() } else { Visuals::light() };
        let (bg, text) = (self.background, self.text);
        // The strongest text: past the scheme's own, towards white or black.
        let strong = mix(text, if self.dark { Color32::WHITE } else { Color32::BLACK }, 0.5);
        v.panel_fill = bg;
        v.window_fill = mix(bg, self.surface, 0.3);
        v.window_stroke = Stroke::new(1.0, self.border);
        v.extreme_bg_color = self.field;
        v.faint_bg_color = mix(bg, text, 0.03);
        v.code_bg_color = self.surface;
        v.hyperlink_color = self.accent;
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = bg;
        w.noninteractive.weak_bg_fill = bg;
        w.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, mix(text, bg, 0.2));
        w.inactive.bg_fill = self.surface;
        w.inactive.weak_bg_fill = self.surface;
        w.inactive.fg_stroke = Stroke::new(1.0, text);
        let hover = mix(self.surface, text, 0.12);
        w.hovered.bg_fill = hover;
        w.hovered.weak_bg_fill = hover;
        w.hovered.bg_stroke = Stroke::new(1.0, mix(self.border, text, 0.4));
        w.hovered.fg_stroke = Stroke::new(1.5, strong);
        let press = mix(self.surface, text, 0.2);
        w.active.bg_fill = press;
        w.active.weak_bg_fill = press;
        w.active.bg_stroke = Stroke::new(1.0, strong);
        w.active.fg_stroke = Stroke::new(2.0, strong);
        w.open.bg_fill = bg;
        w.open.weak_bg_fill = self.surface;
        w.open.bg_stroke = Stroke::new(1.0, self.border);
        w.open.fg_stroke = Stroke::new(1.0, text);
        v
    }
}

/// The highlight when the user hasn't picked one: the scheme's, or Polka's.
static ACCENT: AtomicU32 = AtomicU32::new(u32::MAX);

pub fn set_default_accent(c: Option<Color32>) {
    ACCENT.store(c.map_or(u32::MAX, |c| (c.r() as u32) << 16 | (c.g() as u32) << 8 | c.b() as u32), Ordering::Relaxed);
}

pub fn default_accent() -> [u8; 3] {
    match ACCENT.load(Ordering::Relaxed) {
        u32::MAX => crate::config::DEFAULT_ACCENT,
        c => [(c >> 16) as u8, (c >> 8) as u8, c as u8],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_schemes() {
        let s = parse(r##"{"name": "Mine", "background": "#1e1e2e", "surface": "#313244", "text": "#cdd6f4", "accent": "#f0a"}"##, "mine".into()).unwrap();
        assert_eq!((s.id.as_str(), s.name.as_str(), s.dark), ("file:mine", "Mine", true));
        assert_eq!(s.accent, Color32::from_rgb(0xff, 0x00, 0xaa));
        // Left out: made from the rest.
        assert_ne!(s.field, s.background);
        let light = parse(r##"{"background": "#fafafa", "surface": "#eeeeee", "text": "#222"}"##, "paper".into()).unwrap();
        assert_eq!((light.name.as_str(), light.dark), ("paper", false));
        assert!(parse(r##"{"background": "#fafafa", "text": "#222"}"##, "x".into()).unwrap_err().contains("surface"));
        assert!(parse(r##"{"background": "blue", "surface": "#eee", "text": "#222"}"##, "x".into()).unwrap_err().contains("background"));
        // Every built-in one has its own id, and text that stands out.
        let all = built_in();
        let ids: std::collections::HashSet<_> = all.iter().map(|s| &s.id).collect();
        assert_eq!(ids.len(), all.len());
        for s in &all {
            assert!((luminance(s.text) - luminance(s.background)).abs() > 0.4, "{}", s.id);
            assert_eq!(s.dark, luminance(s.background) < 0.5, "{}", s.id);
        }
    }
}
