//! Application state and input/command handling. Rendering lives in view.rs.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use crate::config::{Config, SortKey, ViewMode};
use crate::i18n::{tr, trn};
use crate::icons::Icons;
use crate::input::{Action, Input, PadKind};
use crate::ops::{self, ArchiveFormat, Job, Op};
use crate::vfs::{self, Entry, Location};

// ---------------------------------------------------------------- panes

pub struct Pane {
    pub loc: Location,
    pub entries: Vec<Entry>,
    pub error: Option<String>,
    pub cursor: usize,
    pub selected: HashSet<String>,
    back: Vec<(Location, String)>,
    forward: Vec<(Location, String)>,
    /// Nested archives opened from inside another archive:
    /// (extracted temp archive, location it was opened from, its name there).
    nest: Vec<(PathBuf, Location, String)>,
    pub loading: Option<Receiver<std::io::Result<Vec<Entry>>>>,
    pending_focus: Option<String>,
    pub scroll_to_cursor: bool,
    /// Viewport height from the last frame, for keeping the cursor visible.
    pub view_h: f32,
    pub scroll_y: f32,
    /// Grid columns from the last frame.
    pub cols: usize,
    pub page_rows: usize,
}

impl Pane {
    pub fn new(loc: Location, cfg: &Config) -> Self {
        let mut p = Self {
            loc: loc.clone(),
            entries: Vec::new(),
            error: None,
            cursor: 0,
            selected: HashSet::new(),
            back: Vec::new(),
            forward: Vec::new(),
            nest: Vec::new(),
            loading: None,
            pending_focus: None,
            scroll_to_cursor: true,
            view_h: 400.0,
            scroll_y: 0.0,
            cols: 1,
            page_rows: 10,
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
        self.loc.local_path().map(|p| p.join(&e.name))
    }

    /// Selection if any, else the entry under the cursor.
    pub fn targets(&self) -> Vec<Entry> {
        if self.selected.is_empty() {
            self.current().cloned().into_iter().collect()
        } else {
            self.entries.iter().filter(|e| self.selected.contains(&e.name)).cloned().collect()
        }
    }

    fn load(&mut self, loc: Location, focus: Option<String>, cfg: &Config) {
        self.loc = loc.clone();
        self.selected.clear();
        self.error = None;
        if loc.is_archive() {
            // Archive indexing can take a while (tar has no index), so it runs
            // off the UI thread.
            let (tx, rx) = crossbeam_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send(vfs::list(&loc));
            });
            self.entries.clear();
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
                sort_entries(&mut v, cfg.sort);
                self.entries = v;
                self.error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.error = Some(e.to_string());
            }
        }
        self.cursor = focus
            .and_then(|n| self.entries.iter().position(|e| e.name == n))
            .unwrap_or(0);
        let names: HashSet<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        self.selected.retain(|n| names.contains(n.as_str()));
        self.scroll_to_cursor = true;
    }

    /// Re-list the current location, keeping cursor and selection.
    pub fn reload(&mut self, cfg: &Config) {
        let focus = self.current().map(|e| e.name.clone());
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

fn sort_entries(v: &mut [Entry], key: SortKey) {
    v.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| match key {
            SortKey::Name => natural_cmp(&a.name, &b.name),
            SortKey::Size => b.size.cmp(&a.size).then_with(|| natural_cmp(&a.name, &b.name)),
            SortKey::Date => b.mtime.cmp(&a.mtime).then_with(|| natural_cmp(&a.name, &b.name)),
        })
    });
}

pub struct Tab {
    pub panes: [Pane; 2],
    pub active: usize,
}

// ---------------------------------------------------------------- overlays

#[derive(Clone, Debug)]
pub enum Cmd {
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
    CompressAsk,
    Compress(ArchiveFormat),
    ExtractHere,
    ExtractOther,
    Pin(PathBuf),
    Unpin(PathBuf),
    OpenInNewTab(Location),
    Properties,
    NewTab,
    CloseTab,
    NewFolder,
    ToggleDual,
    ToggleHidden,
    ToggleSidebar,
    SetView(ViewMode),
    Sort(SortKey),
    GoTo,
    SelectAll,
    Help,
    Quit,
    Nothing,
}

