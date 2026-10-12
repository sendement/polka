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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pinned: Vec::new(),
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
