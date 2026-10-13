//! The app side of the text viewer: keyboard and gamepad editing, the △
//! menu, saving (also back into archives) and unsaved-changes prompts.

use egui::{Event, Key, Modifiers};

use super::TextState;
use super::edit::{Find, Move};
use crate::app::{App, Cmd, Menu, Osk, OskPurpose, Overlay, item, toggle};
use crate::i18n::tr;
use crate::input::Action;

/// What to do once unsaved changes are dealt with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    CloseViewer,
    CloseTab(usize),
    Quit,
}

impl App {
    pub fn text_mut(&mut self) -> Option<&mut TextState> {
        match self.tabs.get_mut(self.tab)?.viewer.as_mut().map(|v| &mut v.content) {
            Some(super::Content::Text(t)) => Some(t),
            _ => None,
        }
    }

    fn text_ref(&self) -> Option<&TextState> {
        match self.tabs.get(self.tab)?.viewer.as_ref().map(|v| &v.content) {
            Some(super::Content::Text(t)) => Some(t),
            _ => None,
        }
    }

    pub fn text_editing(&self) -> bool {
        self.text_ref().is_some_and(|t| t.ed.editing)
    }

    fn md_shown(&self) -> bool {
        self.text_ref().is_some_and(|t| super::draw::md_rendered(t, &self.cfg.viewer))
    }

    fn read_only_toast(&mut self) {
        self.toast(tr("This file can't be edited here"), true);
    }

    /// Gamepad in a text viewer. Returns false to fall back to the generic
    /// viewer handling.
    pub fn viewer_text_input(&mut self, a: Action) -> bool {
        let md = self.md_shown();
        let Some(t) = self.text_mut() else { return false };
        let finding = t.ed.find.as_ref().is_some_and(|f| !f.matches.is_empty());
        if md {
            let step = t.row_h * 3.0;
            let page = t.page_rows as f32 * t.row_h * 0.9;
            let y = t.md_scroll;
            let to = match a {
                Action::Up => y - step,
                Action::Down => y + step,
                Action::LeftTrigger => y - page,
                Action::RightTrigger => y + page,
                Action::Home => 0.0,
                Action::End => f32::MAX / 4.0,
                Action::View => {
                    self.run(Cmd::ViewerToggleMd);
                    return true;
                }
                Action::Accept => {
                    self.run(Cmd::ViewerEdit);
                    return true;
                }
                _ => return false,
            };
            t.md_scroll_to = Some(to.max(0.0));
            return true;
        }
        match a {
            Action::LeftTrigger | Action::RightTrigger if finding => {
                t.find_step(if a == Action::RightTrigger { 1 } else { -1 });
                true
            }
            Action::Back if t.ed.find.is_some() => {
                t.ed.find = None;
                true
            }
            Action::View if t.doc.lang == Some("Markdown") => {
                self.run(Cmd::ViewerToggleMd);
                true
            }
            _ if !t.ed.editing => {
                if a == Action::Accept {
                    self.run(Cmd::ViewerEdit);
                    return true;
                }
                false
            }
            // Editing: the D-pad moves the caret; holding □ selects.
            Action::Up | Action::Down | Action::Left | Action::Right | Action::LeftTrigger | Action::RightTrigger | Action::Home | Action::End => {
                let select = self.edit_hold.is_some();
                if select {
                    self.edit_hold = Some(true);
                }
                let m = match a {
                    Action::Up => Move::Up,
                    Action::Down => Move::Down,
                    Action::Left => Move::Left,
                    Action::Right => Move::Right,
                    Action::LeftTrigger => Move::PageUp,
                    Action::RightTrigger => Move::PageDown,
                    Action::Home => Move::DocStart,
                    _ => Move::DocEnd,
                };
                if let Some(t) = self.text_mut() {
                    t.move_caret(m, select);
                }
                true
            }
            Action::Accept => {
                self.run(Cmd::ViewerInsertAsk);
                true
            }
            Action::Select => {
                self.edit_hold = Some(false);
                true
            }
            Action::Back => {
                t.ed.editing = false;
                t.ed.anchor = None;
                true
            }
            _ => false,
        }
    }

    /// □ released while editing: a tap (no movement) erases.
    pub fn viewer_select_released(&mut self) {
        if self.edit_hold.take() == Some(false) {
            if let Some(t) = self.text_mut() {
                if !t.backspace(false) && !t.editable() {
                    self.read_only_toast();
                }
            }
        }
    }