pub struct MenuItem {
    pub label: String,
    pub cmd: Cmd,
    pub enabled: bool,
    pub checked: Option<bool>,
}

pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
    pub cursor: usize,
}

impl Menu {
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

fn item(label: &str, cmd: Cmd) -> MenuItem {
    MenuItem { label: label.to_string(), cmd, enabled: true, checked: None }
}

pub enum OskPurpose {
    Rename { old: PathBuf },
    NewFolder { dir: PathBuf },
    GoTo,
    Compress { sources: Vec<PathBuf>, dir: PathBuf, format: ArchiveFormat },
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OskKey {
    Char(char),
    Shift,
    Layout,
    Space,
    Backspace,
    Left,
    Right,
    Ok,
}

pub const OSK_LAYOUTS: [(&str, [&str; 4]); 2] = [
    ("EN", ["1234567890-", "qwertyuiop_", "asdfghjkl.,", "zxcvbnm()[]"]),
    ("RU", ["1234567890-", "йцукенгшщзх", "фывапролджэ", "ячсмитьбюъё"]),
];
pub const OSK_SPECIAL: [OskKey; 6] =
    [OskKey::Shift, OskKey::Layout, OskKey::Space, OskKey::Backspace, OskKey::Left, OskKey::Ok];

impl Osk {
    fn new(title: &str, text: &str, purpose: OskPurpose) -> Self {
        let text: Vec<char> = text.chars().collect();
        // Put the caret before the extension when renaming files.
        let caret = match &purpose {
            OskPurpose::Rename { old } if !old.is_dir() => {
                let s: String = text.iter().collect();
                ops::split_ext(&s).0.chars().count()
            }
            OskPurpose::Compress { .. } => text.len(),
            _ => text.len(),
        };
        Self { title: title.into(), text, caret, row: 1, col: 0, layout: 0, shift: false, purpose }
    }

    pub fn rows(&self) -> usize {
        5
    }

    pub fn row_len(&self, row: usize) -> usize {
        if row < 4 { OSK_LAYOUTS[self.layout].1[row].chars().count() } else { OSK_SPECIAL.len() }
    }

    pub fn key_at(&self, row: usize, col: usize) -> OskKey {
        if row < 4 {
            let c = OSK_LAYOUTS[self.layout].1[row].chars().nth(col).unwrap_or(' ');
            OskKey::Char(if self.shift { c.to_uppercase().next().unwrap_or(c) } else { c })
        } else {
            OSK_SPECIAL[col.min(OSK_SPECIAL.len() - 1)]
        }
    }

