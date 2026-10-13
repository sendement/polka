//! Persistent settings in ~/.config/polka/config.toml.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    #[default]
    List,
    Grid,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Date,
    /// By extension, then name.
    Type,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub pinned: Vec<PathBuf>,
    /// Chosen theme icon per pinned folder (path → icon name).
    pub pinned_icons: std::collections::BTreeMap<String, String>,
    pub show_hidden: bool,
    pub sidebar_collapsed: bool,
    pub view: ViewMode,
    pub sort: SortKey,
    /// Flip the sort key's natural order.
    pub sort_reverse: bool,
    pub dirs_first: bool,
    pub dual: bool,
    pub thumbnails: bool,
    /// Overrides the detected icon theme.
    pub icon_theme: Option<String>,
    /// UI zoom; bump it up for couch use.
    pub zoom: f32,
    /// Highlight colour; None = the built-in default.
    pub accent: Option<[u8; 3]>,
    /// Colours the user mixed and saved.
    pub custom_colors: Vec<[u8; 3]>,
    /// Icon sizes in points.
    pub side_icon: f32,
    pub list_icon: f32,
    pub grid_icon: f32,
    /// Last archive settings (never the password).
    pub compress: Option<CompressPrefs>,
    pub viewer: ViewerPrefs,
    /// Open these in Polka's own viewer (else the desktop's app).
    pub open_text: bool,
    pub open_images: bool,
    pub open_audio: bool,
    pub open_video: bool,
    /// Show a multi-volume archive as one item until it's pointed at.
    pub collapse_volumes: bool,
}

/// Text viewer look.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewerPrefs {
    pub theme: String,
    /// Monospace font family; None = the system's.
    pub font: Option<String>,
    pub font_size: f32,
    pub wrap: bool,
    pub line_numbers: bool,
    /// Markdown drawn as on GitHub (else its source).
    pub md_rendered: bool,
}

pub const VIEWER_FONT: (f32, f32, f32) = (9.0, 40.0, 1.0);

impl Default for ViewerPrefs {
    fn default() -> Self {
        Self { theme: "Polka".into(), font: None, font_size: 15.0, wrap: true, line_numbers: true, md_rendered: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompressPrefs {
    pub format: crate::ops::ArchiveFormat,
    pub method: crate::ops::Method,
    pub level: crate::ops::Level,
    pub split: Option<u64>,
    pub encrypt_names: bool,
}

pub const SIDE_ICON: (f32, f32, f32) = (16.0, 48.0, 2.0);
pub const LIST_ICON: (f32, f32, f32) = (16.0, 64.0, 2.0);
pub const GRID_ICON: (f32, f32, f32) = (32.0, 160.0, 8.0);
pub const ZOOM: (f32, f32, f32) = (0.6, 3.0, 0.1);

/// Highlight presets (GNOME's accent palette); index 0 is the default.
pub const ACCENT_PRESETS: [Option<[u8; 3]>; 10] = [
    None,
    Some([0x35, 0x84, 0xe4]),
    Some([0x21, 0x90, 0xa4]),
    Some([0x3a, 0x94, 0x4a]),
    Some([0xc8, 0x88, 0x00]),
    Some([0xed, 0x5b, 0x00]),
    Some([0xe6, 0x2d, 0x42]),
    Some([0xd5, 0x61, 0x99]),
    Some([0x91, 0x41, 0xac]),
    Some([0x6f, 0x83, 0x96]),
];
pub const DEFAULT_ACCENT: [u8; 3] = [0x00, 0x5c, 0x80];
pub const MAX_CUSTOM_COLORS: usize = 12;

impl Config {
    pub fn accent_rgb(&self) -> [u8; 3] {
        self.accent.unwrap_or(DEFAULT_ACCENT)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pinned: Vec::new(),
            pinned_icons: Default::default(),
            show_hidden: false,
            sidebar_collapsed: false,
            view: ViewMode::List,
            sort: SortKey::Name,
            sort_reverse: false,
            dirs_first: true,
            dual: false,
            thumbnails: true,
            icon_theme: None,
            zoom: 1.0,
            accent: None,
            custom_colors: Vec::new(),
            side_icon: 20.0,
            list_icon: 22.0,
            grid_icon: 56.0,
            compress: None,
            viewer: ViewerPrefs::default(),
            open_text: true,
            open_images: true,
            open_audio: false,
            open_video: false,
            collapse_volumes: true,
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("polka/config.toml"))
}

impl Config {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| match toml::from_str(&s) {
                Ok(c) => Some(c),
                Err(e) => {
                    eprintln!("polka: bad config, using defaults: {e}");
                    None
                }
            })
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = toml::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }
}