    /// △ in a text viewer.
    pub fn viewer_text_menu(&mut self) {
        let wrap = self.cfg.viewer.wrap;
        let numbers = self.cfg.viewer.line_numbers;
        let md_on = self.cfg.viewer.md_rendered;
        let Some(v) = self.tabs[self.tab].viewer.as_ref() else { return };
        let name = v.name.clone();
        let Some(t) = self.text_ref() else { return };
        let has_sel = t.ed.selection().is_some();
        let mut items = Vec::new();
        if t.ed.editing {
            items.push(item(tr("Type text…"), Cmd::ViewerInsertAsk));
            items.push(item(tr("New line"), Cmd::ViewerNewline));
            items.push(item(tr("Undo"), Cmd::ViewerUndo));
            items.push(item(tr("Redo"), Cmd::ViewerRedo));
            if has_sel {
                items.push(item(tr("Copy"), Cmd::ViewerCopy));
                items.push(item(tr("Cut"), Cmd::ViewerCut));
            }
            items.push(item(tr("Paste"), Cmd::ViewerPaste));
            items.push(item(tr("Select all"), Cmd::ViewerSelectAll));
        } else {
            items.push(item(tr("Edit"), Cmd::ViewerEdit));
            if has_sel {
                items.push(item(tr("Copy"), Cmd::ViewerCopy));
            }
        }
        items.push(item(tr("Find…"), Cmd::ViewerFindAsk));
        if let Some(f) = &t.ed.find {
            items.push(toggle(tr("Match case"), Cmd::ViewerFindOpt(0), f.case));
            items.push(toggle(tr("Whole words"), Cmd::ViewerFindOpt(1), f.word));
            items.push(toggle(tr("Regular expression"), Cmd::ViewerFindOpt(2), f.regex));
        }
        if t.ed.dirty {
            items.push(item(tr("Save"), Cmd::ViewerSave));
        }
        if t.ed.editing {
            items.push(item(tr("Finish editing"), Cmd::ViewerEdit));
        }
        if t.doc.lang == Some("Markdown") {
            items.push(toggle(tr("Markdown as on GitHub"), Cmd::ViewerToggleMd, md_on));
        }
        items.push(toggle(tr("Wrap lines"), Cmd::ViewerWrap, wrap));
        items.push(toggle(tr("Line numbers"), Cmd::ViewerNumbers, numbers));
        items.push(item(tr("Viewer settings…"), Cmd::ViewerSettings));
        items.push(item(tr("Open in another app"), Cmd::OpenExternal));
        items.push(item(tr("Close"), Cmd::CloseViewer));
        self.overlay = Some(Overlay::Menu(Menu::new(&name, items)));
    }

