//! The app side of the terminal page: the pad, the keyboard, menus,
//! history and saved commands, quick keys and settings.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::Side;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vi_mode::ViMotion;
use egui::Key;

use super::draw::Act;
use super::keys::{self, K, Mods, QKey};
use super::{Out, Terminal};
use crate::app::{App, Cmd, Focus, Menu, MenuItem, Osk, OskPurpose, Overlay, SubMenu, item, toggle};
use crate::i18n::tr;
use crate::input::Action;

/// How long ○ is held to leave the page (a tap is Esc).
const HOLD_LEAVE: Duration = Duration::from_millis(500);

/// The quick keys panel.
pub struct QuickKeys {
    pub title: String,
    pub keys: Vec<QKey>,
    pub cursor: usize,
}

pub const QK_COLS: usize = 6;

/// Terminal settings, shown beside the terminal.
pub struct TermSettings {
    pub row: usize,
}

pub mod trow {
    pub const SCHEME: usize = 0;
    pub const FONT: usize = 1;
    pub const SIZE: usize = 2;
    pub const SHELL: usize = 3;
    pub const CURSOR: usize = 4;
    pub const SCROLLBACK: usize = 5;
    pub const HERE: usize = 6;
    pub const CLOSE: usize = 7;
}

impl App {
    /// The page is what the current tab shows.
    pub fn term_shown(&self) -> bool {
        let t = self.cur_tab();
        t.viewer.is_none() && t.term.as_ref().is_some_and(|u| u.shown)
    }

    fn term_mut(&mut self) -> Option<&mut Terminal> {
        self.tabs[self.tab].term.as_deref_mut()
    }

    fn term_ref(&self) -> Option<&Terminal> {
        self.cur_tab().term.as_deref()
    }

    fn with_session(&mut self, f: impl FnOnce(&mut super::Session)) {
        if let Some(s) = self.term_mut().and_then(|t| t.cur_mut()) {
            f(s);
        }
    }

