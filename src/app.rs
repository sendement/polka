//! Application state and input/command handling. Rendering lives in view.rs.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use crate::config::{Config, SortKey, ViewMode};
use crate::i18n::{tr, trn};
use crate::icons::Icons;
use crate::input::{Action, Input, PadKind};
use crate::ops::{self, ArchiveFormat, CompressOpts, Conflict, Job, Op};
use crate::viewer::Viewer;
use crate::vfs::{self, Dest, Entry, Location};

// ---------------------------------------------------------------- panes

pub struct Pane {
    pub loc: Location,
    /// Visible entries (after the quick filter).
    pub entries: Vec<Entry>,
    /// Listing before the quick filter (hidden files and sorting applied).
    all: Vec<Entry>,
    /// Quick filter on names within this folder.
    pub filter: Option<String>,
    pub error: Option<String>,
    pub cursor: usize,
    pub selected: HashSet<String>,
    back: Vec<(Location, String)>,
    forward: Vec<(Location, String)>,
    /// Nested archives opened from inside another archive:
    /// (extracted temp archive, location it was opened from, its name there).
    nest: Vec<(PathBuf, Location, String)>,
    pub loading: Option<Receiver<std::io::Result<Vec<Entry>>>>,
    pub pending_focus: Option<String>,
    /// Listing failed for a password: (archive, wrong password).
    pub need_password: Option<(PathBuf, bool)>,
    pub scroll_to_cursor: bool,
    /// Viewport height from the last frame, for keeping the cursor visible.
    pub view_h: f32,
    pub scroll_y: f32,
    /// Grid columns from the last frame.
    pub cols: usize,
    pub page_rows: usize,
    /// Volumes of multi-volume archives in this folder, by entry name.
    pub volumes: HashMap<String, crate::volumes::Member>,
    /// Volume set under the mouse in the last frame.
    pub hover_volume: Option<String>,
    /// A selection rectangle being drawn with the mouse.
    pub band: Option<crate::mouse::Band>,
    /// Entries drawn last frame: (index, row, icon and name), for tests.
    pub drawn: Vec<(usize, egui::Rect, egui::Rect)>,
    /// The list's column headers (last frame), for tests.
    pub heads: Vec<(crate::config::SortKey, egui::Rect)>,
    /// No cursor shown (after a mouse drop took its item away); the next
    /// pad or arrow press only brings it back.
    pub cursor_hidden: bool,
    /// Show each volume set as one item (until pointed at).
    pub collapse: bool,
    /// Sets shown open now (under the mouse / cursor).
    pub expanded: Vec<String>,
    /// Collapsed sets on screen: head entry name → (set, total size, names).
    pub collapsed: HashMap<String, (crate::volumes::Member, u64, Vec<String>)>,
}

impl Pane {
    pub fn new(loc: Location, cfg: &Config) -> Self {
        let mut p = Self {
            loc: loc.clone(),
            entries: Vec::new(),
            all: Vec::new(),
            filter: None,
            error: None,
            cursor: 0,
            selected: HashSet::new(),
            back: Vec::new(),
            forward: Vec::new(),
            nest: Vec::new(),
            loading: None,
            pending_focus: None,
            need_password: None,
            scroll_to_cursor: true,
            view_h: 400.0,
            scroll_y: 0.0,
            cols: 1,
            page_rows: 10,
            volumes: HashMap::new(),
            hover_volume: None,
            band: None,
            drawn: Vec::new(),
            heads: Vec::new(),
            cursor_hidden: false,
            collapse: cfg.collapse_volumes,
            expanded: Vec::new(),
            collapsed: HashMap::new(),
        };
        p.load(loc, None, cfg);
        p
    }

    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    pub fn entry_loc(&self, e: &Entry) -> Location {
        self.loc.join(&e.name)
    }

    pub fn entry_path(&self, e: &Entry) -> Option<PathBuf> {
        self.loc.entry_path(&e.name)
    }

    /// Select or deselect an entry; a folded volume set goes as a whole.
    pub fn mark(&mut self, name: &str, on: bool) {
        let mut names = vec![name.to_string()];
        if let Some((_, _, members)) = self.collapsed.get(name) {
            names.extend(members.iter().cloned());
        }
        for n in names {
            if on {
                self.selected.insert(n);
            } else {
                self.selected.remove(&n);
            }
        }
    }

    /// A folded volume set is selected whole or not at all: any of its
    /// volumes selected (picked while it was open, say) selects them all.
    pub fn sync_sets(&mut self) {
        for (rep, (_, _, members)) in &self.collapsed {
            if self.selected.contains(rep) || members.iter().any(|m| self.selected.contains(m)) {
                self.selected.insert(rep.clone());
                self.selected.extend(members.iter().cloned());
            }
        }
    }

    /// Selection if any, else the entry under the cursor.
    pub fn targets(&self) -> Vec<Entry> {
        let picked: Vec<Entry> = if self.selected.is_empty() {
            self.current().cloned().into_iter().collect()
        } else {
            self.entries.iter().filter(|e| self.selected.contains(&e.name)).cloned().collect()
        };
        // A collapsed volume set stands for all its volumes.
        let mut out = Vec::with_capacity(picked.len());
        for e in picked {
            match self.collapsed.get(&e.name) {
                Some((_, _, names)) => out.extend(self.all.iter().filter(|x| names.contains(&x.name)).cloned()),
                None => out.push(e),
            }
        }
        out
    }

    /// Open these volume sets (others collapse); keeps the cursor's item.
    pub fn set_expanded(&mut self, keys: Vec<String>) {
        if keys == self.expanded || !self.collapse {
            return;
        }
        self.expanded = keys;
        let keep = self.scroll_to_cursor;
        self.refilter(None);
        self.scroll_to_cursor = keep;
    }

    /// `refilter` after settings changed.
    pub fn refilter_with(&mut self, focus: Option<String>, cfg: &Config) {
        self.collapse = cfg.collapse_volumes;
        self.refilter(focus);
    }

    fn load(&mut self, loc: Location, focus: Option<String>, cfg: &Config) {
        self.loc = loc.clone();
        self.collapse = cfg.collapse_volumes;
        self.expanded.clear();
        self.selected.clear();
        self.error = None;
        if !matches!(loc, Location::Local(_)) {
            // Archive indexing can take a while (tar has no index), and so
            // can searching, so they run off the UI thread.
            let (tx, rx) = crossbeam_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send(vfs::list(&loc));
            });
            self.entries.clear();
            self.all.clear();
            self.loading = Some(rx);
            self.pending_focus = focus;
        } else {
            self.loading = None;
            let r = vfs::list(&loc);
            self.apply(r, focus, cfg);
        }
    }

    /// Feed a finished background listing in, if any. Returns true if changed.
    pub fn poll(&mut self, cfg: &Config) -> bool {
        let Some(rx) = &self.loading else { return false };
        match rx.try_recv() {
            Ok(r) => {
                self.loading = None;
                let f = self.pending_focus.take();
                self.apply(r, f, cfg);
                true
            }
            Err(crossbeam_channel::TryRecvError::Empty) => false,
            Err(_) => {
                self.loading = None;
                true
            }
        }
    }

    fn apply(&mut self, r: std::io::Result<Vec<Entry>>, focus: Option<String>, cfg: &Config) {
        match r {
            Ok(mut v) => {
                if !cfg.show_hidden {
                    v.retain(|e| !e.hidden());
                }
                // Search results keep their breadth-first order (near first).
                if !matches!(self.loc, Location::Search { .. }) {
                    sort_entries(&mut v, cfg);
                }
                self.all = v;
                self.error = None;
            }
            Err(e) => {
                self.all.clear();
                self.need_password = vfs::need_password(&e).map(|n| (n.archive.clone(), n.wrong));
                self.error = Some(e.to_string());
            }
        }
        self.refilter(focus);
    }

    /// Rebuild `entries` from `all` with the current filter.
    pub fn refilter(&mut self, focus: Option<String>) {
        let focus = focus.or_else(|| self.current().map(|e| e.name.clone()));
        self.volumes = if matches!(self.loc, Location::Local(_)) {
            crate::volumes::groups(self.all.iter().filter(|e| !e.is_dir).map(|e| e.name.as_str()))
        } else {
            HashMap::new()
        };
        let shown: Vec<Entry> = match self.filter.as_deref() {
            Some(f) if !f.is_empty() => self.all.iter().filter(|e| vfs::name_matches(&e.name, f)).cloned().collect(),
            _ => self.all.clone(),
        };
        // Fold each volume set into its first item, unless it's open.
        self.collapsed.clear();
        self.entries = if self.collapse && !self.volumes.is_empty() {
            let mut out = Vec::with_capacity(shown.len());
            let mut head_of: HashMap<String, String> = HashMap::new();
            for e in shown {
                let Some(m) = self.volumes.get(&e.name).filter(|m| !self.expanded.contains(&m.key)) else {
                    out.push(e);
                    continue;
                };
                match head_of.get(&m.key) {
                    Some(head) => {
                        let c = self.collapsed.get_mut(head).unwrap();
                        c.1 += e.size;
                        c.2.push(e.name.clone());
                    }
                    None => {
                        head_of.insert(m.key.clone(), e.name.clone());
                        self.collapsed.insert(e.name.clone(), (m.clone(), e.size, vec![e.name.clone()]));
                        out.push(e);
                    }
                }
            }
            out
        } else {
            shown
        };
        // A hidden volume's place is taken by its set's first item.
        let focus = focus.map(|n| {
            if self.entries.iter().any(|e| e.name == n) {
                return n;
            }
            self.collapsed.iter().find(|(_, (_, _, names))| names.contains(&n)).map(|(h, _)| h.clone()).unwrap_or(n)
        });
        self.cursor = focus
            .and_then(|n| self.entries.iter().position(|e| e.name == n))
            .unwrap_or(0);
        let names: HashSet<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        self.selected.retain(|n| names.contains(n.as_str()));
        self.scroll_to_cursor = true;
    }

    pub fn set_filter(&mut self, f: Option<String>) {
        let f = f.filter(|s| !s.is_empty());
        if f != self.filter {
            self.filter = f;
            self.refilter(None);
        }
    }

    /// Re-list the current location, keeping cursor and selection.
    pub fn reload(&mut self, cfg: &Config) {
        let focus = self.pending_focus.take().or_else(|| self.current().map(|e| e.name.clone()));
        let keep = std::mem::take(&mut self.selected);
        let loc = self.loc.clone();
        self.load(loc, focus, cfg);
        self.selected = keep;
        let names: HashSet<String> = self.entries.iter().map(|e| e.name.clone()).collect();
        self.selected.retain(|n| names.contains(n));
    }

    pub fn navigate(&mut self, loc: Location, focus: Option<String>, cfg: &Config) {
        if loc == self.loc {
            return;
        }
        let here = self.current().map(|e| e.name.clone()).unwrap_or_default();
        self.back.push((self.loc.clone(), here));
        self.forward.clear();
        self.filter = None;
        self.load(loc, focus, cfg);
    }

    pub fn go_up(&mut self, cfg: &Config) {
        if let Location::Archive { archive, inner } = &self.loc {
            if inner.is_empty() {
                if let Some(i) = self.nest.iter().rposition(|(a, _, _)| a == archive) {
                    let (_, origin, name) = self.nest.remove(i);
                    self.navigate(origin, Some(name), cfg);
                    return;
                }
            }
        }
        if let Some((parent, name)) = self.loc.parent() {
            self.navigate(parent, Some(name), cfg);
        }
    }

    pub fn history(&mut self, forward: bool, cfg: &Config) {
        let (from, to) = if forward { (&mut self.forward, &mut self.back) } else { (&mut self.back, &mut self.forward) };
        if let Some((loc, name)) = from.pop() {
            let here = self.entries.get(self.cursor).map(|e| e.name.clone()).unwrap_or_default();
            to.push((self.loc.clone(), here));
            self.filter = None;
            self.load(loc, Some(name), cfg);
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, n - 1) as usize;
        self.scroll_to_cursor = true;
    }

    pub fn set_cursor(&mut self, i: usize) {
        if i < self.entries.len() {
            self.cursor = i;
            self.cursor_hidden = false;
            self.scroll_to_cursor = true;
        }
    }
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Folders first (optional), then the key in its natural order: names A→Z,
/// biggest and newest first, types A→Z. `sort_reverse` flips the key only.
fn sort_entries(v: &mut [Entry], cfg: &Config) {
    let ext = |e: &Entry| if e.is_dir { String::new() } else { ops::split_ext(&e.name).1.to_lowercase() };
    v.sort_by(|a, b| {
        let dirs = if cfg.dirs_first { b.is_dir.cmp(&a.is_dir) } else { Ordering::Equal };
        let key = match cfg.sort {
            SortKey::Name => natural_cmp(&a.name, &b.name),
            SortKey::Size => b.size.cmp(&a.size).then_with(|| natural_cmp(&a.name, &b.name)),
            SortKey::Date => b.mtime.cmp(&a.mtime).then_with(|| natural_cmp(&a.name, &b.name)),
            SortKey::Type => ext(a).cmp(&ext(b)).then_with(|| natural_cmp(&a.name, &b.name)),
        };
        dirs.then(if cfg.sort_reverse { key.reverse() } else { key })
    });
}

pub struct Tab {
    pub panes: [Pane; 2],
    pub active: usize,
    /// A file shown in place of the panes.
    pub viewer: Option<Box<Viewer>>,
    /// The disk usage page (kept while hidden).
    pub usage: Option<Box<crate::usage::Usage>>,
    /// The terminal page (its shells keep running while hidden).
    pub term: Option<Box<crate::terminal::Terminal>>,
}

// ---------------------------------------------------------------- overlays

#[derive(Clone, Debug)]
pub enum Cmd {
    /// The terminal page (a new shell in this folder, if given).
    TermOpen(Option<PathBuf>),
    TermNew,
    TermCloseTab(usize),
    TermSwitch(usize),
    TermHide,
    TermClose,
    TermRun(String),
    TermType(String),
    /// The on-screen keyboard, typing straight into the terminal.
    TermRunAsk,
    TermSaveAsk,
    TermSave(String),
    TermForget(usize),
    TermInterrupt,
    TermClear,
    TermSelect,
    TermCopy,
    TermPaste,
    TermShowFolder,
    TermKeys,
    TermHistoryMenu,
    TermSavedMenu,
    TermSettings,
    Open,
    Copy,
    Cut,
    Paste,
    CopyOther,
    MoveOther,
    Rename,
    TrashAsk,
    DeleteAsk,
    TrashNow(Vec<PathBuf>),
    DeleteNow(Vec<PathBuf>),
    /// Copy/move with a decided conflict policy.
    Transfer { sources: Vec<Location>, dest: Dest, mv: bool, policy: Conflict },
    DeleteInArchive { archive: PathBuf, inners: Vec<String> },
    Filter,
    Search,
    CompressAsk,
    /// Hand the file to the desktop's default app.
    OpenExternal,
    /// Show the current file in the built-in viewer.
    OpenInViewer,
    CloseViewer,
    ViewerSettings,
    ViewerEdit,
    ViewerInsertAsk,
    ViewerNewline,
    ViewerUndo,
    ViewerRedo,
    ViewerCopy,
    ViewerCut,
    ViewerPaste,
    ViewerSelectAll,
    ViewerFindAsk,
    ViewerFindOpt(u8),
    ViewerReplaceAsk,
    ViewerReplace,
    ViewerReplaceAll,
    ViewerToggleMd,
    ViewerSave,
    ViewerSaveThen(crate::viewer::glue::Then),
    ViewerDiscardThen(crate::viewer::glue::Then),
    ViewerWrap,
    ViewerNumbers,
    AudioToggle,
    AudioNext,
    AudioPrev,
    AudioPlay(usize),
    AudioRepeat,
    AudioVolume(i8),
    AudioSeek(f32),
    VideoFullscreen,
    VideoAudio,
    VideoSubs,
    ExtractHere,
    ExtractOther,
    Pin(PathBuf),
    Unpin(PathBuf),
    OpenInNewTab(Location),
    Properties,
    NewTab,
    CloseTab,
    CloseTabAt(usize),
    /// Browse for a picture to use as a pinned folder's icon.
    PickIconFile(PathBuf),
    NewFolder,
    ToggleDual,
    ToggleHidden,
    ToggleThumbnails,
    IconThemes,
    SetIconTheme(Option<String>),
    Settings,
    ChooseIcon(PathBuf),
    SetPinIconFile { folder: PathBuf, image: PathBuf },
    /// None resets to the default icon.
    SetPinIcon(PathBuf, Option<String>),
    ToggleSidebar,
    SetView(ViewMode),
    Sort(SortKey),
    ToggleSortReverse,
    ToggleDirsFirst,
    /// Opens a nested menu next to the current one.
    Submenu(SubMenu),
    GoTo,
    SelectAll,
    Help,
    Quit,
    Nothing,
    /// New file from a template (None = empty).
    NewFile(Option<PathBuf>),
    OpenTemplates,
    EmptyTrashAsk,
    EmptyTrashNow,
    /// Items dropped with the mouse: moved or copied (conflicts asked).
    DropTransfer { sources: Vec<Location>, dest: Dest, mv: bool },
    /// Symbolic links to these files in a folder.
    DropLink { sources: Vec<PathBuf>, dir: PathBuf },
    /// The archive under the cursor, browsed as a folder (whatever ✕ does).
    BrowseArchive,
    /// A program (.exe, .desktop) as a non-Steam game.
    SteamAdd(PathBuf),
    /// The shortcut written: with Steam restarted, or for its next start.
    SteamAddNow { s: crate::steam_add::Shortcut, restart: bool },
    /// A command in a terminal window of its own (installing, a script).
    RunInWindow { title: String, dir: PathBuf, cmd: String },
    /// Run a program or launcher with these files.
    OpenWith { app: PathBuf, files: Vec<PathBuf> },
    /// Stop every running operation.
    CancelJobs,
    /// Sidebar drive commands (index into the sidebar's items).
    DriveMount(usize),
    DriveUnmount(usize),
    DriveEject(usize),
    /// The disk usage page; with a folder, measured right away.
    UsageOpen(Option<PathBuf>),
    UsageReveal,
    UsageZoomIn,
    UsageZoomOut,
    UsageMode(u8),
    UsageRescan,
    UsageStart,
    UsageClose,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubMenu {
    View,
    Sort,
    /// A folder of templates (index into `App::templates`).
    Templates(usize),
    TermHistory,
    TermSaved,
    TermLaunch,
    TermTabs,
    /// A command: saved (index into the saved list) or from the history.
    TermEntry(bool, usize),
}

pub struct MenuItem {
    pub label: String,
    pub cmd: Cmd,
    pub enabled: bool,
    pub checked: Option<bool>,
    /// Toggles stay in the menu so several can be flipped in a row.
    pub keep_open: bool,
}

pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
    pub cursor: usize,
    /// Which submenu this is, to rebuild it after a toggle.
    pub kind: Option<SubMenu>,
    /// Open nested menu, shown to the right.
    pub sub: Option<Box<Menu>>,
    /// The item last scrolled into view (long menus scroll).
    pub shown: Option<usize>,
}