    /// Text viewer commands.
    pub fn viewer_cmd(&mut self, cmd: Cmd) {
        let ctx = self.ctx.clone();
        match cmd {
            Cmd::ViewerEdit => {
                let Some(t) = self.text_mut() else { return };
                if !t.editable() {
                    return self.read_only_toast();
                }
                t.ed.editing = !t.ed.editing;
                if t.ed.editing {
                    // Start at the top of what's on screen.
                    let row = ((t.scroll.y / t.row_h.max(1.0)).ceil() as usize).min(t.rows.len().saturating_sub(1));
                    if !t.rows.is_empty() && t.row_of(t.ed.caret) < row {
                        t.ed.caret = t.at_col(row, 0);
                    }
                    t.ed.reveal = true;
                }
            }
            Cmd::ViewerInsertAsk => {
                let Some(t) = self.text_mut() else { return };
                if !t.editable() {
                    return self.read_only_toast();
                }
                self.overlay = Some(Overlay::Osk(Osk::new(tr("Type text"), "", OskPurpose::ViewerInsert)));
            }
            Cmd::ViewerNewline => {
                if let Some(t) = self.text_mut() {
                    t.newline();
                }
            }
            Cmd::ViewerUndo => {
                if let Some(t) = self.text_mut() {
                    t.undo();
                }
            }
            Cmd::ViewerRedo => {
                if let Some(t) = self.text_mut() {
                    t.redo();
                }
            }
            Cmd::ViewerCopy | Cmd::ViewerCut => {
                let Some(t) = self.text_mut() else { return };
                if let Some(s) = t.selected_text() {
                    if matches!(cmd, Cmd::ViewerCut) {
                        t.backspace(false);
                    }
                    ctx.copy_text(s.clone());
                    self.text_clip = Some(s);
                }
            }
            Cmd::ViewerPaste => {
                // The system clipboard when a tool can read it, else ours.
                let sys = std::process::Command::new("wl-paste")
                    .arg("--no-newline")
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
                if let Some(s) = sys.or_else(|| self.text_clip.clone()) {
                    if let Some(t) = self.text_mut() {
                        if !t.insert(&s) {
                            self.read_only_toast();
                        }
                    }
                }
            }
            Cmd::ViewerSelectAll => {
                if let Some(t) = self.text_mut() {
                    t.select_all();
                }
            }
            Cmd::ViewerFindAsk => {
                let Some(t) = self.text_mut() else { return };
                let seed = t.selected_text().filter(|s| !s.contains('\n')).or_else(|| t.ed.find.as_ref().map(|f| f.query.clone())).unwrap_or_default();
                self.overlay = Some(Overlay::Osk(Osk::new(tr("Find"), &seed, OskPurpose::ViewerFind)));
            }
            Cmd::ViewerFindOpt(i) => {
                if let Some(f) = self.text_mut().and_then(|t| t.ed.find.as_mut()) {
                    match i {
                        0 => f.case = !f.case,
                        1 => f.word = !f.word,
                        _ => f.regex = !f.regex,
                    }
                }
                if let Some(t) = self.text_mut() {
                    t.find_refresh();
                }
            }
            Cmd::ViewerToggleMd => {
                self.cfg.viewer.md_rendered = !self.cfg.viewer.md_rendered;
                self.cfg.save();
                if let Some(t) = self.text_mut() {
                    // Into the GitHub view: leave editing (and the search) first.
                    if t.doc.lang == Some("Markdown") && !t.ed.editing {
                        t.ed.find = None;
                    }
                    t.ed.editing = false;
                }
            }
            Cmd::ViewerSave => self.viewer_save(None),
            Cmd::ViewerSaveThen(then) => self.viewer_save(Some(then)),
            Cmd::ViewerDiscardThen(then) => {
                if let Some(t) = self.text_mut() {
                    t.ed.dirty = false;
                }
                self.finish(then);
            }
            _ => {}
        }
    }

    /// Text typed on the on-screen keyboard.
    pub fn viewer_osk(&mut self, purpose: OskPurpose, raw: String) {
        let Some(t) = self.text_mut() else { return };
        match purpose {
            OskPurpose::ViewerInsert => {
                t.insert(&raw);
            }
            OskPurpose::ViewerFind => {
                let mut f = t.ed.find.take().unwrap_or_default();
                f.query = raw;
                f.typing = false;
                t.ed.find = Some(f);
                t.find_refresh();
                t.find_step(1);
            }
            _ => {}
        }
    }

    /// Write the file; files from archives go back into them.
    fn viewer_save(&mut self, then: Option<Then>) {
        let Some(v) = self.tabs[self.tab].viewer.as_mut() else { return };
        let (path, source) = (v.path.clone(), v.source.clone());
        let super::Content::Text(t) = &mut v.content else { return };
        if !t.editable() {
            return self.read_only_toast();
        }
        match t.save_to(&path) {
            Ok(()) => {
                if let Some((archive, inner)) = source {
                    let plan = crate::archive_rw::Plan { add: vec![(path, inner)], ..Default::default() };
                    self.start(crate::ops::Op::ArchiveEdit { archive, plan, label: tr("Saving") });
                } else {
                    self.toast(tr("Saved"), false);
                    self.reload_all();
                }
                if let Some(then) = then {
                    self.finish(then);
                }
            }
            Err(e) => self.toast(format!("{}: {e}", tr("Failed")), true),
        }
    }

    /// Ask about unsaved changes in tab `tab` before `then`; true if asked.
    pub fn confirm_unsaved(&mut self, tab: usize, then: Then) -> bool {
        let dirty = matches!(self.tabs.get(tab).and_then(|t| t.viewer.as_ref()).map(|v| &v.content), Some(super::Content::Text(t)) if t.ed.dirty);
        if !dirty {
            return false;
        }
        self.tab = tab;
        let name = self.tabs[tab].viewer.as_ref().map(|v| v.name.clone()).unwrap_or_default();
        let items = vec![
            item(tr("Save"), Cmd::ViewerSaveThen(then)),
            item(tr("Don't save"), Cmd::ViewerDiscardThen(then)),
            item(tr("Cancel"), Cmd::Nothing),
        ];
        self.overlay = Some(Overlay::Menu(Menu::new(&format!("{} «{name}»?", tr("Save changes to")), items)));
        true
    }