    fn nav(&mut self, a: Action) {
        let rows = self.rows();
        match a {
            Action::Up | Action::Down => {
                // Keep the horizontal position proportional between rows.
                let frac = (self.col as f32 + 0.5) / self.row_len(self.row) as f32;
                self.row = if a == Action::Up { (self.row + rows - 1) % rows } else { (self.row + 1) % rows };
                self.col = ((frac * self.row_len(self.row) as f32) as usize).min(self.row_len(self.row) - 1);
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
}

// ---------------------------------------------------------------- sidebar

#[derive(Clone)]
pub struct SideItem {
    pub label: String,
    pub icon: &'static [&'static str],
    pub loc: Location,
    pub pinned: bool,
    pub section: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Panes,
}

pub struct Clip {
    pub items: Vec<Location>,
    pub cut: bool,
}

pub enum Msg {
    Toast(String, bool),
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
    select_held: Option<bool>,
    pub clip: Option<Clip>,
    pub jobs: Vec<Arc<Job>>,
    pub toast: Option<(String, bool, Instant)>,
    pub msg_tx: Sender<Msg>,
    msg_rx: Receiver<Msg>,
    pub ctx: egui::Context,
    pub quit: bool,
}

impl App {
    pub fn new(ctx: egui::Context, start: Option<PathBuf>) -> Self {
        let cfg = Config::load();
        let wake_ctx = ctx.clone();
        let pad_rx = crate::input::spawn_gamepad(move || wake_ctx.request_repaint());
        let icon_ctx = ctx.clone();
        let icons = Icons::new(cfg.icon_theme.clone(), move || icon_ctx.request_repaint());
        let home = dirs::home_dir().unwrap_or_else(|| "/".into());
        let start = start.filter(|p| p.is_dir()).unwrap_or(home.clone());
        let tab = Tab {
            panes: [Pane::new(Location::Local(start), &cfg), Pane::new(Location::Local(home), &cfg)],
            active: 0,
        };
        let (msg_tx, msg_rx) = crossbeam_channel::unbounded();
        ctx.set_zoom_factor(cfg.zoom);
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
            select_held: None,
            clip: None,
            jobs: Vec::new(),
            toast: None,
            msg_tx,
            msg_rx,
            ctx,
            quit: false,
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

    pub fn toast(&mut self, s: impl Into<String>, err: bool) {
        self.toast = Some((s.into(), err, Instant::now()));
    }

    // ------------------------------------------------------------ per frame

    pub fn tick(&mut self) {
        let cfg = self.cfg.clone();
        for t in &mut self.tabs {
            for p in &mut t.panes {
                p.poll(&cfg);
            }
        }
        while let Ok(m) = self.msg_rx.try_recv() {
            match m {
                Msg::Toast(s, e) => self.toast(s, e),
            }
        }
        // Finished jobs: refresh what they touched, report errors.
        let mut touched: Vec<Location> = Vec::new();
        let mut report = Vec::new();
        self.jobs.retain(|j| {
            let fin = *j.finished_at.lock().unwrap();
            match fin {
                None => true,
                Some(_) => {
                    touched.extend(j.touches.iter().cloned());
                    if let Some(Err(e)) = j.result.lock().unwrap().as_ref() {
                        report.push(format!("{}: {e}", j.label));
                    }
                    false
                }
            }
        });
        for r in report {
            self.toast(r, true);
        }
        if !touched.is_empty() {
            for t in &mut self.tabs {
                for p in &mut t.panes {
                    if touched.contains(&p.loc) {
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

    pub fn refresh_sidebar(&mut self) {
        self.side_refreshed = Instant::now();
        let mut v = Vec::new();
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
                v.push(SideItem { label: tr(label).into(), icon, loc: Location::Local(p), pinned: false, section: "Places" });
            }
        }
        v.push(SideItem {
            label: tr("File system").into(),
            icon: &["drive-harddisk-root", "drive-harddisk", "folder-root", "folder"],
            loc: Location::Local("/".into()),
            pinned: false,
            section: "Places",
        });
        let trash = dirs::data_dir().map(|d| d.join("Trash/files"));
        if let Some(t) = trash.filter(|t| t.is_dir()) {
            v.push(SideItem { label: tr("Trash").into(), icon: &["user-trash", "user-trash-full"], loc: Location::Local(t), pinned: false, section: "Places" });
        }
        for p in &self.cfg.pinned {
            v.push(SideItem {
                label: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()),
                icon: &["folder-bookmark", "bookmark-new", "folder"],
                loc: Location::Local(p.clone()),
                pinned: true,
                section: "Pinned",
            });
        }
        for (dev, mnt) in mounts() {
            let label = mnt.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(dev);
            v.push(SideItem { label, icon: &["drive-removable-media", "drive-harddisk", "media-flash"], loc: Location::Local(mnt), pinned: false, section: "Devices" });
        }
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
                self.select_held = None;
                return;
            }
            Input::Release(_) => return,
            Input::Press(a) => a,
        };
        if self.overlay.is_some() {
            self.handle_overlay(a);
            return;
        }
        if self.focus == Focus::Sidebar {
            self.handle_sidebar(a);
            return;
        }
        let cfg = self.cfg.clone();
        let grid = cfg.view == ViewMode::Grid;
        let cols = if grid { self.pane().cols.max(1) as isize } else { 1 };
        match a {
            Action::Up | Action::FastUp => self.move_and_paint(-cols),
            Action::Down | Action::FastDown => self.move_and_paint(cols),
            Action::Left => {
                let p = self.pane();
                if grid && p.cursor as isize % cols != 0 {
                    self.move_and_paint(-1);
                } else {
                    self.focus_left();
                }
            }
            Action::Right => {
                let p = self.pane();
                let last_col = (p.cursor as isize + 1) % cols == 0 || p.cursor + 1 >= p.entries.len();
                if grid && !last_col {
                    self.move_and_paint(1);
                } else if cfg.dual && self.cur_tab().active == 0 {
                    self.tabs[self.tab].active = 1;
                }
            }
            Action::Home => self.pane_mut().set_cursor(0),
            Action::End => {
                let n = self.pane().entries.len();
                self.pane_mut().set_cursor(n.saturating_sub(1));
            }
            Action::Accept => self.activate(),
            Action::Back => {
                if !self.pane().selected.is_empty() {
                    self.pane_mut().selected.clear();
                } else {
                    self.pane_mut().go_up(&cfg);
                }
            }
            Action::Select => {
                let p = self.pane_mut();
                if let Some(name) = p.current().map(|e| e.name.clone()) {
                    let on = !p.selected.contains(&name);
                    if on { p.selected.insert(name) } else { p.selected.remove(&name) };
                    self.select_held = Some(on);
                }
            }
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
            Action::RightStick => self.run(Cmd::Properties),
            Action::HistoryBack => self.pane_mut().history(false, &cfg),
            Action::HistoryForward => self.pane_mut().history(true, &cfg),
        }
    }

    fn focus_left(&mut self) {
        if self.cfg.dual && self.cur_tab().active == 1 {
            self.tabs[self.tab].active = 0;
        } else {
            self.focus = Focus::Sidebar;
            // Land on the sidebar entry for the current folder if there is one.
            let loc = self.pane().loc.clone();
            if let Some(i) = self.side_items.iter().position(|s| s.loc == loc) {
                self.side_cursor = i;
            }
        }
    }

    /// Move the cursor; while Select is held, paint the selection along.
    fn move_and_paint(&mut self, d: isize) {
        let held = self.select_held;
        let p = self.pane_mut();
        let from = p.cursor;
        p.move_cursor(d);
        if let Some(on) = held {
            let (lo, hi) = (from.min(p.cursor), from.max(p.cursor));
            for i in lo..=hi {
                let n = p.entries[i].name.clone();
                if on { p.selected.insert(n) } else { p.selected.remove(&n) };
            }
        }
    }

    fn switch_tab(&mut self, d: isize) {
        let n = self.tabs.len() as isize;
        self.tab = (self.tab as isize + d).rem_euclid(n) as usize;
    }

    fn handle_sidebar(&mut self, a: Action) {
        let n = self.side_items.len();
        match a {
            Action::Up | Action::FastUp => self.side_cursor = self.side_cursor.saturating_sub(1),
            Action::Down | Action::FastDown => self.side_cursor = (self.side_cursor + 1).min(n.saturating_sub(1)),
            Action::Home => self.side_cursor = 0,
            Action::End => self.side_cursor = n.saturating_sub(1),
            Action::Right | Action::Back => self.focus = Focus::Panes,
            Action::Accept => {
                if let Some(it) = self.side_items.get(self.side_cursor).cloned() {
                    let cfg = self.cfg.clone();
                    self.pane_mut().navigate(it.loc, None, &cfg);
                    self.focus = Focus::Panes;
                }
            }
            Action::Actions => {
                if let Some(it) = self.side_items.get(self.side_cursor).cloned() {
                    let mut items = vec![item(tr("New tab"), Cmd::OpenInNewTab(it.loc.clone()))];
                    if let (true, Some(p)) = (it.pinned, it.loc.local_path()) {
                        items.push(item(tr("Unpin from sidebar"), Cmd::Unpin(p.to_path_buf())));
                    }
                    self.overlay = Some(Overlay::Menu(Menu { title: it.label, items, cursor: 0 }));
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
            Overlay::Menu(m) => match a {
                Action::Up | Action::FastUp => m.step(-1),
                Action::Down | Action::FastDown => m.step(1),
                Action::Accept => {
                    let cmd = m.items.get(m.cursor).filter(|i| i.enabled).map(|i| i.cmd.clone());
                    self.overlay = None;
                    if let Some(c) = cmd {
                        self.run(c);
                    }
                }
                Action::Back | Action::Menu | Action::Actions => self.overlay = None,
                _ => {}
            },
            Overlay::Osk(k) => match a {
                Action::Up | Action::Down | Action::Left | Action::Right => k.nav(a),
                Action::Accept => {
                    let key = k.key_at(k.row, k.col);
                    self.osk_key(key);
                }
                Action::Back => self.overlay = None,
                Action::Select => k.backspace(),
                Action::Actions => k.insert(" "),
                Action::PrevTab => k.caret_by(-1),
                Action::NextTab => k.caret_by(1),
                Action::LeftTrigger => k.shift = !k.shift,
                Action::View => k.layout = (k.layout + 1) % OSK_LAYOUTS.len(),
                Action::Menu | Action::RightTrigger => self.osk_submit(),
                _ => {}
            },
            Overlay::Props(_) | Overlay::Help => {
                if matches!(a, Action::Back | Action::Accept | Action::RightStick | Action::Menu) {
                    self.overlay = None;
                }
            }
        }
    }

    pub fn osk_key(&mut self, key: OskKey) {
        let Some(Overlay::Osk(k)) = self.overlay.as_mut() else { return };
        match key {
            OskKey::Char(c) => {
                k.insert(&c.to_string());
            }
            OskKey::Shift => k.shift = !k.shift,
            OskKey::Layout => k.layout = (k.layout + 1) % OSK_LAYOUTS.len(),
            OskKey::Space => k.insert(" "),
            OskKey::Backspace => k.backspace(),
            OskKey::Left => k.caret_by(-1),
            OskKey::Right => k.caret_by(1),
            OskKey::Ok => self.osk_submit(),
        }
    }

    pub fn osk_submit(&mut self) {
        let Some(Overlay::Osk(k)) = self.overlay.take() else { return };
        let text = k.string().trim().to_string();
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
                    Err(e) => self.toast(format!("{}: {e}", tr("Failed")), true),
                }
            }
            OskPurpose::NewFolder { dir } => {
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
                    Err(e) => self.toast(format!("{}: {e}", tr("Failed")), true),
                }
            }
            OskPurpose::GoTo => {
                let path = expand_tilde(&text);
                if path.is_dir() {
                    self.pane_mut().navigate(Location::Local(path), None, &cfg);
                } else if path.is_file() && vfs::is_archive_name(&text) {
                    self.pane_mut().navigate(Location::Archive { archive: path, inner: String::new() }, None, &cfg);
                } else {
                    self.toast(format!("{}: {text}", tr("Failed")), true);
                }
            }
            OskPurpose::Compress { sources, dir, format } => {
                if bad_name {
                    return self.toast(format!("{}: {text}", tr("Failed")), true);
                }
                let name = if text.to_lowercase().ends_with(format.ext()) { text } else { format!("{text}{}", format.ext()) };
                let out = ops::unique_path(&dir, &name);
                self.start(Op::Compress { sources, out, format });
            }
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
            Location::Local(dir) => {
                let path = dir.join(&e.name);
                if vfs::is_archive_name(&e.name) {
                    self.pane_mut().navigate(Location::Archive { archive: path, inner: String::new() }, None, &cfg);
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
                        Err(err) => self.toast(format!("{}: {err}", tr("Failed")), true),
                    }
                } else {
                    let (archive, tx) = (archive.clone(), self.msg_tx.clone());
                    let wake = self.wake();
                    std::thread::spawn(move || {
                        let r = vfs::extract_to_temp(&archive, &inner_path)
                            .map_err(|e| e.to_string())
                            .and_then(|p| crate::launch::open(&p));
                        if let Err(err) = r {
                            let _ = tx.send(Msg::Toast(format!("{}: {err}", tr("Failed")), true));
                            wake();
                        }
                    });
                }
            }
        }
    }