impl Menu {
    pub fn new(title: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self { title: title.into(), items, cursor: 0, kind: None, sub: None, shown: None }
    }

    pub fn depth(&self) -> usize {
        self.sub.as_ref().map_or(0, |s| 1 + s.depth())
    }

    pub fn at_depth_mut(&mut self, d: usize) -> &mut Menu {
        if d == 0 || self.sub.is_none() {
            return self;
        }
        self.sub.as_mut().unwrap().at_depth_mut(d - 1)
    }

    fn first_enabled(mut self) -> Self {
        self.cursor = self.items.iter().position(|i| i.enabled).unwrap_or(0);
        self
    }
    fn step(&mut self, d: isize) {
        let n = self.items.len() as isize;
        if n == 0 {
            return;
        }
        let mut i = self.cursor as isize;
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if self.items[i as usize].enabled {
                self.cursor = i as usize;
                return;
            }
        }
    }
}

pub(crate) fn item(label: &str, cmd: Cmd) -> MenuItem {
    MenuItem { label: label.to_string(), cmd, enabled: true, checked: None, keep_open: false }
}

/// A checkbox / radio entry that keeps its menu open.
pub(crate) fn toggle(label: &str, cmd: Cmd, on: bool) -> MenuItem {
    MenuItem { checked: Some(on), keep_open: true, ..item(label, cmd) }
}

pub enum OskPurpose {
    Rename { old: PathBuf },
    RenameInArchive { archive: PathBuf, from: String },
    NewFolder { dest: Dest },
    NewFile { dest: Dest, template: Option<PathBuf> },
    GoTo,
    /// Live quick filter; `prev` is restored on cancel.
    Filter { prev: Option<String> },
    Search { root: PathBuf },
    Password { archive: PathBuf, retry: Retry },
    /// Text typed into the viewer's document / its search.
    ViewerInsert,
    ViewerFind,
    ViewerReplace,
    /// A text field of the archive dialog, which comes back afterwards.
    CompressField { dlg: Box<CompressDlg>, field: CField },
    /// Keys straight into the terminal (nothing kept here).
    TermLive,
    /// The sudo password for an operation that needs it.
    Sudo { op: Box<Op> },
    /// A command to keep among the saved ones.
    TermSave,
}

/// What to do again once an archive password is entered.
pub enum Retry {
    Op(Op),
    Activate,
    Reload(PathBuf),
}

pub struct Osk {
    pub title: String,
    pub text: Vec<char>,
    pub caret: usize,
    pub row: usize,
    pub col: usize,
    pub layout: usize,
    pub shift: bool,
    pub purpose: OskPurpose,
    /// Draw the text as dots (passwords).
    pub secret: bool,
    /// Caps Lock, and Ctrl / Alt for the next key.
    pub caps: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// The character typed just before (it may combine with the next).
    pub prev: Option<char>,
}

pub use crate::keyboard::OskKey;

impl Osk {
    pub(crate) fn new(title: &str, text: &str, purpose: OskPurpose) -> Self {
        let text: Vec<char> = text.chars().collect();
        // Put the caret before the extension when renaming files.
        let caret = match &purpose {
            OskPurpose::Rename { old } if !old.is_dir() => {
                let s: String = text.iter().collect();
                ops::split_ext(&s).0.chars().count()
            }
            OskPurpose::RenameInArchive { .. } | OskPurpose::NewFile { .. } => {
                let s: String = text.iter().collect();
                ops::split_ext(&s).0.chars().count()
            }
            _ => text.len(),
        };
        // On "q" (or its layout's first letter).
        Self {
            title: title.into(),
            text,
            caret,
            row: 1,
            col: 1,
            layout: crate::keyboard::last_layout(),
            shift: false,
            purpose,
            secret: false,
            caps: false,
            ctrl: false,
            alt: false,
            prev: None,
        }
    }

    /// Typing straight into the terminal, nothing kept here.
    pub fn live(&self) -> bool {
        matches!(self.purpose, OskPurpose::TermLive)
    }

    pub fn row_len(&self, row: usize) -> usize {
        crate::keyboard::row(row).len()
    }

    pub fn key_at(&self, row: usize, col: usize) -> OskKey {
        match crate::keyboard::row(row).get(col).map(|(s, _)| *s) {
            Some(crate::keyboard::Slot::Ch(i)) => OskKey::Char(crate::keyboard::char_at(self.layout, row, i, self.shift, self.caps).unwrap_or(' ')),
            Some(crate::keyboard::Slot::Key(k)) => k,
            None => OskKey::Ok,
        }
    }

    fn nav(&mut self, a: Action) {
        let rows = crate::keyboard::ROWS;
        match a {
            Action::Up | Action::Down => {
                // The key under the same spot, as on a real keyboard.
                let x = crate::keyboard::center(self.row, self.col);
                self.row = if a == Action::Up { (self.row + rows - 1) % rows } else { (self.row + 1) % rows };
                self.col = crate::keyboard::nearest(self.row, x);
            }
            Action::Left => self.col = (self.col + self.row_len(self.row) - 1) % self.row_len(self.row),
            Action::Right => self.col = (self.col + 1) % self.row_len(self.row),
            _ => {}
        }
    }

    pub fn insert(&mut self, s: &str) {
        for c in s.chars() {
            self.text.insert(self.caret, c);
            self.caret += 1;
        }
    }

    pub fn backspace(&mut self) {
        if self.caret > 0 {
            self.caret -= 1;
            self.text.remove(self.caret);
        }
    }

    pub fn delete(&mut self) {
        if self.caret < self.text.len() {
            self.text.remove(self.caret);
        }
    }

    pub fn caret_by(&mut self, d: isize) {
        self.caret = (self.caret as isize + d).clamp(0, self.text.len() as isize) as usize;
    }

    pub fn string(&self) -> String {
        self.text.iter().collect()
    }
}

pub struct Props {
    pub rows: Vec<(String, String)>,
    pub icon: Option<String>,
}

pub enum Overlay {
    Menu(Menu),
    Osk(Osk),
    Props(Props),
    Help,
    Themes(ThemePicker),
    Settings(Settings),
    IconPicker(IconPicker),
    Compress(Box<CompressDlg>),
    ViewerSettings(ViewerSettings),
    TermKeys(crate::terminal::app::QuickKeys),
    TermSettings(crate::terminal::app::TermSettings),
}

/// Text viewer settings, shown beside the open file.
pub struct ViewerSettings {
    pub row: usize,
}

pub mod vrow {
    pub const THEME: usize = 0;
    pub const FONT: usize = 1;
    pub const SIZE: usize = 2;
    pub const WRAP: usize = 3;
    pub const NUMBERS: usize = 4;
    pub const CLOSE: usize = 5;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CField {
    Name,
    Split,
    Password,
    /// Second entry of a new password.
    Confirm,
}

/// Archive settings before compressing.
pub struct CompressDlg {
    pub sources: Vec<PathBuf>,
    pub dir: PathBuf,
    /// Without the extension.
    pub name: String,
    pub opts: CompressOpts,
    pub row: usize,
    /// First entry of the password while it's being confirmed.
    pub pending_pw: Option<String>,
}

pub mod crow {
    pub const NAME: usize = 0;
    pub const FORMAT: usize = 1;
    pub const METHOD: usize = 2;
    pub const LEVEL: usize = 3;
    pub const SPLIT: usize = 4;
    pub const PASSWORD: usize = 5;
    pub const NAMES: usize = 6;
    pub const CREATE: usize = 7;
    pub const CANCEL: usize = 8;
}

/// Volume size presets (MiB); 4095 fits FAT32.
pub const SPLIT_PRESETS: [u64; 6] = [10, 100, 700, 1024, 2048, 4095];
const MIB: u64 = 1024 * 1024;

pub fn split_label(split: Option<u64>) -> String {
    match split {
        None => tr("Don't split").to_string(),
        Some(b) if b == 4095 * MIB => format!("4 {} (FAT32)", tr("GB")),
        Some(b) if b % (1024 * MIB) == 0 => format!("{} {}", b / (1024 * MIB), tr("GB")),
        Some(b) => format!("{} {}", b / MIB, tr("MB")),
    }
}

impl CompressDlg {
    /// Rows that apply to the chosen format.
    pub fn enabled(&self, row: usize) -> bool {
        let f = self.opts.format;
        match row {
            crow::METHOD => !f.methods().is_empty(),
            crow::PASSWORD => f.encrypts(),
            crow::NAMES => f == ArchiveFormat::SevenZ && self.opts.password.is_some(),
            _ => true,
        }
    }

    pub fn formats() -> Vec<ArchiveFormat> {
        ArchiveFormat::ALL.into_iter().filter(|f| *f != ArchiveFormat::SevenZ || ops::have_7z()).collect()
    }

    fn move_row(&mut self, d: isize) {
        let mut r = self.row as isize;
        loop {
            r += d;
            if !(0..=crow::CANCEL as isize).contains(&r) {
                return;
            }
            if self.enabled(r as usize) {
                self.row = r as usize;
                return;
            }
        }
    }

    /// ←/→ on a row.
    pub fn step(&mut self, row: usize, d: isize) {
        fn cycle<T: PartialEq + Copy>(all: &[T], cur: T, d: isize) -> T {
            let i = all.iter().position(|x| *x == cur).unwrap_or(0) as isize;
            all[(i + d).rem_euclid(all.len() as isize) as usize]
        }
        let o = &mut self.opts;
        match row {
            crow::FORMAT => {
                o.format = cycle(&Self::formats(), o.format, d);
                o.fit();
            }
            crow::METHOD if !o.format.methods().is_empty() => o.method = cycle(o.format.methods(), o.method, d),
            crow::LEVEL => o.level = cycle(o.format.levels(), o.level, d),
            crow::SPLIT => {
                let mut all: Vec<Option<u64>> = vec![None];
                all.extend(SPLIT_PRESETS.iter().map(|m| Some(m * MIB)));
                o.split = match o.split.filter(|s| !all.contains(&Some(*s))) {
                    // A custom size steps to the neighbouring preset.
                    Some(s) => {
                        let above = all.iter().position(|x| x.is_some_and(|x| x > s)).unwrap_or(all.len());
                        if d > 0 { all.get(above).copied().unwrap_or(None) } else { all[above - 1] }
                    }
                    None => cycle(&all, o.split, d),
                };
            }
            crow::NAMES => o.encrypt_names = !o.encrypt_names,
            crow::CREATE | crow::CANCEL => self.row = if self.row == crow::CREATE { crow::CANCEL } else { crow::CREATE },
            _ => {}
        }
    }
}

/// Theme icon chooser for a pinned folder. Groups are colour variants
/// (L1/R1), the grid holds that group's icons.
pub struct IconPicker {
    pub path: PathBuf,
    pub groups: Vec<crate::icons::IconGroup>,
    pub group: usize,
    pub cursor: usize,
    /// Grid columns (fixed, so gamepad ↑↓ math matches the drawing).
    pub cols: usize,
    pub scrolled: Option<(usize, usize)>,
    /// Scroll offset from the last frame.
    pub scroll_y: f32,
}

impl IconPicker {
    pub const COLS: usize = 8;

    pub fn names(&self) -> &[String] {
        &self.groups[self.group].names
    }

    /// Scroll target once per cursor/group change.
    pub fn take_scroll(&mut self) -> Option<usize> {
        let now = (self.group, self.cursor);
        (self.scrolled != Some(now)).then(|| {
            self.scrolled = Some(now);
            self.cursor
        })
    }
}

pub const SETTINGS_ROWS: usize = 15;
pub mod row {
    pub const ACCENT: usize = 0;
    pub const APPEARANCE: usize = 1;
    pub const LANGUAGE: usize = 2;
    pub const SIDE_ICON: usize = 3;
    pub const LIST_ICON: usize = 4;
    pub const GRID_ICON: usize = 5;
    pub const ZOOM: usize = 6;
    pub const THEME: usize = 7;
    pub const OPEN_IN: usize = 8;
    pub const VOLUMES: usize = 9;
    pub const WEB_IMAGES: usize = 10;
    pub const PASSWORDS: usize = 11;
    pub const LAYOUTS: usize = 12;
    pub const INSTALL: usize = 13;
    pub const CLOSE: usize = 14;
}

/// "Open in Polka" kinds, in the settings' order.
pub const OPEN_KINDS: usize = 4;

impl Config {
    pub fn open_flag(&mut self, i: usize) -> &mut bool {
        match i {
            0 => &mut self.open_text,
            1 => &mut self.open_images,
            2 => &mut self.open_music,
            _ => &mut self.open_videos,
        }
    }
}

/// Settings dialog. Changes apply (and save) immediately.
pub struct Settings {
    pub row: usize,
    /// Cursor among the "open in Polka" kinds.
    pub chip: usize,
    /// Cursor among the keyboard layouts.
    pub layout_chip: usize,
    /// Cursor in the colour row: presets, then saved colours, then "+".
    pub swatch: usize,
    pub editor: Option<ColorEditor>,
}

/// HSV colour mixer for a new saved colour.
pub struct ColorEditor {
    pub hsv: [f32; 3],
    /// 0..=2 sliders, 3 = save, 4 = cancel
    pub row: usize,
}

impl ColorEditor {
    pub fn rgb(&self) -> [u8; 3] {
        let c: egui::Color32 = egui::ecolor::Hsva::new(self.hsv[0], self.hsv[1], self.hsv[2], 1.0).into();
        [c.r(), c.g(), c.b()]
    }

    fn adjust(&mut self, d: f32) {
        if let Some(v) = self.hsv.get_mut(self.row) {
            *v = if self.row == 0 { (*v + d * 0.02).rem_euclid(1.0) } else { (*v + d * 0.04).clamp(0.0, 1.0) };
        }
    }
}

/// The swatches of the colour row, in order.
pub fn swatches(cfg: &Config) -> Vec<Option<[u8; 3]>> {
    let mut v: Vec<Option<[u8; 3]>> = crate::config::ACCENT_PRESETS.to_vec();
    v.extend(cfg.custom_colors.iter().map(|c| Some(*c)));
    v
}

/// Push the highlight colour into egui's styles (dark and light).
pub fn apply_accent(ctx: &egui::Context, cfg: &Config) {
    let [r, g, b] = cfg.accent_rgb();
    let c = egui::Color32::from_rgb(r, g, b);
    ctx.all_styles_mut(|s| {
        s.visuals.selection.bg_fill = c;
        s.visuals.selection.stroke.color = egui::Color32::WHITE;
        s.visuals.hyperlink_color = c;
    });
}

/// Icon theme chooser. Row 0 is "system default" (id None).
pub struct ThemePicker {
    pub items: Vec<(Option<String>, String)>,
    pub cursor: usize,
    /// Theme the preview shows for each row (system default resolved).
    pub preview: Vec<String>,
    /// Cursor the list was last scrolled to.
    pub scrolled: Option<usize>,
}

impl ThemePicker {
    /// The cursor row, once per cursor change, for scrolling it into view.
    pub fn take_scroll(&mut self) -> Option<usize> {
        (self.scrolled != Some(self.cursor)).then(|| {
            self.scrolled = Some(self.cursor);
            self.cursor
        })
    }
}

// ---------------------------------------------------------------- sidebar

#[derive(Clone)]
pub struct SideItem {
    pub label: String,
    pub icon: &'static [&'static str],
    /// User-chosen theme icon (pinned folders), tried first.
    pub custom_icon: Option<String>,
    pub loc: Location,
    pub pinned: bool,
    pub section: &'static str,
    /// Opens the disk usage page instead of a place.
    pub usage: bool,
    /// Opens the terminal page.
    pub term: bool,
    /// A drive (maybe not mounted: then `loc` is no real place).
    pub drive: Option<crate::drives::Drive>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Panes,
}

/// Picture picking for a pinned folder's icon (browsing the panes).
pub struct IconPick {
    pub folder: PathBuf,
    pub tab: usize,
    pub pane: usize,
    pub back: Location,
}

pub struct RangeSel {
    pub anchor: usize,
    /// Select (true) or deselect the range.
    pub on: bool,
    /// Selection before the range started.
    pub base: HashSet<String>,
    /// Started by Shift: ends when Shift is released.
    pub keyboard: bool,
}

pub struct Clip {
    pub items: Vec<Location>,
    pub cut: bool,
}