    fn finish(&mut self, then: Then) {
        match then {
            Then::CloseViewer => self.close_viewer(),
            Then::CloseTab(i) => self.run(Cmd::CloseTabAt(i)),
            Then::Quit => self.run(Cmd::Quit),
        }
    }

    /// A link clicked in rendered Markdown.
    pub fn viewer_link(&mut self, link: String) {
        if let Some(anchor) = link.strip_prefix('#') {
            if let Some(t) = self.text_mut() {
                let key = super::md::slug(anchor);
                if let Some(y) = t.anchors.get(&key).or_else(|| t.anchors.get(anchor)).copied() {
                    t.md_scroll_to = Some(y);
                }
            }
            return;
        }
        let external = link.contains("://") || link.starts_with("mailto:");
        let target = if external {
            None
        } else {
            let dir = self.tabs[self.tab].viewer.as_ref().and_then(|v| v.path.parent().map(|p| p.to_path_buf()));
            let rel = link.split('#').next().unwrap_or(&link).to_string();
            dir.map(|d| d.join(rel)).filter(|p| p.exists())
        };
        match target {
            Some(p) if p.is_file() && self.tabs[self.tab].viewer.as_ref().is_some_and(|v| v.source.is_none()) => {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if super::kind(&p, &name).is_some() {
                    let origin = self.tabs[self.tab].viewer.as_ref().map_or(0, |v| v.origin);
                    let (tab, folder) = (self.tab, self.tabs[self.tab].viewer.as_ref().and_then(|v| v.folder.clone()));
                    self.show_viewer(tab, p, name, origin, Vec::new());
                    if let Some(v) = self.tabs[tab].viewer.as_mut() {
                        v.folder = folder;
                    }
                    return;
                }
                let _ = crate::launch::open(&p);
            }
            _ if external => {
                let mut cmd = std::process::Command::new("xdg-open");
                cmd.arg(&link);
                if let Err(e) = crate::launch::spawn_cmd(cmd) {
                    self.toast(format!("{}: {e}", tr("Failed")), true);
                }
            }
            _ => self.toast(format!("{}: {link}", tr("Failed")), true),
        }
    }

    /// Keyboard in a text viewer. Returns the events it didn't use.
    pub fn editor_keyboard(&mut self, events: Vec<Event>) -> Vec<Event> {
        let md = self.md_shown();
        let mut rest = Vec::new();
        for ev in events {
            if !self.editor_event(&ev, md) {
                rest.push(ev);
            }
        }
        rest
    }

    fn editor_event(&mut self, ev: &Event, md: bool) -> bool {
        let ctx = self.ctx.clone();
        let Some(t) = self.text_mut() else { return false };
        let typing = t.ed.find.as_ref().is_some_and(|f| f.typing);
        // Typing into the search field.
        if typing {
            let f = t.ed.find.as_mut().unwrap();
            match ev {
                Event::Text(s) | Event::Paste(s) => {
                    f.query.push_str(&s.replace('\n', " "));
                    t.find_refresh();
                    let caret = t.ed.selection().map_or(t.ed.caret, |s| s.start);
                    if let Some(i) = t.ed.find.as_ref().and_then(|f| f.nearest(caret)) {
                        t.goto_match(i);
                    }
                    return true;
                }
                Event::Key { key, pressed: true, modifiers: m, .. } => {
                    match key {
                        Key::Escape => t.ed.find = None,
                        Key::Enter | Key::F3 => t.find_step(if m.shift { -1 } else { 1 }),
                        Key::Backspace => {
                            if m.ctrl {
                                let keep = f.query.trim_end().rfind(' ').map_or(0, |i| i + 1);
                                f.query.truncate(keep);
                            } else {
                                f.query.pop();
                            }
                        }
                        Key::C if m.alt => f.case = !f.case,
                        Key::W if m.alt => f.word = !f.word,
                        Key::R if m.alt => f.regex = !f.regex,
                        Key::ArrowDown | Key::Tab => f.typing = false,
                        _ if m.command => return false,
                        _ => {}
                    }
                    return true;
                }
                _ => return false,
            }
        }
        match ev {
            Event::Text(s) if !md => {
                if s.chars().all(|c| !c.is_control() || c == '\n' || c == '\t') {
                    if t.insert(s) {
                        t.ed.editing = true;
                    } else {
                        self.read_only_toast();
                    }
                }
                true
            }
            Event::Paste(s) if !md => {
                if t.insert(s) {
                    t.ed.editing = true;
                } else {
                    self.read_only_toast();
                }
                true
            }
            Event::Copy | Event::Cut => {
                if let Some(s) = t.selected_text() {
                    if matches!(ev, Event::Cut) && !md {
                        t.backspace(false);
                    }
                    ctx.copy_text(s.clone());
                    self.text_clip = Some(s);
                }
                true
            }
            Event::Key { key, pressed: true, modifiers: m, .. } => self.editor_key(*key, *m, md),
            _ => false,
        }
    }