    fn open_actions_menu(&mut self) {
        let p = self.pane();
        let targets = p.targets();
        let in_archive = p.loc.is_archive();
        let local = !in_archive;
        let single = targets.len() == 1;
        let any = !targets.is_empty();
        let dual = self.cfg.dual;
        let other_local = self.cur_tab().panes[1 - self.cur_tab().active].loc.local_path().is_some();
        let one_archive = single && !targets[0].is_dir && vfs::is_archive_name(&targets[0].name);
        let one_dir_local = single && targets[0].is_dir && local;
        let pinned = one_dir_local
            && p.entry_path(&targets[0]).is_some_and(|pp| self.cfg.pinned.contains(&pp));

        let mut items = Vec::new();
        let mut add = |label: &str, cmd: Cmd, enabled: bool| {
            items.push(MenuItem { label: label.into(), cmd, enabled, checked: None })
        };
        add(if one_archive { tr("Open as folder") } else { tr("Open") }, Cmd::Open, single);
        add(tr("Copy"), Cmd::Copy, any);
        add(tr("Cut"), Cmd::Cut, any && local);
        add(tr("Paste"), Cmd::Paste, self.clip.is_some() && local);
        if dual {
            add(tr("Copy to other pane"), Cmd::CopyOther, any && other_local);
            add(tr("Move to other pane"), Cmd::MoveOther, any && local && other_local);
        }
        add(tr("Rename"), Cmd::Rename, single && local);
        add(tr("Move to trash"), Cmd::TrashAsk, any && local);
        add(tr("Delete permanently"), Cmd::DeleteAsk, any && local);
        add(tr("Compress…"), Cmd::CompressAsk, any && local);
        if one_archive {
            add(tr("Extract here"), Cmd::ExtractHere, local);
            if dual {
                add(tr("Extract to other pane"), Cmd::ExtractOther, local && other_local);
            }
        }
        if one_dir_local {
            let path = p.entry_path(&targets[0]).unwrap();
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
        self.overlay = Some(Overlay::Menu(Menu { title, items, cursor: 0 }.first_enabled()));
    }

    fn open_app_menu(&mut self) {
        let c = &self.cfg;
        let check = |label: &str, cmd: Cmd, on: bool| MenuItem { label: label.into(), cmd, enabled: true, checked: Some(on) };
        let local = self.pane().loc.local_path().is_some();
        let items = vec![
            item(tr("New tab"), Cmd::NewTab),
            MenuItem { enabled: self.tabs.len() > 1, ..item(tr("Close tab"), Cmd::CloseTab) },
            MenuItem { enabled: local, ..item(tr("New folder"), Cmd::NewFolder) },
            MenuItem { enabled: self.clip.is_some() && local, ..item(tr("Paste"), Cmd::Paste) },
            item(tr("Go to path…"), Cmd::GoTo),
            item(tr("Select all"), Cmd::SelectAll),
            check(tr("Dual pane"), Cmd::ToggleDual, c.dual),
            check(tr("Sidebar"), Cmd::ToggleSidebar, !c.sidebar_collapsed),
            check(tr("Show hidden files"), Cmd::ToggleHidden, c.show_hidden),
            check(tr("List view"), Cmd::SetView(ViewMode::List), c.view == ViewMode::List),
            check(tr("Grid view"), Cmd::SetView(ViewMode::Grid), c.view == ViewMode::Grid),
            check(tr("Sort by name"), Cmd::Sort(SortKey::Name), c.sort == SortKey::Name),
            check(tr("Sort by size"), Cmd::Sort(SortKey::Size), c.sort == SortKey::Size),
            check(tr("Sort by date"), Cmd::Sort(SortKey::Date), c.sort == SortKey::Date),
            item(tr("Controls"), Cmd::Help),
            item(tr("Quit"), Cmd::Quit),
        ];
        self.overlay = Some(Overlay::Menu(Menu { title: "Polka".into(), items, cursor: 0 }));
    }

    fn start(&mut self, op: Op) {
        let j = ops::spawn(op, self.wake());
        self.jobs.push(j);
    }

    fn reload_all(&mut self) {
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
                    self.toast(format!("{} {}", items.len(), if cut { tr("Cut") } else { tr("Copy") }), false);
                    self.clip = Some(Clip { items, cut });
                    self.pane_mut().selected.clear();
                }
            }
            Cmd::Paste => {
                let Some(dest) = self.pane().loc.local_path().map(Path::to_path_buf) else {
                    return self.toast(tr("Archives are read-only for now"), true);
                };
                let Some(clip) = self.clip.take() else { return self.toast(tr("Nothing to paste"), true) };
                if clip.cut {
                    self.start(Op::Move { sources: clip.items, dest });
                } else {
                    self.start(Op::Copy { sources: clip.items.clone(), dest });
                    self.clip = Some(clip);
                }
            }
            Cmd::CopyOther | Cmd::MoveOther => {
                let (a, b) = self.pane_pair();
                let Some(dest) = b.loc.local_path().map(Path::to_path_buf) else {
                    return self.toast(tr("Archives are read-only for now"), true);
                };
                let sources: Vec<Location> = a.targets().iter().map(|e| a.entry_loc(e)).collect();
                a.selected.clear();
                if matches!(cmd, Cmd::MoveOther) {
                    self.start(Op::Move { sources, dest });
                } else {
                    self.start(Op::Copy { sources, dest });
                }
            }
            Cmd::Rename => {
                let p = self.pane();
                if let Some(e) = p.current() {
                    if let Some(path) = p.entry_path(e) {
                        let name = e.name.clone();
                        self.overlay = Some(Overlay::Osk(Osk::new(tr("New name"), &name, OskPurpose::Rename { old: path })));
                    }
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
                self.overlay = Some(Overlay::Menu(Menu { title, items, cursor: 0 }));
            }
            Cmd::TrashNow(paths) => {
                self.pane_mut().selected.clear();
                self.start(Op::Trash { paths });
            }
            Cmd::DeleteNow(paths) => {
                self.pane_mut().selected.clear();
                self.start(Op::Delete { paths });
            }
            Cmd::CompressAsk => {
                let items = ArchiveFormat::ALL.iter().map(|f| item(f.ext(), Cmd::Compress(*f))).collect();
                self.overlay = Some(Overlay::Menu(Menu { title: tr("Archive format").into(), items, cursor: 0 }));
            }
            Cmd::Compress(format) => {
                let sources = self.local_targets();
                let Some(dir) = self.pane().loc.local_path().map(Path::to_path_buf) else { return };
                let base = if sources.len() == 1 {
                    sources[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                } else {
                    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "archive".into())
                };
                let base = if sources.len() == 1 && !sources[0].is_dir() { ops::split_ext(&base).0.to_string() } else { base };
                self.overlay = Some(Overlay::Osk(Osk::new(tr("Archive name"), &base, OskPurpose::Compress { sources, dir, format })));
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
                self.tabs.push(Tab { panes: [Pane::new(loc, &cfg), Pane::new(other, &cfg)], active: 0 });
                self.tab = self.tabs.len() - 1;
                self.focus = Focus::Panes;
            }
            Cmd::Properties => self.open_props(),
            Cmd::NewTab => {
                let loc = self.pane().loc.clone();
                self.run(Cmd::OpenInNewTab(loc));
            }
            Cmd::CloseTab => {
                if self.tabs.len() > 1 {
                    self.tabs.remove(self.tab);
                    self.tab = self.tab.min(self.tabs.len() - 1);
                }
            }
            Cmd::NewFolder => {
                if let Some(dir) = self.pane().loc.local_path().map(Path::to_path_buf) {
                    self.overlay = Some(Overlay::Osk(Osk::new(tr("Folder name"), "", OskPurpose::NewFolder { dir })));
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
            Cmd::GoTo => {
                let cur = self.pane().loc.display();
                self.overlay = Some(Overlay::Osk(Osk::new(tr("Path"), &cur, OskPurpose::GoTo)));
            }
            Cmd::SelectAll => {
                let p = self.pane_mut();
                p.selected = p.entries.iter().map(|e| e.name.clone()).collect();
            }
            Cmd::Help => self.overlay = Some(Overlay::Help),
            Cmd::Quit => self.quit = true,
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