pub enum Msg {
    Toast(String, bool),
    /// A drive operation finished.
    Drive(Result<crate::drives::Done, String>),
    /// The trash was emptied: (items, failures).
    TrashEmptied(usize, usize),
    NeedPassword(PathBuf, bool, Retry),
    /// A file from an archive was extracted for the viewer.
    View { tab: usize, path: PathBuf, name: String, origin: usize, source: Option<(PathBuf, String)> },
    /// "Show in folder" from another program (D-Bus), with its activation
    /// token if it gave one.
    Show(crate::fm1::Show, String),
}

// ---------------------------------------------------------------- app

pub struct App {
    pub cfg: Config,
    pub tabs: Vec<Tab>,
    pub tab: usize,
    pub focus: Focus,
    pub side_cursor: usize,
    pub side_items: Vec<SideItem>,
    side_refreshed: Instant,
    pub overlay: Option<Overlay>,
    pub icons: Icons,
    pub pad_rx: Receiver<Input>,
    pub pad: PadKind,
    pub pad_seen: bool,
    /// Range selection while Select (or Shift on the keyboard) is held.
    pub range: Option<RangeSel>,
    pub icon_pick: Option<IconPick>,
    pub clip: Option<Clip>,
    pub jobs: Vec<Arc<Job>>,
    pub toast: Option<(String, bool, Instant)>,
    pub msg_tx: Sender<Msg>,
    /// Started for a D-Bus request: its folder takes the first tab.
    pub dbus_fresh: bool,
    msg_rx: Receiver<Msg>,
    pub ctx: egui::Context,
    pub quit: bool,
    /// Full screen asked of the window (for a video).
    pub full_sent: bool,
    /// x of the line between the two panes (last frame), to split the
    /// address bar at the same place.
    pub divider_x: Option<f32>,
    /// □ held while editing text: Some(moved since).
    pub edit_hold: Option<bool>,
    /// Text copied in the viewer (when the system clipboard can't be read).
    pub text_clip: Option<String>,
    /// Shell history behind the terminal's history menu.
    pub term_hist: Vec<String>,
    /// Top of the pad's keyboard while it types into the terminal.
    pub osk_top: Option<f32>,
    /// A terminal window running one command (installing, a script).
    pub term_window: Option<Box<crate::terminal::Terminal>>,
    pub term_window_title: String,
    /// The sudo password that last worked, and since when.
    pub sudo_pw: Option<(crate::ops::Secret, Instant)>,
    /// The system clipboard, read straight from the display connection
    /// (gamepad paste has no Ctrl+V event to carry it). None in tests.
    pub sys_clip: Option<egui_winit::clipboard::Clipboard>,
    /// Items being dragged with the mouse.
    pub drag: Option<crate::mouse::Drag>,
    /// Drag and drop with other programs (Wayland), and what it feeds egui.
    pub dnd: Option<crate::dnd::Dnd>,
    pub dnd_inject: Vec<egui::Event>,
    pub dnd_pos: Option<egui::Pos2>,
    /// Our drag went out to the system / one came in from it / was dropped here.
    pub dnd_out: bool,
    pub dnd_in: bool,
    pub dnd_dropped: bool,
    /// A drop's release is on its way to egui (keep the drag till then).
    pub dnd_release: Option<Instant>,
    /// Light/dark last set on the context.
    pub dark_applied: Option<bool>,
    /// Template folders for the "New document" submenus.
    pub templates: Vec<PathBuf>,
}

impl App {
    pub fn new(ctx: egui::Context, start: Option<PathBuf>) -> Self {
        let cfg = Config::load();
        crate::i18n::set(cfg.language.as_deref());
        crate::keyboard::set_enabled(&cfg.layouts());
        let (pad_tx, pad_rx) = crossbeam_channel::unbounded();
        if std::env::var_os("POLKA_NO_GAMEPAD").is_none() {
            let c = ctx.clone();
            crate::input::spawn_gamepad(pad_tx.clone(), move || c.request_repaint());
            // Dropping the last sender would close the channel.
            std::mem::forget(pad_tx);
        } else {
            // Keep the sender alive so the channel never reports disconnection.
            std::mem::forget(pad_tx);
        }
        let icon_ctx = ctx.clone();
        let icons = Icons::new(cfg.icon_theme.clone(), move || icon_ctx.request_repaint());
        let home = dirs::home_dir().unwrap_or_else(|| "/".into());
        let (start, focus) = start.and_then(|p| start_location(&p)).unwrap_or((Location::Local(home.clone()), None));
        let mut first = Pane::new(start, &cfg);
        if let Some(name) = focus {
            // Archives list in the background; the name is applied when ready.
            first.pending_focus = Some(name.clone());
            if let Some(i) = first.entries.iter().position(|e| e.name == name) {
                first.set_cursor(i);
            }
        }
        let tab = Tab { panes: [first, Pane::new(Location::Local(home), &cfg)], active: 0, viewer: None, usage: None, term: None };
        let (msg_tx, msg_rx) = crossbeam_channel::unbounded();
        ctx.set_zoom_factor(cfg.zoom);
        apply_accent(&ctx, &cfg);
        crate::fonts::install_with(&ctx, cfg.viewer.font.as_deref());
        crate::locks::set_waker(ctx.clone());
        crate::packages::set_waker(ctx.clone());
        let mut app = Self {
            cfg,
            tabs: vec![tab],
            tab: 0,
            focus: Focus::Panes,
            side_cursor: 0,
            side_items: Vec::new(),
            side_refreshed: Instant::now(),
            overlay: None,
            icons,
            pad_rx,
            pad: PadKind::default(),
            pad_seen: false,
            range: None,
            icon_pick: None,
            clip: None,
            jobs: Vec::new(),
            toast: None,
            msg_tx,
            dbus_fresh: false,
            msg_rx,
            ctx,
            quit: false,
            full_sent: false,
            divider_x: None,
            edit_hold: None,
            text_clip: None,
            term_hist: Vec::new(),
            osk_top: None,
            term_window: None,
            term_window_title: String::new(),
            sudo_pw: None,
            sys_clip: None,
            dark_applied: None,
            dnd: None,
            dnd_inject: Vec::new(),
            dnd_pos: None,
            dnd_out: false,
            dnd_in: false,
            dnd_dropped: false,
            dnd_release: None,
            drag: None,
            templates: Vec::new(),
        };
        app.refresh_sidebar();
        app
    }

    pub fn cur_tab(&self) -> &Tab {
        &self.tabs[self.tab]
    }
    pub fn pane(&self) -> &Pane {
        let t = self.cur_tab();
        &t.panes[if self.cfg.dual { t.active } else { 0 }]
    }
    pub fn pane_mut(&mut self) -> &mut Pane {
        let dual = self.cfg.dual;
        let t = &mut self.tabs[self.tab];
        &mut t.panes[if dual { t.active } else { 0 }]
    }
    /// (active pane, other pane) of the current tab.
    fn pane_pair(&mut self) -> (&mut Pane, &mut Pane) {
        let t = &mut self.tabs[self.tab];
        let [a, b] = &mut t.panes;
        if t.active == 0 { (a, b) } else { (b, a) }
    }

    /// Light or dark as chosen (or as the desktop is, which may change).
    pub fn apply_appearance(&mut self) {
        let dark = crate::appearance::is_dark(self.cfg.appearance);
        if self.dark_applied != Some(dark) {
            self.dark_applied = Some(dark);
            self.ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
            self.icons.set_dark(dark);
            apply_accent(&self.ctx, &self.cfg);
        }
    }

    pub fn toast(&mut self, s: impl Into<String>, err: bool) {
        self.toast = Some((s.into(), err, Instant::now()));
    }

    // ------------------------------------------------------------ per frame

    pub fn tick(&mut self) {
        self.apply_appearance();
        self.tick_audio();
        self.tick_usage();
        self.tick_terminal();
        self.dnd_tick();
        let cfg = self.cfg.clone();
        for t in &mut self.tabs {
            for p in &mut t.panes {
                p.poll(&cfg);
            }
        }
        while let Ok(m) = self.msg_rx.try_recv() {
            match m {
                Msg::Toast(s, e) => self.toast(s, e),
                Msg::Drive(r) => self.drive_done(r),
                Msg::TrashEmptied(n, failed) => {
                    self.refresh_sidebar();
                    self.reload_all();
                    if failed > 0 {
                        self.toast(trn("Some items stayed in the trash: %n", failed), true);
                    } else {
                        self.toast(trn("Trash emptied: %n", n), false);
                    }
                }
                Msg::NeedPassword(a, wrong, retry) => self.ask_password(a, wrong, retry),
                Msg::Show(what, token) => self.show_from_dbus(what, &token),
                Msg::View { tab, path, name, origin, source } => {
                    if tab < self.tabs.len() {
                        self.show_viewer(tab, path, name, origin, Vec::new());
                        // Saving goes back into the archive, if it can be written.
                        let writable = self.tabs[tab].panes[origin].loc.dest().is_some();
                        if let Some(v) = self.tabs[tab].viewer.as_mut() {
                            v.source = source;
                            if let crate::viewer::Content::Text(t) = &mut v.content {
                                t.read_only = !writable;
                            }
                        }
                    }
                }
            }
        }
        // Leaving the folder (sidebar, address bar…) closes the viewer.
        for t in &mut self.tabs {
            if let Some(v) = &t.viewer {
                if v.folder.as_ref().is_some_and(|f| *f != t.panes[v.origin].loc) {
                    t.viewer = None;
                }
            }
        }
        // The filter keyboard filters live.
        if let Some(Overlay::Osk(k)) = &self.overlay {
            if let OskPurpose::Filter { .. } = k.purpose {
                let f = k.string();
                self.pane_mut().set_filter(Some(f));
            }
        }
        // Finished jobs: refresh what they touched, report errors.
        let mut touched: Vec<Location> = Vec::new();
        let mut report = Vec::new();
        let mut ask = Vec::new();
        let mut ended = Vec::new();
        self.jobs.retain(|j| {
            let fin = *j.finished_at.lock().unwrap();
            match fin {
                None => true,
                Some(_) => {
                    touched.extend(j.touches.iter().cloned());
                    if let (Some((a, wrong)), Some(op)) = (j.need_password.lock().unwrap().take(), j.op.clone()) {
                        ask.push((a, wrong, op));
                    } else if let (Some(r), Some(op)) = (j.result.lock().unwrap().clone(), j.op.clone()) {
                        ended.push((op, r, j.denied.load(std::sync::atomic::Ordering::Relaxed), j.label.clone()));
                    }
                    false
                }
            }
        });
        // Not allowed: again as administrator; else errors are reported.
        for (op, r, denied, label) in ended {
            if !self.elevated_done(&op, &r, denied) {
                if let Err(e) = r {
                    report.push(format!("{label}: {e}"));
                }
            }
        }
        for r in report {
            self.toast(r, true);
        }
        for (a, wrong, op) in ask {
            self.ask_password(a, wrong, Retry::Op(op));
        }
        // A listing that needs a password (7z with encrypted names).
        let need: Vec<(PathBuf, bool)> = self.tabs[self.tab].panes.iter_mut().filter_map(|p| p.need_password.take()).collect();
        for (a, wrong) in need {
            self.ask_password(a.clone(), wrong, Retry::Reload(a));
        }
        if !touched.is_empty() {
            for t in &mut self.tabs {
                for p in &mut t.panes {
                    if touched.iter().any(|t| p.loc.affected_by(t)) {
                        p.reload(&cfg);
                    }
                }
            }
        }
        if self.side_refreshed.elapsed() > Duration::from_secs(3) {
            self.refresh_sidebar();
        }
        if let Some((_, _, at)) = &self.toast {
            if at.elapsed() > Duration::from_secs(4) {
                self.toast = None;
            }
        }
    }

