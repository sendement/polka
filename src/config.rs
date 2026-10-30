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
    /// The search button looks in subfolders (else filters the folder).
    pub search_subfolders: bool,
    pub dual: bool,
    pub thumbnails: bool,
    /// Overrides the detected icon theme.
    pub icon_theme: Option<String>,
    /// Folders in this colour, in themes that come in several (Papirus).
    pub folder_color: Option<String>,
    /// Polka's colour scheme (None: the standard dark or light look).
    pub scheme: Option<String>,
    /// UI zoom; bump it up for couch use.
    pub zoom: f32,
    /// The same in the game mode (gamescope), where the screen is often a
    /// handheld's: kept apart so neither spoils the other.
    pub zoom_game: f32,
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
    pub open_music: bool,
    pub open_videos: bool,
    pub open_pdf: bool,
    /// Word processing (doc, docx, odt, rtf…).
    pub open_docs: bool,
    /// Spreadsheets, CSV too.
    pub open_sheets: bool,
    pub open_slides: bool,
    /// Show a multi-volume archive as one item until it's pointed at.
    pub collapse_volumes: bool,
    /// Disk usage page: rings, tiles or list.
    pub usage_mode: u8,
    /// Light or dark: 0 as the desktop, 1 dark, 2 light.
    pub appearance: u8,
    /// Interface language code; None follows the system.
    pub language: Option<String>,
    pub terminal: TermPrefs,
    /// Passwords typed show nothing at all (else an asterisk a character).
    pub password_hidden: bool,
    /// On-screen keyboard layouts ⇄ goes through (codes; empty: English
    /// and the interface's own).
    pub osk_layouts: Vec<String>,
    /// ✕ on a package installs it (else archives open as folders).
    pub install_on_open: bool,
    /// The right pane opens where it was (else beside the left one).
    pub remember_right: bool,
    /// Network places.
    pub connections: Vec<crate::net::Conn>,
    /// Where the right pane was last.
    pub right_dir: Option<std::path::PathBuf>,
    /// A finger opens things on a double tap (else on one; then a tap on
    /// the icon chooses).
    pub touch_double: bool,
    /// The desktop's title bar on the window; else Polka's own top bar is
    /// it (dragged by the address, with its own buttons).
    pub system_titlebar: bool,
    /// Polka's own title bar: its buttons' look, order (one of
    /// titlebar::ORDERS) and side.
    pub win_style: crate::titlebar::Style,
    pub win_order: u8,
    pub win_left: bool,
    /// Where the tabs sit in their row: 0 left, 1 in the middle, 2 right.
    pub tabs_align: u8,
    /// The buttons on the top bar and beside the tabs.
    pub toolbars: crate::toolbar::Toolbars,
    /// Sidebar places not shown, and the order chosen for them (each kept
    /// within its section), by their keys.
    pub side_hidden: Vec<String>,
    /// Typing in the file list starts the filter.
    pub type_filter: bool,
    pub side_order: Vec<String>,
    /// The status bar's figures.
    pub status: crate::status::Shown,
    /// The gamepad's button hints: shown while it's in use, always, never.
    pub pad_hints: PadHints,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PadHints {
    #[default]
    Auto,
    Always,
    Never,
}

impl PadHints {
    pub const ALL: [PadHints; 3] = [PadHints::Auto, PadHints::Always, PadHints::Never];

    pub fn label(self) -> &'static str {
        crate::i18n::tr(match self {
            PadHints::Auto => "While in use",
            PadHints::Always => "Always",
            PadHints::Never => "Never",
        })
    }
}

/// The terminal page.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TermPrefs {
    /// Colour scheme name ("Polka" follows the app's light or dark).
    pub scheme: String,
    pub font_size: f32,
    /// Full path; None = the user's login shell.
    pub shell: Option<String>,
    pub scrollback: usize,
    /// 0 block, 1 bar, 2 underline.
    pub cursor: u8,
    /// Start in the folder shown in the panel (else home).
    pub here: bool,
    /// Commands kept for running again.
    pub saved: Vec<String>,
}

pub const TERM_FONT: (f32, f32, f32) = (8.0, 32.0, 1.0);
pub const SCROLLBACK: [usize; 4] = [1000, 5000, 10000, 50000];

impl Default for TermPrefs {
    fn default() -> Self {
        Self { scheme: "Polka".into(), font_size: 14.0, shell: None, scrollback: 10000, cursor: 0, here: true, saved: Vec::new() }
    }
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
    /// Music player: volume 0..1 and repeat (0 off, 1 all, 2 one).
    pub volume: f32,
    pub repeat: u8,
    /// Markdown may fetch pictures from the internet.
    pub web_images: bool,
}

pub const VIEWER_FONT: (f32, f32, f32) = (9.0, 40.0, 1.0);

impl Default for ViewerPrefs {
    fn default() -> Self {
        Self { theme: "Polka".into(), font: None, font_size: 15.0, wrap: true, line_numbers: true, md_rendered: true, volume: 0.8, repeat: 0, web_images: true }
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
    /// The keyboard layouts in use.
    pub fn layouts(&self) -> Vec<String> {
        if self.osk_layouts.is_empty() { crate::keyboard::default_codes(crate::i18n::current()) } else { self.osk_layouts.clone() }
    }

    pub fn accent_rgb(&self) -> [u8; 3] {
        self.accent.unwrap_or_else(crate::schemes::default_accent)
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
            search_subfolders: true,
            dual: false,
            thumbnails: true,
            icon_theme: None,
            folder_color: None,
            scheme: None,
            zoom: 1.0,
            zoom_game: 1.0,
            accent: None,
            custom_colors: Vec::new(),
            side_icon: 20.0,
            list_icon: 22.0,
            grid_icon: 56.0,
            compress: None,
            viewer: ViewerPrefs::default(),
            open_text: true,
            open_images: true,
            open_music: true,
            open_videos: true,
            open_pdf: true,
            open_docs: true,
            open_sheets: true,
            open_slides: true,
            collapse_volumes: true,
            usage_mode: 0,
            appearance: 0,
            language: None,
            terminal: TermPrefs::default(),
            password_hidden: false,
            osk_layouts: Vec::new(),
            install_on_open: true,
            remember_right: true,
            connections: Vec::new(),
            right_dir: None,
            touch_double: true,
            system_titlebar: true,
            win_style: crate::titlebar::Style::Auto,
            win_order: 0,
            win_left: false,
            tabs_align: 1,
            toolbars: Default::default(),
            side_hidden: Vec::new(),
            type_filter: true,
            side_order: Vec::new(),
            pad_hints: PadHints::Auto,
            status: crate::status::Shown::default(),
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
