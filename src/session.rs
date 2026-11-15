//! The tabs kept for the next start (as set): written as a window closes,
//! read back by the first window started while no other one is open.

use crate::app::{App, Pane, Tab};
use crate::vfs::Location;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
pub struct Saved {
    pub tabs: Vec<SavedTab>,
    /// The active side's tab, the other side's, the active side.
    pub tab: usize,
    pub other: usize,
    pub side: usize,
}

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
pub struct SavedTab {
    /// The folder (or the archive) shown.
    pub path: PathBuf,
    /// Inside an archive: where.
    pub inner: Option<String>,
    /// In the right pane's own row.
    pub right: bool,
}

fn file() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("polka/tabs.toml"))
}

/// Held for as long as this window runs: shared by every window, so taking
/// it whole means none other is open.
static LOCK: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();

/// No other Polka window open (the first one started takes the tabs kept).
/// From then on this one counts as open.
pub fn first_window() -> bool {
    let Some(dir) = dirs::cache_dir().map(|d| d.join("polka")) else { return true };
    let _ = std::fs::create_dir_all(&dir);
    let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("windows.lock")) else { return true };
    let alone = f.try_lock().is_ok();
    if alone {
        let _ = f.unlock();
    }
    let _ = f.lock_shared();
    let _ = LOCK.set(f);
    alone
}

/// The tabs as they are: folders only (pages are left out), each as it
/// can be found again.
pub fn of(app: &App) -> Saved {
    let mut out = Saved::default();
    // Each tab's place in the list kept (a page's: the one before it).
    let mut at = Vec::with_capacity(app.tabs.len());
    for t in &app.tabs {
        let kept = !t.own
            && match &t.pane.loc {
                Location::Local(p) => {
                    out.tabs.push(SavedTab { path: p.clone(), inner: None, right: t.right });
                    true
                }
                Location::Archive { archive, inner } => {
                    out.tabs.push(SavedTab { path: archive.clone(), inner: Some(inner.clone()), right: t.right });
                    true
                }
                Location::Search { root, .. } => {
                    out.tabs.push(SavedTab { path: root.clone(), inner: None, right: t.right });
                    true
                }
                Location::Steam(_) => false,
            };
        at.push(if kept { out.tabs.len() - 1 } else { out.tabs.len().saturating_sub(1) });
    }
    out.tab = at.get(app.tab).copied().unwrap_or(0);
    out.other = at.get(app.other_tab).copied().unwrap_or(0);
    out.side = app.side;
    out
}

pub fn save(app: &App) {
    let Some(p) = file() else { return };
    let s = of(app);
    if let Ok(text) = toml::to_string_pretty(&s) {
        let _ = std::fs::write(p, text);
    }
}

pub fn load() -> Option<Saved> {
    toml::from_str(&std::fs::read_to_string(file()?).ok()?).ok()
}

impl App {
    /// The tabs kept, back; the one the window was started for (a folder
    /// asked for) after them, the current one.
    pub fn restore_tabs(&mut self, saved: Saved, asked: bool) {
        let cfg = self.cfg.clone();
        let mut tabs: Vec<Tab> = Vec::new();
        let mut at = Vec::new();
        for t in &saved.tabs {
            let loc = match &t.inner {
                Some(inner) if t.path.is_file() => Location::Archive { archive: t.path.clone(), inner: inner.clone() },
                None if t.path.is_dir() => Location::Local(t.path.clone()),
                _ => {
                    at.push(tabs.len().saturating_sub(1));
                    continue;
                }
            };
            tabs.push(Tab { right: t.right, ..Tab::new(Pane::new(loc, &cfg)) });
            at.push(tabs.len() - 1);
        }
        if tabs.is_empty() {
            return;
        }
        let pick = |i: usize| at.get(i).copied().unwrap_or(0);
        let (mut tab, mut other) = (pick(saved.tab), pick(saved.other));
        let mut side = saved.side.min(1);
        if asked {
            // The folder asked for in a tab of its own, shown on the left;
            // the right side as it was.
            let right = if side == 0 { other } else { tab };
            let mut first = self.tabs.swap_remove(0);
            first.right = false;
            tabs.push(first);
            tab = tabs.len() - 1;
            other = right;
            side = 0;
        }
        self.tabs = tabs;
        self.tab = tab;
        self.other_tab = other;
        self.side = if cfg.dual { side } else { 0 };
        if cfg.dual {
            if self.tab == self.other_tab {
                self.open_right_pane();
            }
            self.tabs_by_side();
        }
    }
}
