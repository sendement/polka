//! The torrents page in the app: starting the engine, the page's pad and
//! menu, adding a torrent (its files chosen, where it goes).

use std::path::PathBuf;

use super::{Engine, Peek, Settings, Source, State};
use crate::app::{App, Cmd, Focus, Menu, MenuItem, Osk, OskPurpose, Overlay, item};
use crate::i18n::tr;
use crate::input::Action;
use crate::vfs::Location;

/// Adding a torrent: its files to choose, where it goes.
pub struct AddDlg {
    /// The engine's look inside it.
    pub peek: u64,
    /// What it came from (a file's name, "Magnet link").
    pub title: String,
    pub magnet: bool,
    /// Once looked inside.
    pub name: String,
    pub files: Vec<(String, u64, bool)>,
    pub bytes: Vec<u8>,
    pub error: Option<String>,
    /// 0: where it goes; then the files.
    pub cursor: usize,
    /// The folder the panes show (another place to put it), and whether
    /// it's chosen.
    pub here: Option<PathBuf>,
    pub to_here: bool,
    pub scrolled: Option<usize>,
}

impl AddDlg {
    pub fn ready(&self) -> bool {
        !self.bytes.is_empty()
    }

    pub fn chosen(&self) -> (usize, u64) {
        self.files.iter().filter(|f| f.2).fold((0, 0), |(n, s), f| (n + 1, s + f.1))
    }

    /// All chosen, or none if all were.
    pub fn all_or_none(&mut self) {
        let all = self.files.iter().all(|f| f.2);
        for f in &mut self.files {
            f.2 = !all;
        }
    }

    pub fn toggle(&mut self, i: usize) {
        if let Some(f) = self.files.get_mut(i) {
            f.2 = !f.2;
        }
    }
}

/// What a click on the add window asks.
pub enum AddEv {
    Toggle(usize),
    All,
    Dest,
    Download,
    Cancel,
}

impl App {
    fn torrent_settings(&self) -> Settings {
        let t = &self.cfg.torrent;
        let env = |k: &str| std::env::var_os(k);
        Settings {
            folder: t.folder(),
            down: t.down,
            up: t.up,
            seed: t.seed,
            // (Tests: their own place, no internet.)
            data: env("POLKA_TORRENT_DATA").map_or_else(super::data_dir, PathBuf::from),
            local_only: env("POLKA_TORRENT_LOCAL").is_some(),
            peers: env("POLKA_TORRENT_PEERS").map(|s| s.to_string_lossy().split(',').filter_map(|a| a.parse().ok()).collect()).unwrap_or_default(),
        }
    }

    /// The engine, started if need be.
    pub fn torrent_engine(&mut self) -> &mut Engine {
        if self.torrents.is_none() {
            let set = self.torrent_settings();
            self.torrents = Some(Engine::start(set, self.wake()));
        }
        self.torrents.as_mut().expect("started")
    }

    /// Torrents left from last time go on as Polka starts (if set so).
    pub fn torrents_autostart(&mut self) {
        let t = &self.cfg.torrent;
        if t.enabled && t.autostart && super::any_remembered(&self.torrent_settings().data) {
            self.torrent_engine();
        }
    }

    /// The page is what the current tab shows.
    pub fn torrents_shown(&self) -> bool {
        let t = self.cur_tab();
        t.viewer.is_none() && t.torrents
    }

    /// The page in a tab of its own.
    pub fn open_torrents(&mut self) {
        self.tab_for_page(|t| t.torrents);
        let t = &mut self.tabs[self.tab];
        t.hide_pages();
        t.torrents = true;
        self.torrent_engine();
        self.focus = Focus::Panes;
        self.range = None;
    }