    pub fn wake(&self) -> impl Fn() + Send + Sync + 'static {
        let c = self.ctx.clone();
        move || c.request_repaint()
    }

    /// "Show in folder": each folder in a tab (the current one when it's
    /// already there), its asked-for files selected, the cursor on the
    /// first; the window raised.
    pub fn show_from_dbus(&mut self, what: crate::fm1::Show, token: &str) {
        use crate::fm1::{Show, group};
        let (groups, props) = match what {
            Show::Folders(v) => (v.into_iter().map(|p| if p.is_dir() { vec![(p, Vec::new())] } else { group(&[p]) }).flatten().collect::<Vec<_>>(), false),
            Show::Items(v) => (group(&v), false),
            Show::Properties(v) => (group(&v), true),
        };
        let cfg = self.cfg.clone();
        for (dir, names) in groups {
            let loc = Location::Local(dir);
            if self.dbus_fresh {
                self.dbus_fresh = false;
                self.tab = 0;
                self.tabs[0].active = 0;
                self.pane_mut().navigate(loc, None, &cfg);
            } else if !(self.pane().loc == loc && self.cur_tab().viewer.is_none() && !self.usage_shown() && !self.term_shown()) {
                self.run(Cmd::OpenInNewTab(loc));
            }
            let pane = self.pane_mut();
            pane.selected.clear();
            if let Some(first) = names.first() {
                // A folder still listing gets the cursor when it's ready.
                pane.pending_focus = Some(first.clone());
                if let Some(i) = pane.entries.iter().position(|e| &e.name == first) {
                    pane.set_cursor(i);
                }
            }
            if names.len() > 1 {
                pane.selected.extend(names);
            }
        }
        self.overlay = None;
        self.focus = Focus::Panes;
        if props {
            self.open_props();
        }
        // The caller's token raises the window where the compositor allows.
        let raised = !token.is_empty() && self.dnd.as_ref().is_some_and(|d| d.activate(token.to_string()));
        if !raised {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    pub fn refresh_sidebar(&mut self) {
        self.side_refreshed = Instant::now();
        let mut v = Vec::new();
        let icon_for = |cfg: &Config, key: &str| cfg.pinned_icons.get(key).cloned();
        let place = |label: String, icon: &'static [&'static str], p: PathBuf, section: &'static str, cfg: &Config| SideItem {
            custom_icon: icon_for(cfg, &p.to_string_lossy()),
            label,
            icon,
            loc: Location::Local(p),
            pinned: false,
            section,
            usage: false,
            term: false,
            drive: None,
        };
        let home = dirs::home_dir().unwrap_or_else(|| "/".into());
        let places: [(&str, Option<PathBuf>, &'static [&'static str]); 7] = [
            ("Home", Some(home.clone()), &["user-home", "folder-home", "folder"]),
            ("Desktop", dirs::desktop_dir(), &["user-desktop", "folder-desktop", "folder"]),
            ("Documents", dirs::document_dir(), &["folder-documents", "folder"]),
            ("Downloads", dirs::download_dir(), &["folder-download", "folder-downloads", "folder"]),
            ("Music", dirs::audio_dir(), &["folder-music", "folder"]),
            ("Pictures", dirs::picture_dir(), &["folder-pictures", "folder"]),
            ("Videos", dirs::video_dir(), &["folder-videos", "folder"]),
        ];
        for (label, p, icon) in places {
            if let Some(p) = p.filter(|p| p.is_dir() && (label == "Home" || *p != home)) {
                v.push(place(tr(label).into(), icon, p, "Places", &self.cfg));
            }
        }
        v.push(place(tr("File system").into(), &["drive-harddisk-root", "drive-harddisk", "folder-root", "folder"], crate::sandbox::root(), "Places", &self.cfg));
        if let Some(t) = trash_files().filter(|t| t.is_dir()) {
            // A full bin looks full.
            let icon: &'static [&'static str] = if crate::trashbin::count() > 0 { &["user-trash-full", "user-trash"] } else { &["user-trash", "user-trash-full"] };
            v.push(place(tr("Trash").into(), icon, t, "Places", &self.cfg));
        }
        v.push(SideItem {
            label: tr("Terminal").into(),
            icon: &["utilities-terminal", "terminal", "org.gnome.Terminal", "konsole", "xterm"],
            custom_icon: icon_for(&self.cfg, "polka:terminal"),
            loc: Location::Local("/".into()),
            pinned: false,
            section: "Places",
            usage: false,
            term: true,
            drive: None,
        });
        if let Some(root) = crate::steam::root() {
            v.push(SideItem { custom_icon: icon_for(&self.cfg, "polka:steam"), ..place("Steam".into(), &["steam", "com.valvesoftware.Steam", "applications-games"], PathBuf::new(), "Places", &self.cfg) });
            if let Some(last) = v.last_mut() {
                last.loc = Location::Steam(crate::steam::View::Root);
                let _ = root;
            }
        }
        for p in &self.cfg.pinned {
            let label = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
            v.push(SideItem { pinned: true, ..place(label, &["folder-bookmark", "bookmark-new", "folder"], p.clone(), "Pinned", &self.cfg) });
        }
        // Drives, mounted or not; then other things mounted for the user
        // (network shares, FUSE) that aren't block devices.
        let drives = crate::drives::list();
        for d in &drives {
            let label = d.label.clone().unwrap_or_else(|| format!("{} {}", tr("Drive"), human_size(d.size)));
            let loc = Location::Local(d.mount.clone().unwrap_or_else(|| PathBuf::from(d.key())));
            v.push(SideItem { label, icon: d.icon(), custom_icon: icon_for(&self.cfg, &d.key()), loc, pinned: false, section: "Drives", usage: false, term: false, drive: Some(d.clone()) });
        }
        for (dev, mnt) in mounts() {
            if drives.iter().any(|d| d.mount.as_ref() == Some(&mnt)) {
                continue;
            }
            let label = mnt.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(dev);
            v.push(place(label, &["drive-removable-media", "drive-harddisk", "media-flash"], mnt, "Drives", &self.cfg));
        }
        v.push(SideItem {
            label: tr("Disk usage").into(),
            icon: &["baobab", "org.gnome.baobab", "filelight", "disk-usage-analyzer", "drive-harddisk"],
            custom_icon: None,
            loc: Location::Local("/".into()),
            pinned: false,
            section: "Tools",
            usage: true,
            term: false,
            drive: None,
        });
        self.side_items = v;
        self.side_cursor = self.side_cursor.min(self.side_items.len().saturating_sub(1));
    }

    // ------------------------------------------------------------ input

    pub fn handle(&mut self, input: Input) {
        let a = match input {
            Input::Pad(k) => {
                self.pad = k;
                self.pad_seen = true;
                return;
            }
            Input::Release(Action::Select) => {
                self.range = None;
                if self.overlay.is_none() && self.video_shown() {
                    self.video_select_released();
                } else if self.overlay.is_none() && self.audio_shown() {
                    self.audio_select_released();
                } else if self.overlay.is_none() && self.cur_tab().viewer.is_some() {
                    self.viewer_select_released();
                } else if self.overlay.is_none() && (self.term_window.is_some() || (self.focus == Focus::Panes && self.term_shown())) {
                    self.terminal_release(Action::Select);
                }
                return;
            }
            Input::Release(a) => {
                if self.overlay.is_none() && (self.term_window.is_some() || (self.focus == Focus::Panes && self.term_shown())) {
                    self.terminal_release(a);
                }
                return;
            }
            Input::Press(a) => a,
        };
        if self.overlay.is_some() {
            self.handle_overlay(a);
            return;
        }
        // A terminal window has the pad to itself.
        if self.term_window.is_some() {
            return self.terminal_input(a);
        }
        if self.focus == Focus::Sidebar {
            self.handle_sidebar(a);
            return;
        }
        if self.cur_tab().viewer.is_some() && !matches!(a, Action::PrevTab | Action::NextTab | Action::LeftStick) {
            return self.viewer_input(a);
        }
        // A hidden cursor comes back first instead of acting unseen.
        if self.pane().cursor_hidden && !self.usage_shown() && !self.term_shown() {
            self.pane_mut().cursor_hidden = false;
            if matches!(a, Action::Up | Action::Down | Action::Left | Action::Right | Action::Accept | Action::Select | Action::Actions | Action::Home | Action::End) {
                return;
            }
        }
        if self.usage_shown() && !matches!(a, Action::PrevTab | Action::NextTab | Action::LeftStick) {
            return self.usage_input(a);
        }
        if self.term_shown() && !matches!(a, Action::PrevTab | Action::NextTab | Action::LeftStick) {
            return self.terminal_input(a);
        }
        let cfg = self.cfg.clone();
        let grid = cfg.view == ViewMode::Grid;
        let cols = if grid { self.pane().cols.max(1) as isize } else { 1 };
        // Choosing a picture: ✕ on an image picks it, △ cancels.
        if self.icon_pick.is_some() {
            match a {
                Action::Actions => return self.finish_icon_pick(None),
                Action::Accept => {
                    let p = self.pane();
                    if let Some(e) = p.current().filter(|e| !e.is_dir) {
                        match p.entry_path(e).filter(|_| is_icon_image(&e.name)) {
                            Some(img) => return self.finish_icon_pick(Some(img)),
                            None => return self.toast(tr("Choose a picture (png, svg, jpg…)"), true),
                        }
                    }
                }
                Action::Select => return,
                _ => {}
            }
        }
        match a {
            Action::Up => self.move_and_paint(-cols),
            Action::Down => self.move_and_paint(cols),
            // While a range is being selected, nothing leaves this pane.
            Action::Left => {
                let p = self.pane();
                if grid && p.cursor as isize % cols != 0 {
                    self.move_and_paint(-1);
                } else if self.range.is_none() {
                    self.focus_left();
                }
            }
            Action::Right => {
                let p = self.pane();
                let last_col = (p.cursor as isize + 1) % cols == 0 || p.cursor + 1 >= p.entries.len();
                if grid && !last_col {
                    self.move_and_paint(1);
                } else if cfg.dual && self.cur_tab().active == 0 && self.range.is_none() {
                    self.tabs[self.tab].active = 1;
                }
            }
            Action::PrevTab | Action::NextTab | Action::View if self.range.is_some() => {}
            Action::LeftTrigger | Action::RightTrigger if cfg.dual && self.range.is_some() => {}
            Action::Home => {
                self.pane_mut().set_cursor(0);
                self.paint_range();
            }
            Action::End => {
                let n = self.pane().entries.len();
                self.pane_mut().set_cursor(n.saturating_sub(1));
                self.paint_range();
            }
            Action::Accept => self.activate(),
            Action::Back => {
                if !self.pane().selected.is_empty() {
                    self.pane_mut().selected.clear();
                } else if self.pane().filter.is_some() {
                    self.pane_mut().set_filter(None);
                } else {
                    self.pane_mut().go_up(&cfg);
                }
            }
            Action::Select => self.begin_range(false),
            Action::Actions => self.open_actions_menu(),
            Action::Menu => self.open_app_menu(),
            Action::View => self.run(Cmd::ToggleDual),
            Action::PrevTab => self.switch_tab(-1),
            Action::NextTab => self.switch_tab(1),
            Action::LeftTrigger | Action::RightTrigger => {
                let right = a == Action::RightTrigger;
                if cfg.dual {
                    self.tabs[self.tab].active = right as usize;
                } else {
                    let rows = self.pane().page_rows.max(1) as isize * cols;
                    self.move_and_paint(if right { rows } else { -rows });
                }
            }
            Action::LeftStick => self.run(Cmd::ToggleSidebar),
            Action::HistoryBack => self.pane_mut().history(false, &cfg),
            Action::HistoryForward => self.pane_mut().history(true, &cfg),
        }
    }

    fn focus_left(&mut self) {
        if self.cfg.dual && self.cur_tab().active == 1 {
            self.tabs[self.tab].active = 0;
        } else {
            self.focus_sidebar();
        }
    }

    pub(crate) fn focus_sidebar(&mut self) {
        self.focus = Focus::Sidebar;
        // Land on the sidebar entry for the current folder if there is one.
        let loc = self.pane().loc.clone();
        if let Some(i) = self.side_items.iter().position(|s| s.loc == loc) {
            self.side_cursor = i;
        }
    }

    /// Move the cursor; while Select is held, paint the selection along.
    fn move_and_paint(&mut self, d: isize) {
        self.pane_mut().move_cursor(d);
        self.paint_range();
    }

    /// Start a range at the cursor. The anchor item flips (select or
    /// deselect); the whole range then takes that state.
    pub fn begin_range(&mut self, keyboard: bool) {
        let p = self.pane();
        let Some(name) = p.current().map(|e| e.name.clone()) else { return };
        self.range = Some(RangeSel {
            anchor: p.cursor,
            // Shift always selects; □ flips the anchor item's state.
            on: keyboard || !p.selected.contains(&name),
            base: p.selected.clone(),
            keyboard,
        });
        self.paint_range();
    }

    /// Selection = what was selected before the range + anchor..=cursor.
    /// Moving back towards the anchor gives items back their old state.
    fn paint_range(&mut self) {
        let Some(r) = &self.range else { return };
        let (anchor, on, base) = (r.anchor, r.on, r.base.clone());
        let p = self.pane_mut();
        let (lo, hi) = (anchor.min(p.cursor), anchor.max(p.cursor).min(p.entries.len().saturating_sub(1)));
        let names: Vec<String> = p.entries.get(lo..=hi).unwrap_or(&[]).iter().map(|e| e.name.clone()).collect();
        p.selected = base;
        for n in names {
            p.mark(&n, on);
        }
    }

    fn switch_tab(&mut self, d: isize) {
        let n = self.tabs.len() as isize;
        self.tab = (self.tab as isize + d).rem_euclid(n) as usize;
    }

    fn handle_sidebar(&mut self, a: Action) {
        let n = self.side_items.len();
        match a {
            Action::Up => self.side_cursor = self.side_cursor.saturating_sub(1),
            Action::Down => self.side_cursor = (self.side_cursor + 1).min(n.saturating_sub(1)),
            Action::Home => self.side_cursor = 0,
            Action::End => self.side_cursor = n.saturating_sub(1),
            Action::Right | Action::Back => self.focus = Focus::Panes,
            Action::Accept => {
                if let Some(it) = self.side_items.get(self.side_cursor).cloned() {
                    // The sidebar keeps the focus: the place shows, and → or
                    // ○ goes over to it.
                    if it.usage {
                        self.open_usage(None);
                        self.focus = Focus::Sidebar;
                        return;
                    }
                    if it.term {
                        self.open_terminal(None);
                        self.focus = Focus::Sidebar;
                        return;
                    }
                    // A drive not mounted yet: mount it, then open it.
                    if let Some(d) = it.drive.as_ref().filter(|d| d.mount.is_none()) {
                        return self.drive_op(d.clone(), crate::drives::Op::Mount);
                    }
                    let cfg = self.cfg.clone();
                    // A place replaces the disk usage page (kept for later).
                    if let Some(u) = self.tabs[self.tab].usage.as_deref_mut() {
                        u.shown = false;
                    }
                    if let Some(t) = self.tabs[self.tab].term.as_deref_mut() {
                        t.shown = false;
                    }
                    self.pane_mut().navigate(it.loc, None, &cfg);
                }
            }
            Action::Actions => {
                if let Some(it) = self.side_items.get(self.side_cursor).cloned().filter(|it| !it.usage) {
                    self.side_menu(it);
                }
            }
            Action::Menu => self.open_app_menu(),
            Action::LeftStick => self.run(Cmd::ToggleSidebar),
            Action::PrevTab => self.switch_tab(-1),
            Action::NextTab => self.switch_tab(1),
            _ => {}
        }
    }

    fn handle_overlay(&mut self, a: Action) {
        let Some(ov) = self.overlay.as_mut() else { return };
        match ov {
            Overlay::Menu(root) => {
                let depth = root.depth();
                let m = root.at_depth_mut(depth);
                match a {
                    Action::Up => m.step(-1),
                    Action::Down => m.step(1),
                    Action::Accept => self.menu_accept(depth),
                    // Right only opens submenus; it never runs a command.
                    Action::Right => {
                        if matches!(m.items.get(m.cursor).map(|i| &i.cmd), Some(Cmd::Submenu(_))) {
                            self.menu_accept(depth);
                        }
                    }
                    Action::Left | Action::Back if depth > 0 => root.at_depth_mut(depth - 1).sub = None,
                    Action::Back | Action::Menu | Action::Actions => self.overlay = None,
                    _ => {}
                }
            }
            Overlay::Osk(k) => match a {
                Action::Up | Action::Down | Action::Left | Action::Right => k.nav(a),
                Action::Accept => {
                    let key = k.key_at(k.row, k.col);
                    self.osk_key(key);
                }
                Action::Back => self.osk_cancel(),
                Action::Select => self.osk_key(OskKey::Backspace),
                Action::Actions => self.osk_key(OskKey::Space),
                Action::PrevTab => self.osk_key(OskKey::Left),
                Action::NextTab => self.osk_key(OskKey::Right),
                Action::LeftTrigger => self.osk_key(OskKey::Shift),
                Action::View => self.osk_key(OskKey::Lang),
                Action::Menu | Action::RightTrigger => self.osk_key(OskKey::Ok),
                _ => {}
            },
            Overlay::Themes(t) => match a {
                Action::Up => t.cursor = t.cursor.saturating_sub(1),
                Action::Down => t.cursor = (t.cursor + 1).min(t.items.len().saturating_sub(1)),
                Action::LeftTrigger => t.cursor = t.cursor.saturating_sub(8),
                Action::RightTrigger => t.cursor = (t.cursor + 8).min(t.items.len().saturating_sub(1)),
                Action::Accept => {
                    let id = t.items[t.cursor].0.clone();
                    self.overlay = None;
                    self.run(Cmd::SetIconTheme(id));
                }
                Action::Back | Action::Menu => self.overlay = None,
                _ => {}
            },
            Overlay::Settings(_) => self.settings_input(a),
            Overlay::ViewerSettings(_) => self.viewer_settings_input(a),
            Overlay::TermKeys(_) => self.term_keys_input(a),
            Overlay::TermSettings(_) => self.term_settings_input(a),
            Overlay::Compress(d) => match a {
                Action::Up => d.move_row(-1),
                Action::Down => d.move_row(1),
                Action::Left | Action::Right => {
                    let row = d.row;
                    d.step(row, if a == Action::Left { -1 } else { 1 });
                }
                Action::Accept => {
                    let row = d.row;
                    self.compress_activate(row);
                }
                Action::Menu => self.compress_create(),
                Action::Back => self.overlay = None,
                _ => {}
            },
            Overlay::IconPicker(p) => {
                let n = p.names().len();
                let cols = p.cols as isize;
                let mv = |p: &mut IconPicker, d: isize| {
                    p.cursor = (p.cursor as isize + d).clamp(0, n.saturating_sub(1) as isize) as usize;
                };
                match a {
                    Action::Left => mv(p, -1),
                    Action::Right => mv(p, 1),
                    Action::Up => mv(p, -cols),
                    Action::Down => mv(p, cols),
                    Action::LeftTrigger => mv(p, -cols * 4),
                    Action::RightTrigger => mv(p, cols * 4),
                    Action::PrevTab | Action::NextTab => {
                        let g = p.groups.len() as isize;
                        let d = if a == Action::NextTab { 1 } else { -1 };
                        p.group = (p.group as isize + d).rem_euclid(g) as usize;
                        p.cursor = 0;
                    }
                    Action::Accept => {
                        let icon = p.names().get(p.cursor).cloned();
                        let path = p.path.clone();
                        self.overlay = None;
                        if icon.is_some() {
                            self.run(Cmd::SetPinIcon(path, icon));
                        }
                    }
                    Action::Select => {
                        let path = p.path.clone();
                        self.overlay = None;
                        self.run(Cmd::SetPinIcon(path, None));
                    }
                    Action::Actions => {
                        let path = p.path.clone();
                        self.run(Cmd::PickIconFile(path));
                    }
                    Action::Back | Action::Menu => self.overlay = None,
                    _ => {}
                }
            }
            Overlay::Props(_) | Overlay::Help => {
                if matches!(a, Action::Back | Action::Accept | Action::Menu) {
                    self.overlay = None;
                }
            }
        }
    }

    pub fn osk_key(&mut self, key: OskKey) {
        use crate::terminal::keys::{K, Mods};
        let Some(Overlay::Osk(k)) = self.overlay.as_mut() else { return };
        // The character typed (Space too), and what it makes with the one
        // typed just before (an accent, a Hangul syllable, a voiced kana).
        let typed = match key {
            OskKey::Char(c) => Some(c),
            OskKey::Space => Some(' '),
            _ => None,
        };
        let composed = typed.and_then(|c| k.prev.and_then(|p| crate::keyboard::compose(k.layout, p, c)));
        match key {
            OskKey::Shift => k.shift = !k.shift,
            OskKey::Caps => k.caps = !k.caps,
            OskKey::Ctrl => k.ctrl = !k.ctrl,
            OskKey::Alt => k.alt = !k.alt,
            OskKey::Lang => {
                k.layout = crate::keyboard::next_layout(k.layout);
                crate::keyboard::set_last_layout(k.layout);
                k.prev = None;
            }
            // Into the terminal as it's pressed.
            _ if k.live() => {
                let mods = Mods { ctrl: k.ctrl, alt: k.alt, shift: false };
                let plain = !k.ctrl && !k.alt;
                if typed.is_some() {
                    (k.shift, k.ctrl, k.alt) = (false, false, false);
                }
                let t = match key {
                    OskKey::Char(c) => Some(K::Char(c)),
                    OskKey::Space => Some(K::Char(' ')),
                    OskKey::Backspace => Some(K::Backspace),
                    OskKey::Tab => Some(K::Tab),
                    OskKey::Enter => Some(K::Enter),
                    OskKey::Left => Some(K::Left),
                    OskKey::Right => Some(K::Right),
                    OskKey::Up => Some(K::Up),
                    OskKey::Down => Some(K::Down),
                    _ => None,
                };
                match (composed.filter(|_| plain), t) {
                    // The one before is replaced by what they make together.
                    (Some(r), _) => {
                        k.prev = r.chars().last();
                        self.term_key(K::Backspace, Mods::NONE);
                        for ch in r.chars() {
                            self.term_key(K::Char(ch), Mods::NONE);
                        }
                    }
                    (None, Some(t)) => {
                        k.prev = typed.filter(|_| plain);
                        self.term_key(t, mods);
                    }
                    (None, None) => self.overlay = None,
                }
            }
            OskKey::Char(_) | OskKey::Space => {
                let c = typed.unwrap_or(' ');
                let before = k.caret.checked_sub(1).and_then(|i| k.text.get(i)).copied();
                match composed.filter(|_| before == k.prev) {
                    Some(r) => {
                        k.backspace();
                        k.insert(&r);
                        k.prev = r.chars().last();
                    }
                    None => {
                        k.insert(&c.to_string());
                        k.prev = Some(c);
                    }
                }
                k.shift = false;
            }
            OskKey::Backspace => {
                k.backspace();
                k.prev = None;
            }
            OskKey::Tab => {}
            OskKey::Left => k.caret_by(-1),
            OskKey::Right => k.caret_by(1),
            OskKey::Up => k.caret = 0,
            OskKey::Down => k.caret = k.text.len(),
            OskKey::Enter | OskKey::Ok => self.osk_submit(),
        }
        // Moving about ends a syllable being built.
        if matches!(key, OskKey::Left | OskKey::Right | OskKey::Up | OskKey::Down | OskKey::Tab | OskKey::Enter) {
            if let Some(Overlay::Osk(k)) = self.overlay.as_mut() {
                k.prev = None;
            }
        }
    }

    pub fn osk_cancel(&mut self) {
        if let Some(Overlay::Osk(k)) = self.overlay.take() {
            match k.purpose {
                OskPurpose::Filter { prev } => self.pane_mut().set_filter(prev),
                OskPurpose::CompressField { mut dlg, .. } => {
                    dlg.pending_pw = None;
                    self.overlay = Some(Overlay::Compress(dlg));
                }
                _ => {}
            }
        }
    }

    pub fn osk_submit(&mut self) {
        let Some(Overlay::Osk(k)) = self.overlay.take() else { return };
        // Passwords keep their spaces; everything else is trimmed.
        let raw = k.string();
        let text = raw.trim().to_string();
        if let OskPurpose::Filter { .. } = k.purpose {
            self.pane_mut().set_filter(Some(text));
            return;
        }
        if let OskPurpose::CompressField { dlg, field } = k.purpose {
            return self.compress_field(*dlg, field, raw);
        }
        if let OskPurpose::Sudo { op } = k.purpose {
            return self.sudo_typed(*op, raw);
        }
        if matches!(k.purpose, OskPurpose::ViewerInsert | OskPurpose::ViewerFind | OskPurpose::ViewerReplace) {
            return self.viewer_osk(k.purpose, raw);
        }
        if matches!(k.purpose, OskPurpose::TermLive | OskPurpose::TermSave) {
            return self.term_osk(k.purpose, raw);
        }
        if text.is_empty() {
            return;
        }
        let cfg = self.cfg.clone();
        let bad_name = text.contains('/') || text == "." || text == "..";
        match k.purpose {
            OskPurpose::Rename { old } => {
                if bad_name {
                    return self.toast(format!("{}: {text}", tr("Failed")), true);
                }
                let new = old.with_file_name(&text);
                if new == old {
                    return;
                }
                if new.symlink_metadata().is_ok() {
                    return self.toast(format!("{}: {} exists", tr("Failed"), text), true);
                }
                match std::fs::rename(&old, &new) {
                    Ok(()) => {
                        let p = self.pane_mut();
                        p.selected.clear();
                        p.reload(&cfg);
                        if let Some(i) = p.entries.iter().position(|e| e.name == text) {
                            p.set_cursor(i);
                        }
                    }
                    Err(e) if crate::ops::denied(&e) => {
                        self.pending_focus(text);
                        let dir = old.parent().unwrap_or(Path::new("/")).to_path_buf();
                        self.elevate_one(&["mv".as_ref(), "-n".as_ref(), "--".as_ref(), old.as_os_str(), new.as_os_str()], &dir, tr("Rename"));
                    }
                    Err(e) => self.toast(format!("{}: {e}", tr("Failed")), true),
                }
            }
            OskPurpose::RenameInArchive { archive, from } => {
                if bad_name {
                    return self.toast(format!("{}: {text}", tr("Failed")), true);
                }
                let parent = from.rsplit_once('/').map_or("", |(p, _)| p);
                let to = if parent.is_empty() { text.clone() } else { format!("{parent}/{text}") };
                if to == from {
                    return;
                }
                if self.pane().all.iter().any(|e| e.name == text) {
                    return self.toast(format!("{}: {} exists", tr("Failed"), text), true);
                }
                let plan = crate::archive_rw::Plan { rename: vec![(from, to)], ..Default::default() };
                self.pane_mut().selected.clear();
                self.pending_focus(text);
                self.start(Op::ArchiveEdit { archive, plan, label: tr("Rename") });
            }
            OskPurpose::NewFolder { dest: Dest::Archive { archive, inner } } => {
                if bad_name {
                    return self.toast(format!("{}: {text}", tr("Failed")), true);
                }
                if self.pane().all.iter().any(|e| e.name == text) {
                    return self.toast(format!("{}: {} exists", tr("Failed"), text), true);
                }
                let path = if inner.is_empty() { text.clone() } else { format!("{inner}/{text}") };
                let plan = crate::archive_rw::Plan { mkdir: vec![path], ..Default::default() };
                self.pending_focus(text);
                self.start(Op::ArchiveEdit { archive, plan, label: tr("New folder") });
            }
            OskPurpose::Password { archive, retry } => {
                vfs::set_password(&archive, &raw);
                vfs::invalidate(&archive);
                match retry {
                    Retry::Op(op) => self.start(op),
                    Retry::Activate => self.activate(),
                    Retry::Reload(a) => {
                        for t in &mut self.tabs {
                            for p in &mut t.panes {
                                if p.loc.archive() == Some(a.as_path()) {
                                    p.reload(&cfg);
                                }
                            }
                        }
                    }
                }
            }
            OskPurpose::Search { root } => {
                let loc = Location::Search { root, query: text, hidden: cfg.show_hidden };
                self.pane_mut().navigate(loc, None, &cfg);
            }
            OskPurpose::Filter { .. } => {}
            OskPurpose::NewFile { dest, template } => {
                if bad_name {
                    return self.toast(format!("{}: {text}", tr("Failed")), true);
                }
                if self.pane().all.iter().any(|e| e.name == text) {
                    return self.toast(format!("{}: {} {}", tr("Failed"), text, tr("already exists")), true);
                }
                let write = |to: &Path| -> std::io::Result<()> {
                    match &template {
                        Some(t) => std::fs::copy(t, to).map(|_| ()),
                        None => std::fs::File::create_new(to).map(|_| ()),
                    }
                };
                match dest {
                    Dest::Dir(dir) => match write(&dir.join(&text)) {
                        Ok(()) => {
                            let p = self.pane_mut();
                            p.reload(&cfg);
                            if let Some(i) = p.entries.iter().position(|e| e.name == text) {
                                p.set_cursor(i);
                            }
                        }
                        Err(e) if crate::ops::denied(&e) => {
                            self.pending_focus(text.clone());
                            let to = dir.join(&text);
                            match &template {
                                Some(t) => self.elevate_one(&["cp".as_ref(), "--".as_ref(), t.as_os_str(), to.as_os_str()], &dir, tr("New document")),
                                None => self.elevate_one(&["touch".as_ref(), "--".as_ref(), to.as_os_str()], &dir, tr("New document")),
                            }
                        }
                        Err(e) => self.toast(format!("{}: {e}", tr("Failed")), true),
                    },
                    Dest::Archive { archive, inner } => {
                        // Made on disk under its final name, then added.
                        let tmp = vfs::temp_root().join(format!("new-{}", std::process::id()));
                        let _ = std::fs::remove_dir_all(&tmp);
                        let made = std::fs::create_dir_all(&tmp).and_then(|_| write(&tmp.join(&text)));
                        if let Err(e) = made {
                            return self.toast(format!("{}: {e}", tr("Failed")), true);
                        }
                        let path = if inner.is_empty() { text.clone() } else { format!("{inner}/{text}") };
                        let plan = crate::archive_rw::Plan { add: vec![(tmp.join(&text), path)], ..Default::default() };
                        self.pending_focus(text);
                        self.start(Op::ArchiveEdit { archive, plan, label: tr("New document") });
                    }
                }
            }
            OskPurpose::NewFolder { dest: Dest::Dir(dir) } => {
                if bad_name {
                    return self.toast(format!("{}: {text}", tr("Failed")), true);
                }
                match std::fs::create_dir(dir.join(&text)) {
                    Ok(()) => {
                        let p = self.pane_mut();
                        p.reload(&cfg);
                        if let Some(i) = p.entries.iter().position(|e| e.name == text) {
                            p.set_cursor(i);
                        }
                    }
                    Err(e) if crate::ops::denied(&e) => {
                        self.pending_focus(text.clone());
                        let to = dir.join(&text);
                        self.elevate_one(&["mkdir".as_ref(), "--".as_ref(), to.as_os_str()], &dir, tr("New folder"));
                    }
                    Err(e) => self.toast(format!("{}: {e}", tr("Failed")), true),
                }
            }
            OskPurpose::GoTo => {
                let path = expand_tilde(&text);
                if path.is_dir() {
                    self.pane_mut().navigate(Location::Local(path), None, &cfg);
                } else if path.is_file() && vfs::is_archive_name(&text) {
                    self.pane_mut().navigate(vfs::archive_loc(path), None, &cfg);
                } else {
                    self.toast(format!("{}: {text}", tr("Failed")), true);
                }
            }
            OskPurpose::CompressField { .. } | OskPurpose::ViewerInsert | OskPurpose::ViewerFind | OskPurpose::ViewerReplace | OskPurpose::TermLive | OskPurpose::TermSave | OskPurpose::Sudo { .. } => unreachable!(),
        }
    }

    // ------------------------------------------------------------ actions

    pub fn activate(&mut self) {
        let cfg = self.cfg.clone();
        let p = self.pane();
        let Some(e) = p.current().cloned() else { return };
        let loc = p.entry_loc(&e);
        if e.is_dir {
            self.pane_mut().navigate(loc, None, &cfg);
            return;
        }
        match &p.loc {
            Location::Steam(_) => {}
            Location::Local(_) | Location::Search { .. } => {
                let path = p.entry_path(&e).expect("local entry");
                // A package (checked by its content) installs on ✕, as set.
                let package = e.mtime.and_then(|t| crate::packages::package(&path, &e.name, t));
                if let (true, Some(k)) = (self.cfg.install_on_open, package) {
                    if let Some((cmd, tool)) = crate::packages::command(k, &path) {
                        let title = format!("{} ({tool})", tr("Install package"));
                        let dir = path.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| "/".into());
                        return self.open_term_window(title, &dir, &cmd);
                    }
                }
                if vfs::is_archive_name(&e.name) {
                    self.pane_mut().navigate(vfs::archive_loc(path), None, &cfg);
                } else if crate::viewer::kind_in(&path, &e.name, crate::viewer::OpenIn::from(&self.cfg)).is_some() {
                    self.view_current();
                } else if let Err(err) = crate::launch::open(&path) {
                    self.toast(format!("{}: {err}", tr("Failed")), true);
                }
            }
            Location::Archive { archive, inner } => {
                let inner_path = if inner.is_empty() { e.name.clone() } else { format!("{inner}/{}", e.name) };
                if vfs::is_archive_name(&e.name) {
                    // Nested archive: extract it, then browse the copy.
                    match vfs::extract_to_temp(archive, &inner_path) {
                        Ok(tmp) => {
                            let origin = p.loc.clone();
                            let pm = self.pane_mut();
                            pm.nest.push((tmp.clone(), origin, e.name.clone()));
                            pm.navigate(Location::Archive { archive: tmp, inner: String::new() }, None, &cfg);
                        }
                        Err(err) => match vfs::need_password(&err) {
                            Some(n) => self.ask_password(n.archive.clone(), n.wrong, Retry::Activate),
                            None => self.toast(format!("{}: {err}", tr("Failed")), true),
                        },
                    }
                } else {
                    let (archive, tx) = (archive.clone(), self.msg_tx.clone());
                    let wake = self.wake();
                    let (tab, origin, name) = (self.tab, self.cur_tab().active, e.name.clone());
                    let open_in = crate::viewer::OpenIn::from(&self.cfg);
                    std::thread::spawn(move || {
                        let msg = match vfs::extract_to_temp(&archive, &inner_path) {
                            Ok(p) if crate::viewer::kind_in(&p, &name, open_in).is_some() => {
                                let source = Some((archive.clone(), inner_path.clone()));
                                Some(Msg::View { tab, path: p, name, origin, source })
                            }
                            Ok(p) => crate::launch::open(&p).err().map(|e| Msg::Toast(format!("{}: {e}", tr("Failed")), true)),
                            Err(e) => Some(match vfs::need_password(&e) {
                                Some(n) => Msg::NeedPassword(n.archive.clone(), n.wrong, Retry::Activate),
                                None => Msg::Toast(format!("{}: {e}", tr("Failed")), true),
                            }),
                        };
                        if let Some(m) = msg {
                            let _ = tx.send(m);
                            wake();
                        }
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------ viewer

    /// Open the pane's current file in this tab's viewer.
    pub fn view_current(&mut self) {
        let p = self.pane();
        let Some(e) = p.current().filter(|e| !e.is_dir).cloned() else { return };
        let Some(path) = p.entry_path(&e) else { return self.activate() };
        // Pictures of the folder, in the pane's order, for ←/→.
        // Music: the folder's tracks become the playlist.
        let same: Option<fn(&Path, &str) -> bool> = if crate::viewer::image::is_picture(&path, &e.name) {
            Some(crate::viewer::image::is_picture)
        } else if crate::viewer::audio::is_audio(&e.name) {
            Some(|_, n| crate::viewer::audio::is_audio(n))
        } else if crate::viewer::video::is_video(&e.name) {
            Some(|_, n| crate::viewer::video::is_video(n))
        } else {
            None
        };
        let siblings: Vec<PathBuf> = if let Some(same) = same {
            p.entries
                .iter()
                .filter(|x| !x.is_dir)
                .filter_map(|x| p.entry_path(x).filter(|xp| same(xp, &x.name)))
                .collect()
        } else {
            Vec::new()
        };
        let (tab, origin) = (self.tab, self.cur_tab().active);
        self.show_viewer(tab, path, e.name, origin, siblings);
    }

    pub fn show_viewer(&mut self, tab: usize, path: PathBuf, name: String, origin: usize, siblings: Vec<PathBuf>) {
        let mut v = Viewer::open(path, name, origin, siblings, self.cfg.viewer.volume, &self.ctx);
        v.folder = Some(self.tabs[tab].panes[origin].loc.clone());
        self.tabs[tab].viewer = Some(Box::new(v));
        self.range = None;
    }

    pub fn close_viewer(&mut self) {
        let t = &mut self.tabs[self.tab];
        if let Some(v) = t.viewer.take() {
            // Back on the file that was shown last.
            let p = &mut t.panes[v.origin];
            if let Some(i) = p.entries.iter().position(|e| p.loc.entry_path(&e.name).as_deref() == Some(v.path.as_path())) {
                p.set_cursor(i);
            }
        }
    }

    fn viewer_input(&mut self, a: Action) {
        if self.viewer_text_input(a) || self.viewer_audio_input(a) || self.viewer_video_input(a) {
            return;
        }
        let ctx = self.ctx.clone();
        let Some(v) = self.tabs[self.tab].viewer.as_mut() else { return };
        match v.input(a, &ctx) {
            crate::viewer::Out::Nothing => {}
            crate::viewer::Out::Close => self.request_close_viewer(),
            crate::viewer::Out::Settings => self.run(Cmd::ViewerSettings),
            crate::viewer::Out::Actions => self.open_actions_menu(),
            crate::viewer::Out::Moved(path) => {
                let t = &mut self.tabs[self.tab];
                let origin = t.viewer.as_ref().map_or(0, |v| v.origin);
                let p = &mut t.panes[origin];
                if let Some(i) = p.entries.iter().position(|e| p.loc.entry_path(&e.name).as_deref() == Some(path.as_path())) {
                    p.set_cursor(i);
                }
            }
        }
    }

    fn viewer_settings_input(&mut self, a: Action) {
        let Some(Overlay::ViewerSettings(st)) = self.overlay.as_mut() else { return };
        match a {
            Action::Up => st.row = st.row.saturating_sub(1),
            Action::Down => st.row = (st.row + 1).min(vrow::CLOSE),
            Action::Left | Action::Right => {
                let row = st.row;
                self.viewer_step(row, if a == Action::Left { -1 } else { 1 });
            }
            Action::Accept => {
                let row = st.row;
                self.viewer_step(row, 0);
            }
            Action::Back | Action::Menu => self.overlay = None,
            _ => {}
        }
    }

    /// ←/→ (d = ±1) or ✕ (d = 0) on a viewer settings row.
    pub fn viewer_step(&mut self, row: usize, d: isize) {
        if let Some(Overlay::ViewerSettings(st)) = self.overlay.as_mut() {
            st.row = row;
        }
        let v = &mut self.cfg.viewer;
        let cycle = |n: usize, i: usize, d: isize| (i as isize + if d == 0 { 1 } else { d }).rem_euclid(n as isize) as usize;
        match row {
            vrow::THEME => {
                let names: Vec<&str> = crate::viewer::theme::presets().iter().map(|t| t.name).collect();
                let i = names.iter().position(|n| *n == v.theme).unwrap_or(0);
                v.theme = names[cycle(names.len(), i, d)].to_string();
            }
            vrow::FONT => {
                // None (the system monospace) first, then installed families.
                let mut all: Vec<Option<String>> = vec![None];
                all.extend(crate::fonts::mono_families().iter().map(|(f, _)| Some(f.clone())));
                let i = all.iter().position(|f| *f == v.font).unwrap_or(0);
                v.font = all[cycle(all.len(), i, d)].clone();
                crate::fonts::install_with(&self.ctx, v.font.as_deref());
            }
            vrow::SIZE => {
                let (lo, hi, st) = crate::config::VIEWER_FONT;
                v.font_size = (v.font_size + d as f32 * st).clamp(lo, hi);
            }
            vrow::WRAP => v.wrap = !v.wrap,
            vrow::NUMBERS => v.line_numbers = !v.line_numbers,
            vrow::CLOSE => {
                self.overlay = None;
                return;
            }
            _ => return,
        }
        self.cfg.save();
    }

    fn open_actions_menu(&mut self) {
        self.scan_templates();
        if self.usage_shown() {
            return self.usage_menu();
        }
        if self.cur_tab().viewer.as_ref().is_some_and(|v| v.is_text()) {
            return self.viewer_text_menu();
        }
        if self.audio_shown() {
            return self.viewer_audio_menu();
        }
        if self.video_shown() {
            return self.viewer_video_menu();
        }
        if let Some(v) = &self.cur_tab().viewer {
            let mut items = vec![item(tr("Open in another app"), Cmd::OpenExternal)];
            if v.is_text() {
                items.push(toggle(tr("Wrap lines"), Cmd::ViewerWrap, self.cfg.viewer.wrap));
                items.push(toggle(tr("Line numbers"), Cmd::ViewerNumbers, self.cfg.viewer.line_numbers));
                items.push(item(tr("Viewer settings…"), Cmd::ViewerSettings));
            }
            items.push(item(tr("Close"), Cmd::CloseViewer));
            self.overlay = Some(Overlay::Menu(Menu::new(&v.name, items)));
            return;
        }
        let p = self.pane();
        let targets = p.targets();
        // Real files (a folder or search results) vs. a writable archive.
        let local = p.loc.entry_path("").is_some();
        let writable = p.loc.dest().is_some();
        let editable = local || writable;
        let single = targets.len() == 1;
        let any = !targets.is_empty();
        let dual = self.cfg.dual;
        let other_writable = self.cur_tab().panes[1 - self.cur_tab().active].loc.dest().is_some();
        let other_local = self.cur_tab().panes[1 - self.cur_tab().active].loc.local_path().is_some();
        let one_archive = single && !targets[0].is_dir && vfs::is_archive_name(&targets[0].name);
        let one_dir_local = single && targets[0].is_dir && local;
        let pinned = one_dir_local
            && p.entry_path(&targets[0]).is_some_and(|pp| self.cfg.pinned.contains(&pp));

        let mut items = Vec::new();
        let mut add = |label: &str, cmd: Cmd, enabled: bool| {
            items.push(MenuItem { enabled, ..item(label, cmd) })
        };
        // A package that ✕ installs still opens as a folder from here.
        let package = targets.first().filter(|e| single && !e.is_dir).and_then(|e| Some(crate::packages::package(&p.entry_path(e)?, &e.name, e.mtime?)?));
        if one_archive && package.is_some() && self.cfg.install_on_open {
            add(tr("Open as folder"), Cmd::BrowseArchive, true);
        } else {
            add(if one_archive { tr("Open as folder") } else { tr("Open") }, Cmd::Open, single);
        }
        if let Some(path) = targets.first().filter(|e| single && !e.is_dir).and_then(|e| p.entry_path(e)) {
            let name = targets[0].name.clone();
            if crate::viewer::kind_in(&path, &name, crate::viewer::OpenIn::from(&self.cfg)).is_some() {
                add(tr("Open in another app"), Cmd::OpenExternal, true);
            } else if crate::viewer::kind(&path, &name).is_some() {
                add(tr("Open in Polka"), Cmd::OpenInViewer, true);
            } else if crate::launch::runs(&path) && crate::viewer::text::sniff(&path) {
                add(tr("View as text"), Cmd::OpenInViewer, true);
            }
            // A Windows program or a launcher into the Steam library (when
            // there's a Steam).
            let lower = name.to_lowercase();
            if [".exe", ".bat", ".msi", ".desktop"].iter().any(|x| lower.ends_with(x)) && crate::steam::root().is_some() {
                add(tr("Add to Steam"), Cmd::SteamAdd(path.clone()), true);
            }
            // A package (as its content shows) for this system's tool, or a
            // script to run.
            let k = package.or_else(|| crate::packages::kind(&name).filter(|k| *k == crate::packages::Kind::Script));
            if let Some((cmd, tool)) = k.and_then(|k| crate::packages::command(k, &path)) {
                let label = if tool == "sh" { tr("Run in terminal").to_string() } else { format!("{} ({tool})", tr("Install package")) };
                let dir = path.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| "/".into());
                add(&label, Cmd::RunInWindow { title: label.clone(), dir, cmd }, true);
            }
        }
        add(tr("Copy"), Cmd::Copy, any);
        add(tr("Cut"), Cmd::Cut, any && editable);
        add(tr("Paste"), Cmd::Paste, self.clip.is_some() && writable);
        add(tr("New folder"), Cmd::NewFolder, writable);
        add(tr("New document"), Cmd::Submenu(SubMenu::Templates(0)), writable);
        if dual {
            add(tr("Copy to other pane"), Cmd::CopyOther, any && other_writable);
            add(tr("Move to other pane"), Cmd::MoveOther, any && editable && other_writable);
        }
        add(tr("Rename"), Cmd::Rename, single && editable);
        if local {
            add(tr("Move to trash"), Cmd::TrashAsk, any);
        }
        add(tr("Delete permanently"), Cmd::DeleteAsk, any && editable);
        add(tr("Compress…"), Cmd::CompressAsk, any && local);
        if p.loc.local_path().is_some() && p.loc.local_path() == trash_files().as_deref() {
            add(tr("Empty trash…"), Cmd::EmptyTrashAsk, !p.entries.is_empty());
        }
        if one_archive {
            add(tr("Extract here"), Cmd::ExtractHere, local);
            if dual {
                add(tr("Extract to other pane"), Cmd::ExtractOther, local && other_local);
            }
        }
        if one_dir_local {
            let path = p.entry_path(&targets[0]).unwrap();
            add(tr("Disk usage"), Cmd::UsageOpen(Some(path.clone())), true);
            add(tr("Open in terminal"), Cmd::TermOpen(Some(path.clone())), true);
            if pinned {
                add(tr("Unpin from sidebar"), Cmd::Unpin(path), true);
            } else {
                add(tr("Pin to sidebar"), Cmd::Pin(path), true);
            }
        }
        add(tr("Properties"), Cmd::Properties, true);

        let title = if targets.len() > 1 {
            format!("{} {}", targets.len(), tr("selected"))
        } else {
            targets.first().map(|e| e.name.clone()).unwrap_or_else(|| p.loc.title())
        };
        self.overlay = Some(Overlay::Menu(Menu::new(title, items).first_enabled()));
    }

    /// Template folders, the top one first (for the submenus).
    fn scan_templates(&mut self) {
        fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
            out.push(dir.to_path_buf());
            if depth >= 3 {
                return;
            }
            let mut subs: Vec<PathBuf> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
                .map(|e| e.path())
                .collect();
            subs.sort();
            for d in subs {
                walk(&d, depth + 1, out);
            }
        }
        self.templates.clear();
        if let Some(t) = dirs::template_dir() {
            walk(&t, 0, &mut self.templates);
        }
    }

    fn templates_menu(&self, i: usize) -> Vec<MenuItem> {
        let mut items = Vec::new();
        if i == 0 {
            items.push(item(tr("Empty file"), Cmd::NewFile(None)));
        }
        let Some(dir) = self.templates.get(i) else {
            items.push(MenuItem { enabled: false, ..item(tr("No templates folder"), Cmd::Nothing) });
            return items;
        };
        let mut entries: Vec<(bool, String, PathBuf)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| (!e.path().is_dir(), e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect();
        entries.sort_by(|a, b| (a.0, a.1.to_lowercase()).cmp(&(b.0, b.1.to_lowercase())));
        for (file, name, path) in entries {
            if !file {
                if let Some(j) = self.templates.iter().position(|t| *t == path) {
                    items.push(item(&name, Cmd::Submenu(SubMenu::Templates(j))));
                }
            } else {
                // Shown without the extension, as in Nautilus.
                items.push(item(ops::split_ext(&name).0, Cmd::NewFile(Some(path))));
            }
        }
        if i == 0 {
            if items.len() == 1 {
                items.push(MenuItem { enabled: false, ..item(tr("Put files into Templates to see them here"), Cmd::Nothing) });
            }
            items.push(item(tr("Open templates folder"), Cmd::OpenTemplates));
        }
        items
    }

    pub(crate) fn open_app_menu(&mut self) {
        self.scan_templates();
        let local = self.pane().loc.local_path().is_some();
        let writable = self.pane().loc.dest().is_some();
        let mut items = vec![];
        // Running copies and the like can be stopped from the pad too.
        if self.jobs.iter().any(|j| j.result.lock().map(|r| r.is_none()).unwrap_or(false)) {
            items.push(item(tr("Cancel operations"), Cmd::CancelJobs));
        }
        items.extend([
            item(tr("New tab"), Cmd::NewTab),
            MenuItem { enabled: self.tabs.len() > 1, ..item(tr("Close tab"), Cmd::CloseTab) },
            MenuItem { enabled: writable, ..item(tr("New folder"), Cmd::NewFolder) },
            MenuItem { enabled: writable, ..item(tr("New document"), Cmd::Submenu(SubMenu::Templates(0))) },
            MenuItem { enabled: self.clip.is_some() && writable, ..item(tr("Paste"), Cmd::Paste) },
            item(tr("Filter…"), Cmd::Filter),
            MenuItem { enabled: local, ..item(tr("Search…"), Cmd::Search) },
            MenuItem { enabled: local, ..item(tr("Open terminal here"), Cmd::TermOpen(self.pane().loc.local_path().map(|p| p.to_path_buf()))) },
            item(tr("Go to path…"), Cmd::GoTo),
            item(tr("Select all"), Cmd::SelectAll),
            item(tr("View"), Cmd::Submenu(SubMenu::View)),
            item(tr("Sorting"), Cmd::Submenu(SubMenu::Sort)),
            item(tr("Settings…"), Cmd::Settings),
            item(tr("Controls"), Cmd::Help),
            item(tr("Quit"), Cmd::Quit),
        ]);
        self.overlay = Some(Overlay::Menu(Menu::new("Polka", items)));
    }

    fn build_submenu(&self, kind: SubMenu) -> Menu {
        let c = &self.cfg;
        let (title, items) = match kind {
            SubMenu::TermHistory | SubMenu::TermSaved | SubMenu::TermLaunch | SubMenu::TermTabs | SubMenu::TermEntry(..) => return self.term_submenu(kind),
            SubMenu::View => (
                tr("View").to_string(),
                vec![
                    toggle(tr("List"), Cmd::SetView(ViewMode::List), c.view == ViewMode::List),
                    toggle(tr("Grid"), Cmd::SetView(ViewMode::Grid), c.view == ViewMode::Grid),
                    toggle(tr("Dual pane"), Cmd::ToggleDual, c.dual),
                    toggle(tr("Sidebar"), Cmd::ToggleSidebar, !c.sidebar_collapsed),
                    toggle(tr("Show hidden files"), Cmd::ToggleHidden, c.show_hidden),
                    toggle(tr("Thumbnails"), Cmd::ToggleThumbnails, c.thumbnails),
                    item(tr("Icon theme…"), Cmd::IconThemes),
                ],
            ),
            SubMenu::Templates(i) => {
                let title = if i == 0 { tr("New document").to_string() } else { self.templates[i].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() };
                (title, self.templates_menu(i))
            }
            SubMenu::Sort => (
                tr("Sorting").to_string(),
                vec![
                    toggle(tr("By name"), Cmd::Sort(SortKey::Name), c.sort == SortKey::Name),
                    toggle(tr("By size"), Cmd::Sort(SortKey::Size), c.sort == SortKey::Size),
                    toggle(tr("By date"), Cmd::Sort(SortKey::Date), c.sort == SortKey::Date),
                    toggle(tr("By type"), Cmd::Sort(SortKey::Type), c.sort == SortKey::Type),
                    toggle(tr("Reverse order"), Cmd::ToggleSortReverse, c.sort_reverse),
                    toggle(tr("Folders first"), Cmd::ToggleDirsFirst, c.dirs_first),
                ],
            ),
        };
        Menu { kind: Some(kind), ..Menu::new(&title, items) }
    }

    /// Accept the cursor item of the menu at `depth`: open a submenu, run a
    /// toggle in place, or run a command and close.
    pub fn menu_accept(&mut self, depth: usize) {
        let Some(Overlay::Menu(root)) = self.overlay.as_mut() else { return };
        let m = root.at_depth_mut(depth);
        m.sub = None;
        let Some(it) = m.items.get(m.cursor).filter(|i| i.enabled) else { return };
        let (cmd, keep_open, kind) = (it.cmd.clone(), it.keep_open, m.kind);
        match cmd {
            Cmd::Submenu(k) => {
                let sub = self.build_submenu(k);
                if let Some(Overlay::Menu(root)) = self.overlay.as_mut() {
                    root.at_depth_mut(depth).sub = Some(Box::new(sub));
                }
            }
            cmd if keep_open => {
                self.run(cmd);
                // Rebuild in place so the checkmarks follow.
                if let Some(k) = kind {
                    let fresh = self.build_submenu(k);
                    if let Some(Overlay::Menu(root)) = self.overlay.as_mut() {
                        let m = root.at_depth_mut(depth);
                        m.items = fresh.items;
                    }
                }
            }
            cmd => {
                self.overlay = None;
                self.run(cmd);
            }
        }
    }

    fn settings_input(&mut self, a: Action) {
        let Some(Overlay::Settings(st)) = self.overlay.as_mut() else { return };
        if let Some(ed) = st.editor.as_mut() {
            match a {
                Action::Up => ed.row = ed.row.saturating_sub(1),
                Action::Down => ed.row = (ed.row + 1).min(4),
                Action::Left if ed.row < 3 => ed.adjust(-1.0),
                Action::Right if ed.row < 3 => ed.adjust(1.0),
                Action::Left | Action::Right => ed.row = if ed.row == 3 { 4 } else { 3 },
                Action::Accept if ed.row == 4 => st.editor = None,
                Action::Accept => self.settings_save_color(),
                Action::Back => st.editor = None,
                _ => {}
            }
            return;
        }
        let d = match a {
            Action::Left => -1.0,
            Action::Right => 1.0,
            _ => 0.0,
        };
        match a {
            Action::Up | Action::Down => {
                st.row = if a == Action::Up { st.row.saturating_sub(1) } else { (st.row + 1).min(SETTINGS_ROWS - 1) };
                // Long settings scroll to the row.
                self.ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-reveal"), true));
            }
            Action::Back | Action::Menu => self.overlay = None,
            Action::Left | Action::Right => {
                let row = st.row;
                self.settings_step(row, d);
            }
            Action::Accept => {
                let (row, swatch) = (st.row, st.swatch);
                match row {
                    row::ACCENT => self.settings_pick(swatch),
                    row::THEME => self.run(Cmd::IconThemes),
                    row::OPEN_IN => {
                        let chip = match &self.overlay {
                            Some(Overlay::Settings(st)) => st.chip,
                            _ => 0,
                        };
                        self.settings_toggle_open(chip);
                    }
                    row::VOLUMES | row::WEB_IMAGES | row::PASSWORDS | row::INSTALL => self.settings_step(row, 0.0),
                    row::LAYOUTS => {
                        let i = match &self.overlay {
                            Some(Overlay::Settings(st)) => st.layout_chip,
                            _ => 0,
                        };
                        self.settings_toggle_layout(i);
                    }
                    row::APPEARANCE | row::LANGUAGE => self.settings_step(row, 1.0),
                    row::CLOSE => self.overlay = None,
                    _ => {}
                }
            }
            // Delete a saved colour.
            Action::Actions if st.row == row::ACCENT => {
                let i = st.swatch;
                let first_custom = crate::config::ACCENT_PRESETS.len();
                if i >= first_custom && i < first_custom + self.cfg.custom_colors.len() {
                    let removed = self.cfg.custom_colors.remove(i - first_custom);
                    if self.cfg.accent == Some(removed) {
                        self.cfg.accent = None;
                        apply_accent(&self.ctx, &self.cfg);
                    }
                    self.cfg.save();
                    if let Some(Overlay::Settings(st)) = self.overlay.as_mut() {
                        st.swatch = st.swatch.min(swatches(&self.cfg).len());
                    }
                }
            }
            _ => {}
        }
    }

    /// ✕ on a row of the archive dialog.
    pub fn compress_activate(&mut self, row: usize) {
        let Some(Overlay::Compress(d)) = self.overlay.as_mut() else { return };
        if !d.enabled(row) {
            return;
        }
        d.row = row;
        let field = match row {
            crow::NAME => CField::Name,
            crow::SPLIT => CField::Split,
            crow::PASSWORD => CField::Password,
            crow::CREATE => return self.compress_create(),
            crow::CANCEL => {
                self.overlay = None;
                return;
            }
            _ => return d.step(row, 1),
        };
        let Some(Overlay::Compress(d)) = self.overlay.take() else { return };
        self.compress_osk(d, field);
    }

    fn compress_osk(&mut self, dlg: Box<CompressDlg>, field: CField) {
        let (title, text) = match field {
            CField::Name => (tr("Archive name").to_string(), dlg.name.clone()),
            CField::Split => (format!("{} ({})", tr("Volume size"), tr("MB")), dlg.opts.split.map(|b| (b / MIB).to_string()).unwrap_or_default()),
            CField::Password => (format!("{} {}", tr("Archive password"), tr("(empty = none)")), String::new()),
            CField::Confirm => (tr("Repeat the password").to_string(), String::new()),
        };
        let mut k = Osk::new(&title, &text, OskPurpose::CompressField { dlg, field });
        k.secret = matches!(field, CField::Password | CField::Confirm);
        self.overlay = Some(Overlay::Osk(k));
    }

    /// A text field of the archive dialog was entered.
    fn compress_field(&mut self, mut dlg: CompressDlg, field: CField, raw: String) {
        let text = raw.trim();
        match field {
            CField::Name => {
                if !text.is_empty() && !text.contains('/') && text != "." && text != ".." {
                    let ext = dlg.opts.format.ext();
                    let lower = text.to_lowercase();
                    dlg.name = if lower.ends_with(ext) { text[..text.len() - ext.len()].to_string() } else { text.to_string() };
                } else if !text.is_empty() {
                    self.toast(format!("{}: {text}", tr("Failed")), true);
                }
            }
            CField::Split => match text.parse::<u64>() {
                Ok(0) => dlg.opts.split = None,
                Ok(mb) => dlg.opts.split = Some(mb * MIB),
                Err(_) if text.is_empty() => dlg.opts.split = None,
                Err(_) => self.toast(format!("{}: {text}", tr("Failed")), true),
            },
            CField::Password if raw.is_empty() => {
                dlg.opts.password = None;
                dlg.opts.encrypt_names = false;
            }
            CField::Password => {
                dlg.pending_pw = Some(raw);
                return self.compress_osk(Box::new(dlg), CField::Confirm);
            }
            CField::Confirm => {
                if dlg.pending_pw.take().as_deref() == Some(raw.as_str()) {
                    dlg.opts.password = Some(raw);
                } else {
                    self.toast(tr("Passwords don't match").to_string(), true);
                }
            }
        }
        self.overlay = Some(Overlay::Compress(Box::new(dlg)));
    }

    pub fn compress_create(&mut self) {
        let Some(Overlay::Compress(d)) = self.overlay.take() else { return };
        let CompressDlg { sources, dir, name, opts, .. } = *d;
        self.cfg.compress = Some(crate::config::CompressPrefs {
            format: opts.format,
            method: opts.method,
            level: opts.level,
            split: opts.split,
            encrypt_names: opts.encrypt_names,
        });
        self.cfg.save();
        let full = format!("{name}{}", opts.format.ext());
        // Split archives become name.001…, so those must be free too.
        let name = ops::unique_name(&full, |n| {
            let p = dir.join(n);
            p.symlink_metadata().is_ok() || ops::volume_path(&p, 1).symlink_metadata().is_ok()
        });
        self.start(Op::Compress { sources, out: dir.join(name), opts });
    }

    /// ←/→ on a settings row.
    pub fn settings_step(&mut self, row: usize, d: f32) {
        use crate::config::{GRID_ICON, LIST_ICON, SIDE_ICON, ZOOM};
        let step = |v: &mut f32, (lo, hi, st): (f32, f32, f32)| *v = (*v + d * st).clamp(lo, hi);
        match row {
            row::ACCENT => {
                let n = swatches(&self.cfg).len() + 1; // + the "add" tile
                if let Some(Overlay::Settings(st)) = self.overlay.as_mut() {
                    st.swatch = (st.swatch as isize + d as isize).rem_euclid(n as isize) as usize;
                    let i = st.swatch;
                    // Moving over colours previews them right away.
                    if let Some(c) = swatches(&self.cfg).get(i).copied() {
                        self.cfg.accent = c;
                        apply_accent(&self.ctx, &self.cfg);
                        self.cfg.save();
                    }
                }
            }
            row::OPEN_IN => {
                if let Some(Overlay::Settings(st)) = self.overlay.as_mut() {
                    st.chip = (st.chip as isize + d as isize).clamp(0, OPEN_KINDS as isize - 1) as usize;
                }
                return;
            }
            row::LANGUAGE => {
                // "As the system", then each language.
                let n = crate::i18n::LANGUAGES.len() as isize + 1;
                let at = self.cfg.language.as_deref().and_then(|c| crate::i18n::LANGUAGES.iter().position(|(l, _, _)| *l == c)).map_or(0, |i| i as isize + 1);
                let next = (at + d as isize).rem_euclid(n);
                self.cfg.language = (next > 0).then(|| crate::i18n::LANGUAGES[next as usize - 1].0.to_string());
                self.cfg.save();
                crate::i18n::set(self.cfg.language.as_deref());
                self.refresh_sidebar();
            }
            row::APPEARANCE => {
                self.cfg.appearance = (self.cfg.appearance as isize + d as isize).rem_euclid(3) as u8;
                self.cfg.save();
                self.apply_appearance();
            }
            row::WEB_IMAGES => {
                self.cfg.viewer.web_images = !self.cfg.viewer.web_images;
                self.cfg.save();
            }
            row::PASSWORDS => {
                self.cfg.password_hidden = !self.cfg.password_hidden;
                self.cfg.save();
            }
            row::INSTALL => {
                self.cfg.install_on_open = !self.cfg.install_on_open;
                self.cfg.save();
            }
            row::LAYOUTS => {
                if let Some(Overlay::Settings(st)) = self.overlay.as_mut() {
                    st.layout_chip = (st.layout_chip as isize + d as isize).clamp(0, crate::keyboard::CODES.len() as isize - 1) as usize;
                }
                return;
            }
            row::VOLUMES => {
                self.cfg.collapse_volumes = !self.cfg.collapse_volumes;
                self.cfg.save();
                let cfg = self.cfg.clone();
                for t in &mut self.tabs {
                    for p in &mut t.panes {
                        p.refilter_with(None, &cfg);
                    }
                }
                return;
            }
            row::SIDE_ICON => step(&mut self.cfg.side_icon, SIDE_ICON),
            row::LIST_ICON => step(&mut self.cfg.list_icon, LIST_ICON),
            row::GRID_ICON => step(&mut self.cfg.grid_icon, GRID_ICON),
            row::ZOOM => {
                step(&mut self.cfg.zoom, ZOOM);
                self.cfg.zoom = (self.cfg.zoom * 10.0).round() / 10.0;
                self.ctx.set_zoom_factor(self.cfg.zoom);
            }
            _ => return,
        }
        self.cfg.save();
        for t in &mut self.tabs {
            for p in &mut t.panes {
                p.scroll_to_cursor = true;
            }
        }
    }

    /// Flip "open in Polka" for kind `i`.
    /// A keyboard layout on or off (one always stays).
    pub fn settings_toggle_layout(&mut self, i: usize) {
        if let Some(Overlay::Settings(st)) = self.overlay.as_mut() {
            st.row = row::LAYOUTS;
            st.layout_chip = i;
        }
        let Some(code) = crate::keyboard::CODES.get(i) else { return };
        let mut v = self.cfg.layouts();
        if let Some(p) = v.iter().position(|c| c == code) {
            if v.len() > 1 {
                v.remove(p);
            }
        } else {
            v.push(code.to_string());
            // In the keyboard's own order.
            v.sort_by_key(|c| crate::keyboard::CODES.iter().position(|x| x == c));
        }
        crate::keyboard::set_enabled(&v);
        self.cfg.osk_layouts = v;
        self.cfg.save();
    }

    pub fn settings_toggle_open(&mut self, i: usize) {
        if let Some(Overlay::Settings(st)) = self.overlay.as_mut() {
            st.row = row::OPEN_IN;
            st.chip = i;
        }
        let f = self.cfg.open_flag(i);
        *f = !*f;
        self.cfg.save();
    }

    /// Choose swatch `i` (or open the mixer on the "+" tile).
    pub fn settings_pick(&mut self, i: usize) {
        let sw = swatches(&self.cfg);
        let Some(Overlay::Settings(st)) = self.overlay.as_mut() else { return };
        st.row = row::ACCENT;
        st.swatch = i;
        match sw.get(i) {
            Some(c) => {
                self.cfg.accent = *c;
                apply_accent(&self.ctx, &self.cfg);
                self.cfg.save();
            }
            None => {
                let [r, g, b] = self.cfg.accent_rgb();
                let hsva = egui::ecolor::Hsva::from(egui::Color32::from_rgb(r, g, b));
                st.editor = Some(ColorEditor { hsv: [hsva.h, hsva.s, hsva.v], row: 0 });
            }
        }
    }

    pub fn settings_save_color(&mut self) {
        let Some(Overlay::Settings(st)) = self.overlay.as_mut() else { return };
        let Some(ed) = st.editor.take() else { return };
        let c = ed.rgb();
        let cfg = &mut self.cfg;
        if !cfg.custom_colors.contains(&c) {
            if cfg.custom_colors.len() >= crate::config::MAX_CUSTOM_COLORS {
                cfg.custom_colors.remove(0);
            }
            cfg.custom_colors.push(c);
        }
        cfg.accent = Some(c);
        st.swatch = swatches(cfg).iter().position(|s| *s == Some(c)).unwrap_or(0);
        apply_accent(&self.ctx, cfg);
        cfg.save();
    }

    /// End picture picking: set the icon (if any) and return the pane to
    /// where it was.
    pub fn finish_icon_pick(&mut self, image: Option<PathBuf>) {
        let Some(pick) = self.icon_pick.take() else { return };
        let cfg = self.cfg.clone();
        if let Some(t) = self.tabs.get_mut(pick.tab) {
            t.panes[pick.pane].navigate(pick.back, None, &cfg);
        }
        if let Some(image) = image {
            self.run(Cmd::SetPinIconFile { folder: pick.folder, image });
        }
    }

    /// Mouse: move the cursor at `depth` to `index` and accept it.
    pub fn menu_click(&mut self, depth: usize, index: usize) {
        if let Some(Overlay::Menu(root)) = self.overlay.as_mut() {
            root.at_depth_mut(depth).cursor = index;
        }
        self.menu_accept(depth);
    }

    /// Prompt for an archive password, then run `retry`.
    pub fn ask_password(&mut self, archive: PathBuf, wrong: bool, retry: Retry) {
        if self.overlay.is_some() {
            return self.toast(tr("Password required"), true);
        }
        let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = if wrong { format!("{} — {name}", tr("Wrong password")) } else { format!("{} {name}", tr("Password for")) };
        let mut k = Osk::new(&title, "", OskPurpose::Password { archive, retry });
        k.secret = true;
        self.overlay = Some(Overlay::Osk(k));
    }

    /// Copy/move, asking first if names clash at the destination.
    pub(crate) fn request_transfer(&mut self, sources: Vec<Location>, dest: Dest, mv: bool) {
        if sources.is_empty() {
            return;
        }
        let clashes = ops::conflicts(&sources, &dest);
        if clashes.is_empty() {
            return self.run(Cmd::Transfer { sources, dest, mv, policy: Conflict::Rename });
        }
        let title = if clashes.len() == 1 {
            format!("«{}» {}", clashes[0], tr("already exists"))
        } else {
            trn("%n item(s) already exist", clashes.len())
        };
        let go = |policy| Cmd::Transfer { sources: sources.clone(), dest: dest.clone(), mv, policy };
        let items = vec![
            item(tr("Keep both"), go(Conflict::Rename)),
            item(tr("Replace"), go(Conflict::Overwrite)),
            item(tr("Skip"), go(Conflict::Skip)),
            item(tr("Cancel"), Cmd::Nothing),
        ];
        self.overlay = Some(Overlay::Menu(Menu::new(title, items)));
    }

    /// Put the cursor on `name` once the pane next reloads (after a job).
    fn pending_focus(&mut self, name: String) {
        self.pane_mut().pending_focus = Some(name);
    }

    pub(crate) fn start(&mut self, op: Op) {
        let j = ops::spawn(op, self.wake());
        self.jobs.push(j);
    }

    pub(crate) fn reload_all(&mut self) {
        let cfg = self.cfg.clone();
        for t in &mut self.tabs {
            for p in &mut t.panes {
                p.reload(&cfg);
            }
        }
    }

    fn local_targets(&self) -> Vec<PathBuf> {
        let p = self.pane();
        p.targets().iter().filter_map(|e| p.entry_path(e)).collect()
    }

    pub fn run(&mut self, cmd: Cmd) {
        let cfg = self.cfg.clone();
        match cmd {
            Cmd::Nothing => {}
            Cmd::Open => self.activate(),
            Cmd::Copy | Cmd::Cut => {
                let p = self.pane();
                let items: Vec<Location> = p.targets().iter().map(|e| p.entry_loc(e)).collect();
                if !items.is_empty() {
                    let cut = matches!(cmd, Cmd::Cut);
                    self.toast(trn(if cut { "Cut: %n" } else { "Copied: %n" }, items.len()), false);
                    self.clip = Some(Clip { items, cut });
                    self.pane_mut().selected.clear();
                }
            }
            Cmd::Paste => {
                let Some(dest) = self.pane().loc.dest() else {
                    return self.toast(tr("This archive format is read-only"), true);
                };
                let Some(clip) = &self.clip else { return self.toast(tr("Nothing to paste"), true) };
                let (sources, mv) = (clip.items.clone(), clip.cut);
                self.request_transfer(sources, dest, mv);
            }
            Cmd::CopyOther | Cmd::MoveOther => {
                let (a, b) = self.pane_pair();
                let Some(dest) = b.loc.dest() else {
                    return self.toast(tr("This archive format is read-only"), true);
                };
                let sources: Vec<Location> = a.targets().iter().map(|e| a.entry_loc(e)).collect();
                self.request_transfer(sources, dest, matches!(cmd, Cmd::MoveOther));
            }
            Cmd::Transfer { sources, dest, mv, policy } => {
                self.pane_mut().selected.clear();
                if mv {
                    // A cut is used up by its paste.
                    if self.clip.as_ref().is_some_and(|c| c.cut && c.items == sources) {
                        self.clip = None;
                    }
                    self.start(Op::Move { sources, dest, policy });
                } else {
                    self.start(Op::Copy { sources, dest, policy });
                }
            }
            Cmd::Rename => {
                let p = self.pane();
                if let Some(e) = p.current() {
                    let name = e.name.rsplit('/').next().unwrap_or(&e.name).to_string();
                    let purpose = match (p.entry_path(e), p.loc.dest(), p.entry_loc(e)) {
                        (Some(old), _, _) => OskPurpose::Rename { old },
                        (None, Some(_), Location::Archive { archive, inner }) => {
                            OskPurpose::RenameInArchive { archive, from: inner }
                        }
                        _ => return,
                    };
                    self.overlay = Some(Overlay::Osk(Osk::new(tr("New name"), &name, purpose)));
                }
            }
            Cmd::DeleteAsk if self.pane().loc.is_archive() => {
                let p = self.pane();
                let inners: Vec<String> = p
                    .targets()
                    .iter()
                    .filter_map(|e| match p.entry_loc(e) {
                        Location::Archive { inner, .. } => Some(inner),
                        _ => None,
                    })
                    .collect();
                let Some(archive) = p.loc.archive().map(Path::to_path_buf) else { return };
                if inners.is_empty() {
                    return;
                }
                let title = trn("Delete %n item(s) permanently?", inners.len());
                let items = vec![item(tr("Cancel"), Cmd::Nothing), item(tr("Delete"), Cmd::DeleteInArchive { archive, inners })];
                self.overlay = Some(Overlay::Menu(Menu::new(title, items)));
            }
            Cmd::DeleteInArchive { archive, inners } => {
                self.pane_mut().selected.clear();
                let plan = crate::archive_rw::Plan { remove: inners, ..Default::default() };
                self.start(Op::ArchiveEdit { archive, plan, label: tr("Deleting") });
            }
            Cmd::Filter => {
                let prev = self.pane().filter.clone();
                let text = prev.clone().unwrap_or_default();
                self.overlay = Some(Overlay::Osk(Osk::new(tr("Filter"), &text, OskPurpose::Filter { prev })));
            }
            Cmd::Search => {
                if let Some(root) = self.pane().loc.local_path().map(Path::to_path_buf) {
                    self.overlay = Some(Overlay::Osk(Osk::new(tr("Search in subfolders"), "", OskPurpose::Search { root })));
                }
            }
            Cmd::TrashAsk | Cmd::DeleteAsk => {
                let paths = self.local_targets();
                if paths.is_empty() {
                    return;
                }
                let perm = matches!(cmd, Cmd::DeleteAsk);
                let title = if perm {
                    trn("Delete %n item(s) permanently?", paths.len())
                } else {
                    trn("Move %n item(s) to trash?", paths.len())
                };
                let go = if perm { Cmd::DeleteNow(paths) } else { Cmd::TrashNow(paths) };
                // Cancel first, so a stray double press of A never deletes.
                let items = vec![item(tr("Cancel"), Cmd::Nothing), item(if perm { tr("Delete") } else { tr("Move to trash") }, go)];
                self.overlay = Some(Overlay::Menu(Menu::new(title, items)));
            }
            Cmd::TrashNow(paths) => {
                self.pane_mut().selected.clear();
                self.start(Op::Trash { paths });
            }
            Cmd::DeleteNow(paths) => {
                self.pane_mut().selected.clear();
                self.start(Op::Delete { paths });
            }
            Cmd::OpenExternal => {
                let path = match &self.cur_tab().viewer {
                    Some(v) => Some(v.path.clone()),
                    None => self.pane().current().and_then(|e| self.pane().entry_path(e)),
                };
                if let Some(p) = path {
                    // Not the in-app viewer: straight to xdg-open.
                    let mut cmd = std::process::Command::new("xdg-open");
                    cmd.arg(&p);
                    if let Err(e) = crate::launch::spawn_cmd(cmd) {
                        self.toast(format!("{}: {e}", tr("Failed")), true);
                    }
                }
            }
            Cmd::OpenInViewer => self.view_current(),
            Cmd::NewFile(template) => {
                let Some(dest) = self.pane().loc.dest() else { return };
                let name = match &template {
                    Some(t) => t.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    None => tr("New file").to_string(),
                };
                // A free name to start from.
                let taken: HashSet<String> = self.pane().all.iter().map(|e| e.name.clone()).collect();
                let name = ops::unique_name(&name, |n| taken.contains(n));
                self.overlay = Some(Overlay::Osk(Osk::new(tr("File name"), &name, OskPurpose::NewFile { dest, template })));
            }
            Cmd::OpenTemplates => {
                if let Some(t) = dirs::template_dir() {
                    let _ = std::fs::create_dir_all(&t);
                    let cfg = self.cfg.clone();
                    self.close_viewer();
                    self.pane_mut().navigate(Location::Local(t), None, &cfg);
                }
            }
            Cmd::ViewerEdit
            | Cmd::ViewerInsertAsk
            | Cmd::ViewerNewline
            | Cmd::ViewerUndo
            | Cmd::ViewerRedo
            | Cmd::ViewerCopy
            | Cmd::ViewerCut
            | Cmd::ViewerPaste
            | Cmd::ViewerSelectAll
            | Cmd::ViewerFindAsk
            | Cmd::ViewerFindOpt(_)
            | Cmd::ViewerReplaceAsk
            | Cmd::ViewerReplace
            | Cmd::ViewerReplaceAll
            | Cmd::ViewerToggleMd
            | Cmd::ViewerSave
            | Cmd::ViewerSaveThen(_)
            | Cmd::ViewerDiscardThen(_) => self.viewer_cmd(cmd),
            Cmd::CloseViewer => self.request_close_viewer(),
            Cmd::RunInWindow { title, dir, cmd } => self.open_term_window(title, &dir, &cmd),
            Cmd::SteamAdd(path) => self.steam_add(&path),
            Cmd::SteamAddNow { s, restart } => self.steam_add_now(s, restart),
            Cmd::BrowseArchive => {
                let cfg = self.cfg.clone();
                let p = self.pane();
                if let Some(path) = p.current().and_then(|e| p.entry_path(e)) {
                    self.pane_mut().navigate(vfs::archive_loc(path), None, &cfg);
                }
            }
            Cmd::DropLink { sources, dir } => {
                self.drop_done();
                self.make_links(sources, dir);
            }
            Cmd::DropTransfer { sources, dest, mv } => {
                self.drop_done();
                self.request_transfer(sources, dest, mv);
            }
            Cmd::OpenWith { app, files } => {
                self.drop_done();
                if let Err(e) = crate::launch::open_with(&app, &files) {
                    self.toast(format!("{}: {e}", tr("Failed")), true);
                }
            }
            Cmd::CancelJobs => {
                for j in &self.jobs {
                    j.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Cmd::EmptyTrashAsk | Cmd::EmptyTrashNow | Cmd::DriveMount(_) | Cmd::DriveUnmount(_) | Cmd::DriveEject(_) => self.side_cmd(cmd),
            Cmd::UsageOpen(_)
            | Cmd::UsageReveal
            | Cmd::UsageZoomIn
            | Cmd::UsageZoomOut
            | Cmd::UsageMode(_)
            | Cmd::UsageRescan
            | Cmd::UsageStart
            | Cmd::UsageClose => self.usage_cmd(cmd),
            Cmd::TermOpen(_)
            | Cmd::TermNew
            | Cmd::TermCloseTab(_)
            | Cmd::TermSwitch(_)
            | Cmd::TermHide
            | Cmd::TermClose
            | Cmd::TermRun(_)
            | Cmd::TermType(_)
            | Cmd::TermRunAsk
            | Cmd::TermSaveAsk
            | Cmd::TermSave(_)
            | Cmd::TermForget(_)
            | Cmd::TermInterrupt
            | Cmd::TermClear
            | Cmd::TermSelect
            | Cmd::TermCopy
            | Cmd::TermPaste
            | Cmd::TermShowFolder
            | Cmd::TermKeys
            | Cmd::TermHistoryMenu
            | Cmd::TermSavedMenu
            | Cmd::TermSettings => self.terminal_cmd(cmd),
            Cmd::AudioToggle | Cmd::AudioNext | Cmd::AudioPrev | Cmd::AudioPlay(_) | Cmd::AudioRepeat | Cmd::AudioVolume(_) | Cmd::AudioSeek(_) => {
                self.audio_cmd(cmd);
            }
            Cmd::VideoFullscreen | Cmd::VideoAudio | Cmd::VideoSubs => {
                self.video_cmd(cmd);
            }
            Cmd::ViewerSettings => {
                self.overlay = Some(Overlay::ViewerSettings(ViewerSettings { row: vrow::THEME }));
            }
            Cmd::ViewerWrap => self.viewer_step(vrow::WRAP, 0),
            Cmd::ViewerNumbers => self.viewer_step(vrow::NUMBERS, 0),
            Cmd::CompressAsk => {
                let sources = self.local_targets();
                let Some(dir) = self.pane().loc.local_path().map(Path::to_path_buf) else { return };
                let base = if sources.len() == 1 {
                    sources[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                } else {
                    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "archive".into())
                };
                let name = if sources.len() == 1 && !sources[0].is_dir() { ops::split_ext(&base).0.to_string() } else { base };
                let mut opts = CompressOpts::new(ArchiveFormat::Zip);
                if let Some(p) = &self.cfg.compress {
                    if CompressDlg::formats().contains(&p.format) {
                        opts.format = p.format;
                    }
                    opts.method = p.method;
                    opts.level = p.level;
                    opts.split = p.split;
                    opts.encrypt_names = p.encrypt_names;
                    opts.fit();
                }
                let dlg = CompressDlg { sources, dir, name, opts, row: crow::CREATE, pending_pw: None };
                self.overlay = Some(Overlay::Compress(Box::new(dlg)));
            }
            Cmd::ExtractHere | Cmd::ExtractOther => {
                let Some(archive) = self.local_targets().into_iter().next() else { return };
                let dest = if matches!(cmd, Cmd::ExtractOther) {
                    let (_, b) = self.pane_pair();
                    b.loc.local_path().map(Path::to_path_buf)
                } else {
                    archive.parent().map(Path::to_path_buf)
                };
                if let Some(dest) = dest {
                    let archive = crate::volumes::first(&archive);
                    self.start(Op::Extract { archive, dest });
                }
            }
            Cmd::Pin(p) => {
                if !self.cfg.pinned.contains(&p) {
                    self.cfg.pinned.push(p);
                    self.cfg.save();
                    self.refresh_sidebar();
                }
            }
            Cmd::Unpin(p) => {
                self.cfg.pinned.retain(|x| *x != p);
                self.cfg.save();
                self.refresh_sidebar();
            }
            Cmd::OpenInNewTab(loc) => {
                let other = loc.clone();
                self.tabs.push(Tab { panes: [Pane::new(loc, &cfg), Pane::new(other, &cfg)], active: 0, viewer: None, usage: None, term: None });
                self.tab = self.tabs.len() - 1;
                self.focus = Focus::Panes;
            }
            Cmd::Properties => self.open_props(),
            Cmd::NewTab => {
                let loc = self.pane().loc.clone();
                self.run(Cmd::OpenInNewTab(loc));
            }
            Cmd::CloseTab => self.run(Cmd::CloseTabAt(self.tab)),
            Cmd::CloseTabAt(i) => {
                if self.tabs.len() > 1 && i < self.tabs.len() && !self.confirm_unsaved(i, crate::viewer::glue::Then::CloseTab(i)) {
                    self.tabs.remove(i);
                    // Closing a tab left of the current one shifts it; closing
                    // the current one lands on its neighbour.
                    if i < self.tab {
                        self.tab -= 1;
                    }
                    self.tab = self.tab.min(self.tabs.len() - 1);
                    self.range = None;
                }
            }
            Cmd::NewFolder => {
                if let Some(dest) = self.pane().loc.dest() {
                    self.overlay = Some(Overlay::Osk(Osk::new(tr("Folder name"), "", OskPurpose::NewFolder { dest })));
                }
            }
            Cmd::ToggleDual => {
                self.cfg.dual = !self.cfg.dual;
                self.cfg.save();
            }
            Cmd::ToggleHidden => {
                self.cfg.show_hidden = !self.cfg.show_hidden;
                self.cfg.save();
                self.reload_all();
            }
            Cmd::ChooseIcon(path) => {
                // Only names the picker can really draw: no blank cells.
                let mut groups = self.icons.place_icons();
                for g in &mut groups {
                    let names = std::mem::take(&mut g.names);
                    g.names = names.into_iter().filter(|n| self.icons.named(&[n.as_str()], 64).is_some()).collect();
                }
                groups.retain(|g| !g.names.is_empty());
                if groups.iter().all(|g| g.names.is_empty()) {
                    return self.toast(format!("{}: {}", tr("Failed"), tr("no icons in this theme")), true);
                }
                // Start on the current icon, if it's in the theme.
                let cur = self.cfg.pinned_icons.get(&path.to_string_lossy().into_owned()).cloned();
                let (group, cursor) = cur
                    .and_then(|c| {
                        groups.iter().enumerate().find_map(|(gi, g)| g.names.iter().position(|n| *n == c).map(|i| (gi, i)))
                    })
                    .unwrap_or((0, 0));
                self.overlay = Some(Overlay::IconPicker(IconPicker {
                    path,
                    groups,
                    group,
                    cursor,
                    cols: IconPicker::COLS,
                    scrolled: None,
                    scroll_y: 0.0,
                }));
            }
            Cmd::PickIconFile(folder) => {
                // Browse in the current pane; it comes back here afterwards.
                let pane = if self.cfg.dual { self.tabs[self.tab].active } else { 0 };
                let back = self.pane().loc.clone();
                let start = dirs::picture_dir().filter(|d| d.is_dir()).or_else(dirs::home_dir).unwrap_or_else(|| "/".into());
                self.icon_pick = Some(IconPick { folder, tab: self.tab, pane, back });
                self.overlay = None;
                self.focus = Focus::Panes;
                self.pane_mut().navigate(Location::Local(start), None, &cfg);
            }
            Cmd::SetPinIconFile { folder, image } => match store_icon(&image) {
                Ok(stored) => self.run(Cmd::SetPinIcon(folder, Some(stored.to_string_lossy().into_owned()))),
                Err(e) => self.toast(format!("{}: {e}", tr("Failed")), true),
            },
            Cmd::SetPinIcon(path, icon) => {
                let key = path.to_string_lossy().into_owned();
                match icon {
                    Some(i) => self.cfg.pinned_icons.insert(key, i),
                    None => self.cfg.pinned_icons.remove(&key),
                };
                self.cfg.save();
                self.refresh_sidebar();
            }
            Cmd::Settings => {
                let sw = swatches(&self.cfg);
                let swatch = sw.iter().position(|c| *c == self.cfg.accent).unwrap_or(0);
                self.overlay = Some(Overlay::Settings(Settings { row: 0, chip: 0, layout_chip: 0, swatch, editor: None }));
            }
            Cmd::IconThemes => {
                let system = crate::icons::detect_theme().unwrap_or_else(|| "hicolor".into());
                let mut items = vec![(None, format!("{} ({system})", tr("System")))];
                let mut preview = vec![system];
                for t in crate::icons::installed_themes() {
                    preview.push(t.id.clone());
                    items.push((Some(t.id), t.name));
                }
                let cursor = items.iter().position(|(id, _)| *id == self.cfg.icon_theme).unwrap_or(0);
                self.overlay = Some(Overlay::Themes(ThemePicker { items, cursor, preview, scrolled: None }));
            }
            Cmd::SetIconTheme(id) => {
                self.icons.set_theme(id.clone());
                self.cfg.icon_theme = id;
                self.cfg.save();
                self.refresh_sidebar();
            }
            Cmd::ToggleThumbnails => {
                self.cfg.thumbnails = !self.cfg.thumbnails;
                self.cfg.save();
            }
            Cmd::ToggleSidebar => {
                self.cfg.sidebar_collapsed = !self.cfg.sidebar_collapsed;
                self.cfg.save();
            }
            Cmd::SetView(v) => {
                self.cfg.view = v;
                self.cfg.save();
                for t in &mut self.tabs {
                    for p in &mut t.panes {
                        p.scroll_to_cursor = true;
                    }
                }
            }
            Cmd::Sort(k) => {
                self.cfg.sort = k;
                self.cfg.save();
                self.reload_all();
            }
            Cmd::ToggleSortReverse => {
                self.cfg.sort_reverse = !self.cfg.sort_reverse;
                self.cfg.save();
                self.reload_all();
            }
            Cmd::ToggleDirsFirst => {
                self.cfg.dirs_first = !self.cfg.dirs_first;
                self.cfg.save();
                self.reload_all();
            }
            Cmd::Submenu(_) => {}
            Cmd::GoTo => {
                let cur = self.pane().loc.display();
                self.overlay = Some(Overlay::Osk(Osk::new(tr("Path"), &cur, OskPurpose::GoTo)));
            }
            Cmd::SelectAll => {
                let p = self.pane_mut();
                p.selected = p.entries.iter().map(|e| e.name.clone()).collect();
            }
            Cmd::Help => self.overlay = Some(Overlay::Help),
            Cmd::Quit => {
                let dirty = (0..self.tabs.len()).find(|&i| self.confirm_unsaved(i, crate::viewer::glue::Then::Quit));
                if dirty.is_none() {
                    self.quit = true;
                }
            }
        }
    }

    fn open_props(&mut self) {
        let p = self.pane();
        let targets = p.targets();
        let mut rows = Vec::new();
        let mut icon = None;
        if targets.len() == 1 {
            let e = &targets[0];
            let path = p.entry_path(e);
            rows.push((tr("Name").to_string(), e.name.clone()));
            let kind = if e.is_dir { tr("Folder").to_string() } else {
                mime_guess::from_path(&e.name).first_raw().map(str::to_string).unwrap_or_else(|| tr("File").into())
            };
            rows.push((tr("Type").into(), if e.is_link { format!("{kind} ({})", tr("Link")) } else { kind }));
            rows.push((tr("Location").into(), p.loc.display()));
            if e.is_dir {
                if let Ok(es) = vfs::list(&p.entry_loc(e)) {
                    rows.push((tr("Contents").into(), format!("{} {}", es.len(), tr("items"))));
                }
            } else {
                rows.push((tr("Size").into(), format!("{} ({} B)", human_size(e.size), e.size)));
            }
            if let Some(t) = e.mtime {
                rows.push((tr("Modified").into(), fmt_time(t)));
            }
            rows.push((tr("Permissions").into(), fmt_mode(e.mode)));
            if let Some(pp) = &path {
                if let Ok(target) = std::fs::read_link(pp) {
                    rows.push(("→".into(), target.display().to_string()));
                }
            }
            let e = e.clone();
            icon = self.icons.for_entry(&e, path.as_deref(), 64);
        } else {
            let total: u64 = targets.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
            rows.push((tr("selected").into(), targets.len().to_string()));
            rows.push((tr("Size").into(), human_size(total)));
            rows.push((tr("Location").into(), p.loc.display()));
        }
        self.overlay = Some(Overlay::Props(Props { rows, icon }));
    }
}

/// What to show for a command-line argument: a folder, an archive as a
/// folder, or a file's folder with the cursor on it.
fn start_location(p: &Path) -> Option<(Location, Option<String>)> {
    let p = std::fs::canonicalize(p).ok()?;
    if p.is_dir() {
        return Some((Location::Local(p), None));
    }
    let name = p.file_name()?.to_string_lossy().into_owned();
    if vfs::is_archive_name(&name) {
        return Some((vfs::archive_loc(p), None));
    }
    Some((Location::Local(p.parent()?.to_path_buf()), Some(name)))
}

/// A path from argv, which may be a file:// URI (from .desktop %U).
pub fn arg_path(arg: &std::ffi::OsStr) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    let s = arg.to_string_lossy();
    let Some(rest) = s.strip_prefix("file://") else { return PathBuf::from(arg) };
    // Skip an optional host part ("file://localhost/...").
    let rest = &rest[rest.find('/').unwrap_or(0)..];
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    PathBuf::from(std::ffi::OsString::from_vec(out))
}

/// Image types the icon loaders can draw.
pub fn is_icon_image(name: &str) -> bool {
    let l = name.to_lowercase();
    [".png", ".svg", ".jpg", ".jpeg", ".webp", ".ico", ".bmp", ".gif"].iter().any(|e| l.ends_with(e))
}

/// Copy a user image into Polka's data dir (so moving the original doesn't
/// break the icon). Returns the stored path.
fn store_icon(image: &Path) -> std::io::Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let data = std::fs::read(image)?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    let ext = image.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "png".into());
    let dir = dirs::data_dir().ok_or_else(|| std::io::Error::other("no data dir"))?.join("polka/icons");
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{:016x}.{ext}", h.finish()));
    if !out.exists() {
        std::fs::write(&out, data)?;
    }
    Ok(out)
}

fn expand_tilde(s: &str) -> PathBuf {
    if let (Some(rest), Some(h)) = (s.strip_prefix('~'), dirs::home_dir()) {
        return h.join(rest.trim_start_matches('/'));
    }
    PathBuf::from(s)
}

/// Removable/user mounts from /proc/self/mounts: (device, mount point).
fn mounts() -> Vec<(String, PathBuf)> {
    let Ok(text) = std::fs::read_to_string("/proc/self/mounts") else { return Vec::new() };
    let unescape = |s: &str| s.replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\");
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for line in text.lines() {
        let mut f = line.split(' ');
        let (Some(dev), Some(mnt)) = (f.next(), f.next()) else { continue };
        let mnt = unescape(mnt);
        if ["/run/media/", "/media/", "/mnt/"].iter().any(|p| mnt.starts_with(p)) && !out.iter().any(|(_, m)| m.as_os_str() == mnt.as_str()) {
            out.push((dev.to_string(), PathBuf::from(mnt)));
        }
    }
    out
}

/// The home trash's files folder.
pub fn trash_files() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("Trash/files"))
}

pub fn human_size(n: u64) -> String {
    const U: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

pub fn fmt_time(t: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(t, 0) {
        chrono::LocalResult::Single(d) => d.format("%Y-%m-%d %H:%M").to_string(),
        _ => String::new(),
    }
}

fn fmt_mode(m: u32) -> String {
    let mut s = String::new();
    for shift in [6, 3, 0] {
        let b = (m >> shift) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    format!("{s} ({:o})", m & 0o7777)
}

#[cfg(test)]
mod tests {
    #[test]
    fn uri_args() {
        use std::ffi::OsStr;
        use std::path::PathBuf;
        assert_eq!(super::arg_path(OsStr::new("file:///home/a%20b/%D0%B6.zip")), PathBuf::from("/home/a b/ж.zip"));
        assert_eq!(super::arg_path(OsStr::new("file://localhost/tmp/x")), PathBuf::from("/tmp/x"));
        assert_eq!(super::arg_path(OsStr::new("/plain/%20")), PathBuf::from("/plain/%20"));
        assert_eq!(super::arg_path(OsStr::new("file:///end%2")), PathBuf::from("/end%2"));
    }
}