    /// Where a new shell starts: the given folder, else the panel's (or
    /// home, as set).
    fn term_dir(&self, dir: Option<PathBuf>) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| "/".into());
        dir.or_else(|| self.cfg.terminal.here.then(|| self.pane().loc.local_path().map(|p| p.to_path_buf())).flatten()).filter(|p| p.is_dir()).unwrap_or(home)
    }

    /// The page in this tab (a new tab if a file is shown here). With a
    /// folder, or with no shell yet, a shell is started there.
    pub fn open_terminal(&mut self, dir: Option<PathBuf>) {
        if self.cur_tab().viewer.is_some() {
            self.run(Cmd::NewTab);
        }
        let fresh = dir.is_some() || self.term_ref().is_none_or(|t| t.sessions.is_empty());
        let cwd = self.term_dir(dir);
        let ctx = self.ctx.clone();
        let prefs = self.cfg.terminal.clone();
        let t = self.tabs[self.tab].term.get_or_insert_with(|| Box::new(Terminal::new()));
        t.shown = true;
        if fresh {
            if let Err(e) = t.open(&prefs, &cwd, &ctx) {
                self.toast(format!("{}: {e}", tr("Failed")), true);
            }
        }
        self.focus = Focus::Panes;
        self.range = None;
    }

    pub fn tick_terminal(&mut self) {
        let dark = self.dark_applied.unwrap_or(true);
        let scheme = super::scheme::resolve(&self.cfg.terminal.scheme, dark);
        let mut outs = Vec::new();
        let mut leave = false;
        for (ti, tab) in self.tabs.iter_mut().enumerate() {
            let Some(t) = tab.term.as_deref_mut() else { continue };
            for s in &mut t.sessions {
                for o in s.poll(&scheme) {
                    outs.push((ti, o));
                }
            }
            // A shell that ended takes its tab with it; the last one, the page.
            let before = t.sessions.len();
            let mut i = 0;
            while i < t.sessions.len() {
                if t.sessions[i].exited {
                    t.close(i);
                } else {
                    i += 1;
                }
            }
            if before > 0 && t.sessions.is_empty() {
                tab.term = None;
                continue;
            }
            if t.sessions.iter().any(|s| s.waiting()) {
                self.ctx.request_repaint_after(Duration::from_millis(130));
            }
            if let Some(d) = t.back_down {
                if d.elapsed() >= HOLD_LEAVE && ti == self.tab {
                    t.back_down = None;
                    leave = true;
                } else {
                    self.ctx.request_repaint_after(Duration::from_millis(50));
                }
            }
        }
        if leave {
            self.focus_sidebar();
        }
        for (ti, o) in outs {
            match o {
                Out::Copy(s) => {
                    self.ctx.copy_text(s.clone());
                    self.text_clip = Some(s);
                }
                Out::PasteRequest(f) => {
                    let text = self.clipboard_text().unwrap_or_default();
                    if let Some(s) = self.tabs[ti].term.as_deref().and_then(|t| t.cur()) {
                        s.write(f(&text));
                    }
                }
            }
        }
    }

    fn clipboard_text(&mut self) -> Option<String> {
        let sys = self.sys_clip.as_mut().and_then(|c| c.get()).filter(|s| !s.is_empty());
        sys.or_else(|| self.text_clip.clone())
    }

    fn term_key(&mut self, k: K, m: Mods) {
        self.with_session(|s| {
            let b = keys::encode(k, m, s.mode());
            s.input(b);
        });
    }

    /// The pad on the page.
    pub fn terminal_input(&mut self, a: Action) {
        let Some(t) = self.term_mut() else { return };
        if t.sessions.is_empty() {
            match a {
                Action::Back => self.run(Cmd::TermClose),
                Action::Accept => self.run(Cmd::TermNew),
                Action::Menu => self.open_app_menu(),
                Action::Actions => self.terminal_menu(),
                _ => {}
            }
            return;
        }
        if t.selecting {
            return self.select_input(a);
        }
        match a {
            Action::Up => self.term_key(K::Up, Mods::NONE),
            Action::Down => self.term_key(K::Down, Mods::NONE),
            Action::Left => self.term_key(K::Left, Mods::NONE),
            Action::Right => self.term_key(K::Right, Mods::NONE),
            Action::Home => self.term_key(K::Home, Mods::NONE),
            Action::End => self.term_key(K::End, Mods::NONE),
            Action::Accept => self.term_key(K::Enter, Mods::NONE),
            Action::Back => {
                if let Some(t) = self.term_mut() {
                    t.back_down = Some(Instant::now());
                }
            }
            Action::Select => self.run(Cmd::TermRunAsk),
            Action::View => self.run(Cmd::TermKeys),
            Action::Actions => self.terminal_menu(),
            Action::Menu => self.open_app_menu(),
            Action::LeftTrigger | Action::RightTrigger => {
                let up = a == Action::LeftTrigger;
                let alt = self.term_ref().and_then(|t| t.cur()).is_some_and(|s| s.mode().contains(TermMode::ALT_SCREEN));
                if alt {
                    // Full-screen programs page themselves.
                    self.term_key(if up { K::PageUp } else { K::PageDown }, Mods::NONE);
                } else {
                    self.with_session(|s| s.term.lock().scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown }));
                }
            }
            _ => {}
        }
    }

    /// Button let go on the page: a short ○ is Esc.
    pub fn terminal_release(&mut self, a: Action) {
        if a != Action::Back {
            return;
        }
        let Some(t) = self.term_mut() else { return };
        if let Some(d) = t.back_down.take() {
            if d.elapsed() < HOLD_LEAVE {
                if t.selecting {
                    self.end_select();
                } else {
                    self.term_key(K::Esc, Mods::NONE);
                }
            }
        }
    }

    // ------------------------------------------------------------ choosing text

    fn begin_select(&mut self) {
        let Some(t) = self.term_mut() else { return };
        let Some(s) = t.sessions.get(t.active) else { return };
        let mut term = s.term.lock();
        if !term.mode().contains(TermMode::VI) {
            term.toggle_vi_mode();
        }
        term.selection = None;
        drop(term);
        t.selecting = true;
    }

    fn end_select(&mut self) {
        let Some(t) = self.term_mut() else { return };
        t.selecting = false;
        if let Some(s) = t.sessions.get(t.active) {
            let mut term = s.term.lock();
            if term.mode().contains(TermMode::VI) {
                term.toggle_vi_mode();
            }
            term.selection = None;
            term.scroll_display(Scroll::Bottom);
        }
    }

    fn select_input(&mut self, a: Action) {
        let Some(t) = self.term_mut() else { return };
        let rows = t.grid.1.max(1);
        let Some(s) = t.sessions.get(t.active) else { return };
        let mut term = s.term.lock();
        let motion = |term: &mut alacritty_terminal::Term<super::Proxy>, m: ViMotion, n: usize| {
            for _ in 0..n {
                term.vi_motion(m);
            }
            let p = term.vi_mode_cursor.point;
            term.vi_goto_point(p);
        };
        match a {
            Action::Up => motion(&mut term, ViMotion::Up, 1),
            Action::Down => motion(&mut term, ViMotion::Down, 1),
            Action::Left => motion(&mut term, ViMotion::Left, 1),
            Action::Right => motion(&mut term, ViMotion::Right, 1),
            Action::LeftTrigger => motion(&mut term, ViMotion::Up, rows),
            Action::RightTrigger => motion(&mut term, ViMotion::Down, rows),
            Action::Home => motion(&mut term, ViMotion::First, 1),
            Action::End => motion(&mut term, ViMotion::Last, 1),
            Action::PrevTab => motion(&mut term, ViMotion::WordLeft, 1),
            Action::NextTab => motion(&mut term, ViMotion::WordRight, 1),
            Action::Select => {
                // □ starts the selection here (again: drops it).
                if term.selection.is_some() {
                    term.selection = None;
                } else {
                    let p = term.vi_mode_cursor.point;
                    let mut sel = Selection::new(SelectionType::Simple, p, Side::Left);
                    // Non-empty from the start, so moving stretches it.
                    sel.include_all();
                    term.selection = Some(sel);
                }
            }
            Action::View => {
                // ⧉ takes whole lines.
                let p = term.vi_mode_cursor.point;
                let mut sel = Selection::new(SelectionType::Lines, p, Side::Left);
                sel.include_all();
                term.selection = Some(sel);
            }
            Action::Accept => {
                // Nothing chosen: the line under the cursor.
                if term.selection.is_none() {
                    let p = term.vi_mode_cursor.point;
                    let mut sel = Selection::new(SelectionType::Lines, p, Side::Left);
                    sel.include_all();
                    term.selection = Some(sel);
                }
                let text = term.selection_to_string().unwrap_or_default();
                drop(term);
                self.end_select();
                if !text.trim().is_empty() {
                    self.ctx.copy_text(text.clone());
                    self.text_clip = Some(text);
                    self.toast(tr("Copied"), false);
                }
                return;
            }
            Action::Back => {
                drop(term);
                if let Some(t) = self.term_mut() {
                    t.back_down = Some(Instant::now());
                }
                return;
            }
            Action::Actions => {
                drop(term);
                return self.terminal_menu();
            }
            Action::Menu => {
                drop(term);
                return self.open_app_menu();
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ keyboard

    /// Keys go to the shell; a few Ctrl+Shift ones stay with the page.
    pub fn terminal_keyboard(&mut self, events: Vec<egui::Event>) -> Vec<egui::Event> {
        let mut rest = Vec::new();
        let mods = self.ctx.input(|i| i.modifiers);
        for ev in events {
            match ev {
                egui::Event::Text(t) => {
                    let alt = self.ctx.input(|i| i.modifiers.alt);
                    let t = if alt { format!("\x1b{t}") } else { t };
                    self.with_session(|s| s.input(t.into_bytes()));
                }
                // The window system turns these keys into clipboard events.
                egui::Event::Copy => {
                    if mods.shift {
                        self.run(Cmd::TermCopy);
                    } else {
                        self.term_key(K::Char('c'), Mods::CTRL);
                    }
                }
                egui::Event::Cut => self.term_key(K::Char('x'), Mods::CTRL),
                egui::Event::Paste(p) => {
                    if mods.shift || !mods.ctrl {
                        self.with_session(|s| s.paste(&p));
                    } else {
                        self.term_key(K::Char('v'), Mods::CTRL);
                    }
                }
                egui::Event::Key { key, pressed: true, modifiers: m, .. } => {
                    if !self.terminal_key(key, m) {
                        rest.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m });
                    }
                }
                _ => {}
            }
        }
        rest
    }

    /// One key press; false leaves it to the app.
    fn terminal_key(&mut self, key: Key, m: egui::Modifiers) -> bool {
        let mods = Mods { ctrl: m.ctrl || m.command, alt: m.alt, shift: m.shift };
        if self.term_ref().is_some_and(|t| t.selecting) {
            let a = match key {
                Key::ArrowUp => Action::Up,
                Key::ArrowDown => Action::Down,
                Key::ArrowLeft => Action::Left,
                Key::ArrowRight => Action::Right,
                Key::Enter => Action::Accept,
                Key::Space => Action::Select,
                Key::PageUp => Action::LeftTrigger,
                Key::PageDown => Action::RightTrigger,
                Key::Escape => {
                    self.end_select();
                    return true;
                }
                _ => return true,
            };
            self.select_input(a);
            return true;
        }
        if mods.ctrl && mods.shift {
            let cmd = match key {
                Key::C => Some(Cmd::TermCopy),
                Key::V => Some(Cmd::TermPaste),
                Key::T => Some(Cmd::TermNew),
                Key::W => Some(Cmd::TermCloseTab(self.term_ref().map_or(0, |t| t.active))),
                Key::Space => Some(Cmd::TermSelect),
                Key::K => Some(Cmd::TermKeys),
                Key::H => Some(Cmd::TermHistoryMenu),
                Key::Comma => Some(Cmd::TermSettings),
                _ => None,
            };
            if let Some(c) = cmd {
                self.run(c);
                return true;
            }
        }
        if mods.ctrl && matches!(key, Key::PageUp | Key::PageDown) {
            let Some(t) = self.term_mut() else { return true };
            let n = t.sessions.len().max(1);
            t.active = if key == Key::PageUp { (t.active + n - 1) % n } else { (t.active + 1) % n };
            return true;
        }
        if mods.shift && !mods.ctrl && matches!(key, Key::PageUp | Key::PageDown | Key::Home | Key::End) {
            let sc = match key {
                Key::PageUp => Scroll::PageUp,
                Key::PageDown => Scroll::PageDown,
                Key::Home => Scroll::Top,
                _ => Scroll::Bottom,
            };
            self.with_session(|s| s.term.lock().scroll_display(sc));
            return true;
        }
        // Zoom and app tabs stay with the app.
        if mods.ctrl && !mods.shift && matches!(key, Key::Plus | Key::Equals | Key::Minus | Key::Num0) {
            let d = match key {
                Key::Minus => -1.0,
                Key::Num0 => 0.0,
                _ => 1.0,
            };
            let p = &mut self.cfg.terminal;
            let (lo, hi, _) = crate::config::TERM_FONT;
            p.font_size = if d == 0.0 { 14.0 } else { (p.font_size + d).clamp(lo, hi) };
            self.cfg.save();
            return true;
        }
        if mods.ctrl && key == Key::Tab {
            return false;
        }
        let k = match key {
            Key::Enter => K::Enter,
            Key::Tab => K::Tab,
            Key::Backspace => K::Backspace,
            Key::Escape => K::Esc,
            Key::ArrowUp => K::Up,
            Key::ArrowDown => K::Down,
            Key::ArrowLeft => K::Left,
            Key::ArrowRight => K::Right,
            Key::Home => K::Home,
            Key::End => K::End,
            Key::PageUp => K::PageUp,
            Key::PageDown => K::PageDown,
            Key::Insert => K::Insert,
            Key::Delete => K::Delete,
            Key::F1 => K::F(1),
            Key::F2 => K::F(2),
            Key::F3 => K::F(3),
            Key::F4 => K::F(4),
            Key::F5 => K::F(5),
            Key::F6 => K::F(6),
            Key::F7 => K::F(7),
            Key::F8 => K::F(8),
            Key::F9 => K::F(9),
            Key::F10 => K::F(10),
            Key::F11 => K::F(11),
            Key::F12 => K::F(12),
            // Letters and the like come as text; with Ctrl they don't.
            _ if mods.ctrl => match key_char(key) {
                Some(c) => K::Char(c),
                None => return true,
            },
            _ => return true,
        };
        self.term_key(k, mods);
        true
    }

    // ------------------------------------------------------------ clicks

    pub fn terminal_act(&mut self, act: Act) {
        self.focus = Focus::Panes;
        match act {
            Act::Tab(i) => self.run(Cmd::TermSwitch(i)),
            Act::CloseTab(i) => self.run(Cmd::TermCloseTab(i)),
            Act::New => self.run(Cmd::TermNew),
            Act::Keys => self.run(Cmd::TermKeys),
            Act::History => self.run(Cmd::TermHistoryMenu),
            Act::Saved => self.run(Cmd::TermSavedMenu),
            Act::Settings => self.run(Cmd::TermSettings),
            Act::Menu => self.terminal_menu(),
            Act::Focus => {}
            Act::Selected(s) => self.text_clip = Some(s),
            Act::Paste => self.run(Cmd::TermPaste),
        }
    }

    // ------------------------------------------------------------ menus

    pub fn terminal_menu(&mut self) {
        self.term_hist = super::shell_history(&self.cfg.terminal, 60);
        let Some(t) = self.term_ref() else { return };
        let has = !t.sessions.is_empty();
        let sel = t.cur().is_some_and(|s| s.term.lock().selection.is_some());
        let cwd = t.cur().and_then(|s| s.cwd()).is_some();
        let en = |mut i: MenuItem, on: bool| {
            i.enabled = on;
            i
        };
        let items = vec![
            en(item(tr("Run command…"), Cmd::TermRunAsk), has),
            en(item(tr("Type text…"), Cmd::TermTypeAsk), has),
            en(item(tr("Command history"), Cmd::Submenu(SubMenu::TermHistory)), has),
            en(item(tr("Saved commands"), Cmd::Submenu(SubMenu::TermSaved)), has),
            en(item(tr("Launch a program"), Cmd::Submenu(SubMenu::TermLaunch)), has),
            en(item(tr("Quick keys"), Cmd::TermKeys), has),
            en(item(tr("Interrupt (Ctrl+C)"), Cmd::TermInterrupt), has),
            en(item(tr("Choose text to copy"), Cmd::TermSelect), has),
            en(item(tr("Copy"), Cmd::TermCopy), sel),
            en(item(tr("Paste"), Cmd::TermPaste), has),
            en(item(tr("Clear the screen"), Cmd::TermClear), has),
            en(item(tr("Open this folder in the panel"), Cmd::TermShowFolder), cwd),
            item(tr("Terminal tabs"), Cmd::Submenu(SubMenu::TermTabs)),
            item(tr("Terminal settings…"), Cmd::TermSettings),
            item(tr("Back to files"), Cmd::TermHide),
            item(tr("Close the terminal"), Cmd::TermClose),
        ];
        self.overlay = Some(Overlay::Menu(Menu::new(tr("Terminal"), items)));
    }

    /// History or saved commands as a menu of their own.
    fn open_list_menu(&mut self, kind: SubMenu) {
        if kind == SubMenu::TermHistory {
            self.term_hist = super::shell_history(&self.cfg.terminal, 60);
        }
        let m = self.term_submenu(kind);
        self.overlay = Some(Overlay::Menu(m));
    }

    pub fn term_submenu(&self, kind: SubMenu) -> Menu {
        let (title, items) = match kind {
            SubMenu::TermHistory => {
                let mut v: Vec<MenuItem> = self.term_hist.iter().enumerate().map(|(i, c)| item(c, Cmd::Submenu(SubMenu::TermEntry(false, i)))).collect();
                if v.is_empty() {
                    v.push(MenuItem { enabled: false, ..item(tr("No commands yet"), Cmd::TermHide) });
                }
                (tr("Command history").to_string(), v)
            }
            SubMenu::TermSaved => {
                let mut v = vec![item(tr("Save a new command…"), Cmd::TermSaveAsk)];
                v.extend(self.cfg.terminal.saved.iter().enumerate().map(|(i, c)| item(c, Cmd::Submenu(SubMenu::TermEntry(true, i)))));
                (tr("Saved commands").to_string(), v)
            }
            SubMenu::TermEntry(saved, i) => {
                let text = if saved { self.cfg.terminal.saved.get(i) } else { self.term_hist.get(i) }.cloned().unwrap_or_default();
                let mut v = vec![
                    item(tr("Run"), Cmd::TermRun(text.clone())),
                    item(tr("Edit and run…"), Cmd::TermEdit(text.clone())),
                    item(tr("Type without running"), Cmd::TermType(text.clone())),
                ];
                if saved {
                    v.push(item(tr("Remove from saved"), Cmd::TermForget(i)));
                } else {
                    let known = self.cfg.terminal.saved.contains(&text);
                    v.push(MenuItem { enabled: !known, ..item(tr("Save"), Cmd::TermSave(text.clone())) });
                }
                (text, v)
            }
            SubMenu::TermLaunch => {
                let mut v: Vec<MenuItem> = super::installed(keys::LAUNCHERS).into_iter().map(|p| item(p, Cmd::TermRun(p.to_string()))).collect();
                if v.is_empty() {
                    v.push(MenuItem { enabled: false, ..item(tr("Nothing found"), Cmd::TermHide) });
                }
                (tr("Launch a program").to_string(), v)
            }
            SubMenu::TermTabs => {
                let t = self.term_ref();
                let active = t.map_or(0, |t| t.active);
                let mut v: Vec<MenuItem> = t
                    .map(|t| t.sessions.iter().enumerate().map(|(i, s)| toggle(&s.title, Cmd::TermSwitch(i), i == active)).collect())
                    .unwrap_or_default();
                for it in &mut v {
                    it.keep_open = false;
                }
                v.push(item(tr("New terminal tab"), Cmd::TermNew));
                v.push(MenuItem { enabled: t.is_some_and(|t| !t.sessions.is_empty()), ..item(tr("Close terminal tab"), Cmd::TermCloseTab(active)) });
                (tr("Terminal tabs").to_string(), v)
            }
            _ => (String::new(), Vec::new()),
        };
        Menu { kind: Some(kind), ..Menu::new(title, items) }
    }

    // ------------------------------------------------------------ commands

    pub fn terminal_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::TermOpen(dir) => self.open_terminal(dir),
            Cmd::TermNew => {
                let dir = self.term_ref().and_then(|t| t.cur()).and_then(|s| s.cwd());
                let cwd = self.term_dir(dir);
                let (ctx, prefs) = (self.ctx.clone(), self.cfg.terminal.clone());
                let t = self.tabs[self.tab].term.get_or_insert_with(|| Box::new(Terminal::new()));
                t.shown = true;
                if let Err(e) = t.open(&prefs, &cwd, &ctx) {
                    self.toast(format!("{}: {e}", tr("Failed")), true);
                }
                self.focus = Focus::Panes;
            }
            Cmd::TermCloseTab(i) => {
                if let Some(t) = self.term_mut() {
                    t.close(i);
                    if t.sessions.is_empty() {
                        self.tabs[self.tab].term = None;
                    }
                }
            }
            Cmd::TermSwitch(i) => {
                if let Some(t) = self.term_mut() {
                    if i < t.sessions.len() {
                        t.active = i;
                        t.selecting = false;
                    }
                }
            }
            Cmd::TermHide => {
                if let Some(t) = self.term_mut() {
                    t.shown = false;
                }
            }
            Cmd::TermClose => self.tabs[self.tab].term = None,
            Cmd::TermRun(c) => {
                super::remember(&c);
                self.with_session(|s| s.input(format!("{c}\r").into_bytes()));
            }
            Cmd::TermType(c) => self.with_session(|s| s.input(c.into_bytes())),
            Cmd::TermEdit(c) => self.overlay = Some(Overlay::Osk(Osk::new(tr("Run command"), &c, OskPurpose::TermRun))),
            Cmd::TermRunAsk => self.overlay = Some(Overlay::Osk(Osk::new(tr("Run command"), "", OskPurpose::TermRun))),
            Cmd::TermTypeAsk => self.overlay = Some(Overlay::Osk(Osk::new(tr("Type text"), "", OskPurpose::TermType))),
            Cmd::TermSaveAsk => self.overlay = Some(Overlay::Osk(Osk::new(tr("Save a command"), "", OskPurpose::TermSave))),
            Cmd::TermSave(c) => {
                if !c.trim().is_empty() && !self.cfg.terminal.saved.contains(&c) {
                    self.cfg.terminal.saved.push(c);
                    self.cfg.save();
                    self.toast(tr("Saved"), false);
                }
            }
            Cmd::TermForget(i) => {
                if i < self.cfg.terminal.saved.len() {
                    self.cfg.terminal.saved.remove(i);
                    self.cfg.save();
                }
            }
            Cmd::TermInterrupt => self.term_key(K::Char('c'), Mods::CTRL),
            Cmd::TermClear => self.term_key(K::Char('l'), Mods::CTRL),
            Cmd::TermSelect => self.begin_select(),
            Cmd::TermCopy => {
                let text = self.term_ref().and_then(|t| t.cur()).and_then(|s| s.term.lock().selection_to_string()).filter(|s| !s.is_empty());
                if let Some(text) = text {
                    self.ctx.copy_text(text.clone());
                    self.text_clip = Some(text);
                    self.toast(tr("Copied"), false);
                }
            }
            Cmd::TermPaste => {
                if let Some(text) = self.clipboard_text() {
                    self.with_session(|s| s.paste(&text));
                }
            }
            Cmd::TermShowFolder => {
                let Some(dir) = self.term_ref().and_then(|t| t.cur()).and_then(|s| s.cwd()) else { return };
                if let Some(t) = self.term_mut() {
                    t.shown = false;
                }
                let cfg = self.cfg.clone();
                self.pane_mut().navigate(crate::vfs::Location::Local(dir), None, &cfg);
                self.focus = Focus::Panes;
            }
            Cmd::TermKeys => {
                let app = self.term_ref().and_then(|t| t.cur()).and_then(|s| s.foreground());
                let (name, mut v) = keys::for_app(app.as_deref());
                // The program's own keys, then the ones every program knows.
                for k in keys::COMMON {
                    if !v.iter().any(|x| x.label == k.label) {
                        v.push(*k);
                    }
                }
                let title = match name {
                    "Shell" => tr("Quick keys").to_string(),
                    "Program" => format!("{} · {}", tr("Quick keys"), app.unwrap_or_default()),
                    n => format!("{} · {n}", tr("Quick keys")),
                };
                self.overlay = Some(Overlay::TermKeys(QuickKeys { title, keys: v, cursor: 0 }));
            }
            Cmd::TermHistoryMenu => self.open_list_menu(SubMenu::TermHistory),
            Cmd::TermSavedMenu => self.open_list_menu(SubMenu::TermSaved),
            Cmd::TermSettings => self.overlay = Some(Overlay::TermSettings(TermSettings { row: trow::SCHEME })),
            _ => {}
        }
    }

    /// The command typed on the on-screen keyboard.
    pub fn term_osk(&mut self, purpose: OskPurpose, text: String) {
        match purpose {
            OskPurpose::TermRun if !text.trim().is_empty() => self.run(Cmd::TermRun(text)),
            OskPurpose::TermType => self.run(Cmd::TermType(text)),
            OskPurpose::TermSave => self.run(Cmd::TermSave(text.trim().to_string())),
            _ => {}
        }
    }

    // ------------------------------------------------------------ quick keys

    pub fn quick_key(&mut self, i: usize, close: bool) {
        let Some(Overlay::TermKeys(q)) = self.overlay.as_mut() else { return };
        let Some(k) = q.keys.get(i).copied() else { return };
        q.cursor = i;
        if close {
            self.overlay = None;
        }
        match k.send {
            keys::Send::Key(k, m) => self.term_key(k, m),
            keys::Send::Text(t) => self.with_session(|s| s.input(t.as_bytes().to_vec())),
        }
    }

    pub fn term_keys_input(&mut self, a: Action) {
        let Some(Overlay::TermKeys(q)) = self.overlay.as_mut() else { return };
        let n = q.keys.len();
        let c = q.cursor;
        match a {
            Action::Left => q.cursor = c.saturating_sub(1),
            Action::Right => q.cursor = (c + 1).min(n.saturating_sub(1)),
            Action::Up => q.cursor = c.saturating_sub(QK_COLS),
            Action::Down => q.cursor = if c + QK_COLS < n { c + QK_COLS } else { c },
            Action::Accept => self.quick_key(c, false),
            Action::Select => self.quick_key(c, true),
            Action::Back | Action::View | Action::Actions | Action::Menu => self.overlay = None,
            _ => {}
        }
    }

    // ------------------------------------------------------------ settings

    pub fn term_settings_input(&mut self, a: Action) {
        let Some(Overlay::TermSettings(st)) = self.overlay.as_mut() else { return };
        match a {
            Action::Up => st.row = st.row.saturating_sub(1),
            Action::Down => st.row = (st.row + 1).min(trow::CLOSE),
            Action::Left | Action::Right => {
                let row = st.row;
                self.term_settings_step(row, if a == Action::Left { -1 } else { 1 });
            }
            Action::Accept => {
                let row = st.row;
                self.term_settings_step(row, 0);
            }
            Action::Back | Action::Menu => self.overlay = None,
            _ => {}
        }
    }

    /// ←/→ (d = ±1) or ✕ (d = 0) on a settings row.
    pub fn term_settings_step(&mut self, row: usize, d: isize) {
        if let Some(Overlay::TermSettings(st)) = self.overlay.as_mut() {
            st.row = row;
        }
        let cycle = |n: usize, i: usize, d: isize| (i as isize + if d == 0 { 1 } else { d }).rem_euclid(n as isize) as usize;
        let p = &mut self.cfg.terminal;
        match row {
            trow::SCHEME => {
                let names = super::scheme::names();
                let i = names.iter().position(|n| *n == p.scheme).unwrap_or(0);
                p.scheme = names[cycle(names.len(), i, d)].to_string();
            }
            trow::FONT => {
                let v = &mut self.cfg.viewer;
                let mut all: Vec<Option<String>> = vec![None];
                all.extend(crate::fonts::mono_families().iter().map(|(f, _)| Some(f.clone())));
                let i = all.iter().position(|f| *f == v.font).unwrap_or(0);
                v.font = all[cycle(all.len(), i, d)].clone();
                crate::fonts::install_with(&self.ctx, v.font.as_deref());
            }
            trow::SIZE => {
                let (lo, hi, st) = crate::config::TERM_FONT;
                p.font_size = (p.font_size + if d == 0 { st } else { d as f32 * st }).clamp(lo, hi);
            }
            trow::SHELL => {
                let mut all: Vec<Option<String>> = vec![None];
                all.extend(super::shells().into_iter().map(Some));
                let i = all.iter().position(|s| *s == p.shell).unwrap_or(0);
                p.shell = all[cycle(all.len(), i, d)].clone();
            }
            trow::CURSOR => {
                p.cursor = cycle(3, p.cursor as usize, d) as u8;
                self.apply_term_config();
            }
            trow::SCROLLBACK => {
                let all = crate::config::SCROLLBACK;
                let i = all.iter().position(|s| *s == p.scrollback).unwrap_or(2);
                p.scrollback = all[cycle(all.len(), i, d)];
                self.apply_term_config();
            }
            trow::HERE => p.here = !p.here,
            _ => {
                self.overlay = None;
            }
        }
        self.cfg.save();
    }

    /// Cursor and scrollback into the running shells.
    fn apply_term_config(&mut self) {
        let c = super::term_config(&self.cfg.terminal);
        for t in self.tabs.iter().filter_map(|t| t.term.as_deref()) {
            for s in &t.sessions {
                s.term.lock().set_options(c.clone());
            }
        }
    }

    // ------------------------------------------------------------ hints

    pub fn terminal_hints(&self) -> Vec<(Action, &'static str)> {
        let Some(t) = self.term_ref() else { return Vec::new() };
        if t.sessions.is_empty() {
            return vec![(Action::Accept, tr("New terminal tab")), (Action::Back, tr("Close"))];
        }
        if t.selecting {
            return vec![
                (Action::Up, tr("Move")),
                (Action::Select, tr("Start here")),
                (Action::View, tr("Whole lines")),
                (Action::Accept, tr("Copy")),
                (Action::Back, tr("Cancel")),
            ];
        }
        vec![
            (Action::Accept, "Enter"),
            (Action::Back, tr("Esc · hold: leave")),
            (Action::Select, tr("Run command")),
            (Action::View, tr("Quick keys")),
            (Action::LeftTrigger, tr("Scroll")),
            (Action::Actions, tr("Actions")),
        ]
    }
}

/// The character a letter or digit key stands for (for Ctrl combinations).
fn key_char(k: Key) -> Option<char> {
    let name = k.name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(c.to_ascii_lowercase()),
        _ => match k {
            Key::Space => Some(' '),
            Key::OpenBracket => Some('['),
            Key::CloseBracket => Some(']'),
            Key::Backslash => Some('\\'),
            Key::Minus => Some('-'),
            Key::Slash => Some('_'),
            _ => None,
        },
    }
}