    /// A torrent file or magnet link to download: its files to choose
    /// first (a file opens elsewhere when the client is off).
    pub fn torrent_add(&mut self, src: Source) {
        if !self.cfg.torrent.enabled {
            let r = match &src {
                Source::File(p) => crate::launch::open(p),
                Source::Magnet(m) => crate::launch::open(std::path::Path::new(m)),
                Source::Bytes(_) => return,
            };
            if let Err(e) = r {
                self.toast(format!("{}: {e}", tr("Failed")), true);
            }
            return;
        }
        let (title, magnet) = match &src {
            Source::File(p) => (p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned()), false),
            Source::Magnet(_) => (tr("Magnet link").to_string(), true),
            Source::Bytes(_) => (tr("Torrent").to_string(), false),
        };
        let here = self.pane().loc.local_path().map(|p| p.to_path_buf()).filter(|p| *p != self.cfg.torrent.folder());
        let peek = self.torrent_engine().peek(src);
        self.overlay = Some(Overlay::TorrentAdd(Box::new(AddDlg { peek, title, magnet, name: String::new(), files: Vec::new(), bytes: Vec::new(), error: None, cursor: 0, here, to_here: false, scrolled: None })));
    }

    /// Each frame: what the engine found, finished, or failed.
    pub fn tick_torrents(&mut self) {
        let set = self.torrent_settings();
        let Some(e) = self.torrents.as_mut() else { return };
        e.set(set);
        // The look inside a torrent being added.
        if let Some(Overlay::TorrentAdd(d)) = self.overlay.as_mut() {
            if !d.ready() && d.error.is_none() {
                // (The client didn't start: nothing will come.)
                let failed = e.snap.lock().ok().and_then(|s| s.error.clone());
                match e.peek_state(d.peek) {
                    _ if failed.is_some() => d.error = failed.map(|f| format!("{}: {f}", tr("The torrent client couldn't start"))),
                    Some(Peek::Ready { name, files, bytes }) => {
                        d.name = name;
                        d.files = files.into_iter().map(|(p, l)| (p, l, true)).collect();
                        d.bytes = bytes;
                        e.forget_peek(d.peek);
                    }
                    Some(Peek::Failed(err)) => {
                        d.error = Some(match err.strip_prefix("already:") {
                            Some(name) => format!("{}: {name}", tr("Already in the list")),
                            None => err,
                        });
                        e.forget_peek(d.peek);
                    }
                    _ => self.ctx.request_repaint_after(std::time::Duration::from_millis(200)),
                }
            }
        }
        let (finished, failed) = e.take_news();
        for n in finished {
            self.toast(format!("{}: {n}", tr("Downloaded")), false);
        }
        for f in failed {
            self.toast(format!("{}: {f}", tr("Failed")), true);
        }
        if self.torrents_shown() {
            self.ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }

    pub fn torrent_items(&self) -> Vec<super::Item> {
        self.torrents.as_ref().map(|e| e.items()).unwrap_or_default()
    }

    pub fn torrents_input(&mut self, a: Action) {
        let n = self.torrent_items().len();
        let c = &mut self.torrent_cursor;
        match a {
            Action::Up => *c = c.saturating_sub(1),
            Action::Down => *c = (*c + 1).min(n.saturating_sub(1)),
            Action::Home => *c = 0,
            Action::End => *c = n.saturating_sub(1),
            Action::Accept => {
                let c = *c;
                self.run(Cmd::TorrentToggle(c));
            }
            Action::Select => self.run(Cmd::TorrentMagnet),
            Action::Actions => self.torrents_menu(),
            Action::Left => self.focus_sidebar(),
            Action::Back => self.run(Cmd::TorrentsClose),
            Action::Menu => self.open_app_menu(),
            _ => {}
        }
    }

    /// △ on the page: the torrent under the cursor, and the page's own.
    pub fn torrents_menu(&mut self) {
        let items_now = self.torrent_items();
        let i = self.torrent_cursor.min(items_now.len().saturating_sub(1));
        let mut items: Vec<MenuItem> = Vec::new();
        let title = items_now.get(i).map_or_else(|| tr("Torrents").to_string(), |t| t.name.clone());
        if let Some(t) = items_now.get(i) {
            let paused = matches!(t.state, State::Paused | State::Done);
            items.push(item(if paused { tr("Resume") } else { tr("Pause") }, Cmd::TorrentToggle(i)));
            items.push(item(tr("Show in folder"), Cmd::TorrentReveal(i)));
            items.push(item(tr("Remove from the list"), Cmd::TorrentRemove(i, false)));
            items.push(item(tr("Remove with its files"), Cmd::TorrentRemove(i, true)));
        }
        items.push(item(tr("Add a magnet link…"), Cmd::TorrentMagnet));
        items.push(item(tr("Close"), Cmd::TorrentsClose));
        self.overlay = Some(Overlay::Menu(Menu::new(title, items)));
    }

    pub fn torrents_cmd(&mut self, cmd: Cmd) {
        let items = self.torrent_items();
        let id = |i: usize| items.get(i).map(|t| t.id);
        match cmd {
            Cmd::TorrentsOpen => self.open_torrents(),
            Cmd::TorrentsClose if self.tabs[self.tab].own && self.tabs[self.tab].viewer.is_none() => {
                let i = self.tab;
                self.close_beside(i);
            }
            Cmd::TorrentsClose => self.tabs[self.tab].torrents = false,
            Cmd::TorrentMagnet => {
                // (A link copied lately goes in ready.)
                let clip = self.text_clip.clone().filter(|c| c.trim_start().starts_with("magnet:")).unwrap_or_default();
                let mut k = Osk::new(tr("Magnet link"), clip.trim(), OskPurpose::TorrentMagnet);
                k.select_all = true;
                self.overlay = Some(Overlay::Osk(k));
            }
            Cmd::TorrentToggle(i) => {
                self.torrent_cursor = i;
                let (Some(t), Some(e)) = (items.get(i), self.torrents.as_ref()) else { return };
                if matches!(t.state, State::Paused | State::Done | State::Error(_)) {
                    e.resume(t.id);
                } else {
                    e.pause(t.id);
                }
            }
            Cmd::TorrentReveal(i) => {
                let Some(t) = items.get(i) else { return };
                let (dir, focus) = t.reveal();
                if !dir.exists() {
                    return self.toast(format!("{}: {}", tr("Not found"), dir.display()), true);
                }
                let cfg = self.cfg.clone();
                self.tabs[self.tab].hide_pages();
                self.pane_mut().navigate(Location::Local(dir), focus, &cfg);
                self.focus = Focus::Panes;
            }
            Cmd::TorrentRemove(i, files) => {
                if let (Some(id), Some(e)) = (id(i), self.torrents.as_ref()) {
                    e.remove(id, files);
                }
                self.torrent_cursor = self.torrent_cursor.min(items.len().saturating_sub(2));
            }
            _ => {}
        }
    }

    /// A magnet link typed or pasted.
    pub fn torrent_magnet_typed(&mut self, text: &str) {
        match Source::parse(text) {
            Some(src) => self.torrent_add(src),
            None if text.trim().is_empty() => {}
            None => self.toast(tr("That's not a magnet link"), true),
        }
    }

    /// The pad in the add window.
    pub fn torrent_add_input(&mut self, a: Action) {
        let Some(Overlay::TorrentAdd(d)) = self.overlay.as_mut() else { return };
        let n = d.files.len();
        match a {
            Action::Up => d.cursor = d.cursor.saturating_sub(1),
            Action::Down => d.cursor = (d.cursor + 1).min(n),
            Action::Home => d.cursor = 0,
            Action::End => d.cursor = n,
            Action::Accept if d.cursor == 0 => self.torrent_add_ev(AddEv::Dest),
            Action::Left | Action::Right if d.cursor == 0 => self.torrent_add_ev(AddEv::Dest),
            Action::Accept => {
                let i = d.cursor - 1;
                self.torrent_add_ev(AddEv::Toggle(i));
            }
            Action::Select => self.torrent_add_ev(AddEv::All),
            Action::Menu => self.torrent_add_ev(AddEv::Download),
            Action::Back => self.torrent_add_ev(AddEv::Cancel),
            _ => {}
        }
    }

    pub fn torrent_add_ev(&mut self, ev: AddEv) {
        let Some(Overlay::TorrentAdd(d)) = self.overlay.as_mut() else { return };
        match ev {
            AddEv::Toggle(i) => {
                d.cursor = i + 1;
                d.toggle(i);
            }
            AddEv::All => d.all_or_none(),
            AddEv::Dest => {
                d.cursor = 0;
                if d.here.is_some() {
                    d.to_here = !d.to_here;
                }
            }
            AddEv::Cancel => {
                let peek = d.peek;
                if let Some(e) = self.torrents.as_ref() {
                    e.forget_peek(peek);
                }
                self.overlay = None;
            }
            AddEv::Download => {
                if !d.ready() || d.chosen().0 == 0 {
                    return;
                }
                let Some(Overlay::TorrentAdd(d)) = self.overlay.take() else { return };
                let base = match (&d.here, d.to_here) {
                    (Some(h), true) => h.clone(),
                    _ => self.cfg.torrent.folder(),
                };
                let folder = super::target_folder(&base, &d.name, d.files.len());
                let only = (!d.files.iter().all(|f| f.2)).then(|| d.files.iter().enumerate().filter(|(_, f)| f.2).map(|(i, _)| i).collect());
                let bytes = d.bytes;
                self.torrent_engine().add(bytes, folder, only);
                // The page shows it coming.
                if !self.torrents_shown() {
                    self.open_torrents();
                }
                self.torrent_cursor = self.torrent_items().len();
            }
        }
    }

    pub fn torrents_hints(&self) -> Vec<(Action, &'static str)> {
        let mut v = Vec::new();
        if !self.torrent_items().is_empty() {
            v.push((Action::Up, tr("Choose")));
            v.push((Action::Accept, tr("Pause / resume")));
            v.push((Action::Actions, tr("Actions")));
        }
        v.push((Action::Select, tr("Magnet link")));
        v.push((Action::Back, tr("Close")));
        v
    }
}