    fn editor_key(&mut self, key: Key, m: Modifiers, md: bool) -> bool {
        // App-wide shortcuts stay with the app.
        if m.command && matches!(key, Key::T | Key::W | Key::Q | Key::Tab | Key::PageUp | Key::PageDown | Key::Plus | Key::Equals | Key::Minus | Key::Num0 | Key::B | Key::L) {
            return false;
        }
        if m.command && matches!(key, Key::C | Key::X | Key::V) {
            return true; // arrive as Copy / Cut / Paste events
        }
        if md {
            // GitHub view: scrolling, search, editing.
            return match key {
                Key::F if m.command => {
                    self.open_find();
                    true
                }
                Key::E if m.command => {
                    self.run(Cmd::ViewerEdit);
                    true
                }
                Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown | Key::Home | Key::End => {
                    let a = match key {
                        Key::ArrowUp => Action::Up,
                        Key::ArrowDown => Action::Down,
                        Key::PageUp => Action::LeftTrigger,
                        Key::PageDown => Action::RightTrigger,
                        Key::Home => Action::Home,
                        _ => Action::End,
                    };
                    self.viewer_text_input(a)
                }
                Key::Escape => {
                    self.request_close_viewer();
                    true
                }
                _ => false,
            };
        }
        let Some(t) = self.text_mut() else { return false };
        let sel = m.shift;
        let word = m.command;
        let mv = |t: &mut TextState, mo: Move| t.move_caret(mo, sel);
        match key {
            Key::ArrowLeft => mv(t, if word { Move::WordLeft } else { Move::Left }),
            Key::ArrowRight => mv(t, if word { Move::WordRight } else { Move::Right }),
            Key::ArrowUp => mv(t, Move::Up),
            Key::ArrowDown => mv(t, Move::Down),
            Key::Home => mv(t, if word { Move::DocStart } else { Move::Home }),
            Key::End => mv(t, if word { Move::DocEnd } else { Move::End }),
            Key::PageUp => mv(t, Move::PageUp),
            Key::PageDown => mv(t, Move::PageDown),
            Key::Backspace | Key::Delete | Key::Enter | Key::Tab => {
                let ok = match key {
                    Key::Backspace => t.backspace(word),
                    Key::Delete => t.delete(word),
                    Key::Enter => t.newline(),
                    _ => t.tab(),
                };
                if ok {
                    t.ed.editing = true;
                } else if !t.editable() {
                    self.read_only_toast();
                }
            }
            Key::A if m.command => t.select_all(),
            Key::Z if m.command && m.shift => {
                t.redo();
            }
            Key::Z if m.command => {
                t.undo();
            }
            Key::Y if m.command => {
                t.redo();
            }
            Key::S if m.command => self.viewer_save(None),
            Key::F if m.command => self.open_find(),
            Key::E if m.command => self.run(Cmd::ViewerEdit),
            Key::F3 | Key::G if key == Key::F3 || m.command => t.find_step(if m.shift { -1 } else { 1 }),
            Key::Escape => {
                if t.ed.selection().is_some() {
                    t.ed.anchor = None;
                } else if t.ed.find.is_some() {
                    t.ed.find = None;
                } else if t.ed.editing {
                    t.ed.editing = false;
                } else {
                    self.request_close_viewer();
                }
            }
            _ => return false,
        }
        true
    }

    /// Ctrl+F: the search bar, seeded with the selection.
    fn open_find(&mut self) {
        let Some(t) = self.text_mut() else { return };
        let seed = t.selected_text().filter(|s| !s.contains('\n'));
        let f = t.ed.find.get_or_insert_with(Find::default);
        if let Some(s) = seed {
            f.query = s;
        }
        f.typing = true;
        t.find_refresh();
    }

    pub fn request_close_viewer(&mut self) {
        let tab = self.tab;
        if !self.confirm_unsaved(tab, Then::CloseViewer) {
            self.close_viewer();
        }
    }
}
