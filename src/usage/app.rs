//! The app side of the disk usage page: opening it, the pad and the △ menu.

use std::path::PathBuf;

use super::draw::{Act, item_name, mode_name};
use super::{Kind, Mode, State, Target, Usage};
use crate::app::{App, Cmd, Focus, Menu, MenuItem, Overlay, item};
use crate::i18n::tr;
use crate::input::Action;
use crate::vfs::Location;

impl App {
    /// The page is what the current tab shows.
    pub fn usage_shown(&self) -> bool {
        let t = self.cur_tab();
        t.viewer.is_none() && t.usage.as_ref().is_some_and(|u| u.shown)
    }

    fn usage_mut(&mut self) -> Option<&mut Usage> {
        self.tabs[self.tab].usage.as_deref_mut()
    }

    /// What the start screen offers: home, this folder, the disks.
    pub fn usage_targets(&self) -> Vec<Target> {
        let mut v = Vec::new();
        let home = dirs::home_dir().unwrap_or_else(|| "/".into());
        v.push(Target { label: tr("Home folder").into(), path: home.clone(), icon: &["user-home", "folder-home", "folder"], space: None });
        if let Some(here) = self.pane().loc.local_path().filter(|p| *p != home && *p != std::path::Path::new("/")) {
            v.push(Target { label: tr("This folder").into(), path: here.to_path_buf(), icon: &["folder-open", "folder"], space: None });
        }
        for m in super::mounts() {
            let removable = ["/run/media/", "/media/", "/mnt/"].iter().any(|p| m.path.starts_with(p));
            let label = if m.path == std::path::Path::new("/") {
                tr("System disk").to_string()
            } else {
                m.path.file_name().map_or_else(|| m.path.display().to_string(), |n| n.to_string_lossy().into_owned())
            };
            let icon: &'static [&'static str] = if removable { &["drive-removable-media", "drive-harddisk", "media-flash"] } else { &["drive-harddisk-root", "drive-harddisk", "folder-root"] };
            v.push(Target { label: format!("{label}  ·  {}", m.fstype), path: m.path, icon, space: Some((m.total, m.used)) });
        }
        v
    }

    /// The page in this tab (a new tab if a file is shown here); with a
    /// path, measuring starts right away.
    pub fn open_usage(&mut self, path: Option<PathBuf>) {
        if self.cur_tab().viewer.is_some() {
            self.run(Cmd::NewTab);
        }
        let targets = self.usage_targets();
        let mode = Mode::from(self.cfg.usage_mode);
        let ctx = self.ctx.clone();
        let u = self.tabs[self.tab].usage.get_or_insert_with(|| Box::new(Usage::new(mode, Vec::new())));
        u.shown = true;
        if let State::Start { targets: t, cursor } = &mut u.state {
            *cursor = u.last_root.as_ref().and_then(|r| targets.iter().position(|x| x.path == *r)).unwrap_or(0);
            *t = targets;
        }
        if let Some(p) = path {
            u.restore = None;
            u.scan(p, &ctx);
        }
        self.focus = Focus::Panes;
        self.range = None;
    }

    /// The start screen again (after a scan or instead of it).
    fn usage_start(&mut self) {
        let targets = self.usage_targets();
        if let Some(u) = self.usage_mut() {
            let cursor = u.last_root.as_ref().and_then(|r| targets.iter().position(|x| x.path == *r)).unwrap_or(0);
            u.state = State::Start { targets, cursor };
        }
    }

    pub fn tick_usage(&mut self) {
        for t in &mut self.tabs {
            if let Some(u) = t.usage.as_deref_mut() {
                u.tick();
            }
        }
    }

    pub fn usage_input(&mut self, a: Action) {
        let ctx = self.ctx.clone();
        let Some(u) = self.usage_mut() else { return };
        let mode = u.mode;
        match &mut u.state {
            State::Start { targets, cursor } => match a {
                Action::Up => *cursor = cursor.saturating_sub(1),
                Action::Down => *cursor = (*cursor + 1).min(targets.len().saturating_sub(1)),
                Action::Home => *cursor = 0,
                Action::End => *cursor = targets.len().saturating_sub(1),
                Action::Accept => {
                    if let Some(t) = targets.get(*cursor) {
                        let p = t.path.clone();
                        u.restore = None;
                        u.scan(p, &ctx);
                    }
                }
                Action::Left => self.focus_sidebar(),
                Action::Back => self.run(Cmd::UsageClose),
                Action::Menu => self.open_app_menu(),
                _ => {}
            },
            State::Scanning(_) => match a {
                Action::Back => self.usage_start(),
                Action::Menu => self.open_app_menu(),
                _ => {}
            },
            State::Ready(r) => {
                let depth = mode.depth();
                let list = mode == Mode::List;
                u.reveal = true;
                match a {
                    Action::Up if list => r.sibling(-1),
                    Action::Down if list => r.sibling(1),
                    Action::Right if list => {
                        r.zoom_in();
                    }
                    Action::Left if list => {
                        r.zoom_out();
                    }
                    Action::Left => r.sibling(-1),
                    Action::Right => r.sibling(1),
                    Action::Down => {
                        r.deeper(depth);
                    }
                    Action::Up => {
                        r.shallower();
                    }
                    Action::Home => r.sibling_edge(false),
                    Action::End => r.sibling_edge(true),
                    Action::Accept | Action::RightTrigger => {
                        if !r.zoom_in() && a == Action::Accept {
                            // A file: shown in its folder.
                            self.run(Cmd::UsageReveal);
                        }
                    }
                    Action::LeftTrigger => {
                        r.zoom_out();
                    }
                    Action::Back => {
                        if !r.zoom_out() {
                            self.usage_start();
                        }
                    }
                    Action::Select | Action::View => {
                        let next = Mode::ALL[(mode.index() as usize + 1) % Mode::ALL.len()];
                        self.run(Cmd::UsageMode(next.index()));
                    }
                    Action::Actions => self.usage_menu(),
                    Action::Menu => self.open_app_menu(),
                    _ => {}
                }
            }
        }
    }

    /// Clicks on the page.
    pub fn usage_act(&mut self, act: Act) {
        self.focus = Focus::Panes;
        let ctx = self.ctx.clone();
        let Some(u) = self.usage_mut() else { return };
        let depth = u.mode.depth();
        u.reveal = true;
        match act {
            Act::Close => self.run(Cmd::UsageClose),
            Act::Mode(m) => self.run(Cmd::UsageMode(m.index())),
            Act::Rescan => self.run(Cmd::UsageRescan),
            Act::Cancel => self.usage_start(),
            Act::Scan(i) => {
                if let State::Start { targets, cursor } = &mut u.state {
                    *cursor = i;
                    if let Some(p) = targets.get(i).map(|t| t.path.clone()) {
                        u.restore = None;
                        u.scan(p, &ctx);
                    }
                }
            }
            Act::Pick(_) | Act::Zoom(_) | Act::Context(_) => {
                let zoom = matches!(act, Act::Zoom(_));
                let context = matches!(act, Act::Context(_));
                let (Act::Pick(p) | Act::Zoom(p) | Act::Context(p)) = act else { return };
                if let Some(r) = u.ready_mut() {
                    r.select(p, depth);
                    if zoom {
                        r.zoom_in();
                    }
                }
                if context {
                    self.usage_menu();
                }
            }
            Act::ZoomOut => {
                if let Some(r) = u.ready_mut() {
                    r.zoom_out();
                }
            }
            Act::Crumb(len) => {
                if let Some(r) = u.ready_mut() {
                    while r.center.len() > len {
                        r.zoom_out();
                    }
                }
            }
        }
    }

    pub fn usage_menu(&mut self) {
        let Some(u) = self.tabs[self.tab].usage.as_deref() else { return };
        let Some(r) = u.ready() else { return };
        let mode = u.mode;
        let mut items: Vec<MenuItem> = Vec::new();
        let title = r.sel_node().map_or_else(|| tr("Disk usage").to_string(), item_name);
        if let Some(n) = r.sel_node() {
            if n.is_dir() {
                items.push(item(tr("Open in the panel"), Cmd::UsageReveal));
                if let Some(root) = u.last_root.as_deref() {
                    items.push(item(tr("New tab"), Cmd::OpenInNewTab(Location::Local(r.path_of(root, &r.sel)))));
                }
                if !n.children.is_empty() {
                    items.push(item(tr("Look inside"), Cmd::UsageZoomIn));
                }
            } else {
                items.push(item(tr("Show in folder"), Cmd::UsageReveal));
            }
        }
        if !r.center.is_empty() {
            items.push(item(tr("Up a level"), Cmd::UsageZoomOut));
        }
        for m in Mode::ALL {
            items.push(MenuItem { checked: Some(m == mode), ..item(mode_name(m), Cmd::UsageMode(m.index())) });
        }
        items.push(item(tr("Measure again"), Cmd::UsageRescan));
        items.push(item(tr("Measure something else"), Cmd::UsageStart));
        items.push(item(tr("Close"), Cmd::UsageClose));
        self.overlay = Some(Overlay::Menu(Menu::new(title, items)));
    }

    pub fn usage_cmd(&mut self, cmd: Cmd) {
        let ctx = self.ctx.clone();
        match cmd {
            Cmd::UsageOpen(p) => self.open_usage(p),
            Cmd::UsageClose => {
                self.tabs[self.tab].usage = None;
            }
            Cmd::UsageStart => self.usage_start(),
            Cmd::UsageMode(i) => {
                let m = Mode::from(i);
                self.cfg.usage_mode = i;
                self.cfg.save();
                if let Some(u) = self.usage_mut() {
                    u.mode = m;
                    u.reveal = true;
                    if let Some(r) = u.ready_mut() {
                        r.fit(m.depth());
                    }
                }
            }
            Cmd::UsageZoomIn => {
                if let Some(r) = self.usage_mut().and_then(|u| u.ready_mut()) {
                    r.zoom_in();
                }
            }
            Cmd::UsageZoomOut => {
                if let Some(r) = self.usage_mut().and_then(|u| u.ready_mut()) {
                    r.zoom_out();
                }
            }
            Cmd::UsageRescan => {
                let Some(u) = self.usage_mut() else { return };
                let Some(root) = u.last_root.clone() else { return };
                if let Some(r) = u.ready() {
                    u.restore = Some((r.tree.names(&r.center), r.tree.names(&r.sel)));
                }
                u.scan(root, &ctx);
            }
            Cmd::UsageReveal => {
                let Some(u) = self.tabs[self.tab].usage.as_deref_mut() else { return };
                let (Some(root), Some(r)) = (u.last_root.clone(), u.ready()) else { return };
                let Some(n) = r.sel_node() else { return };
                let path = r.path_of(&root, &r.sel);
                let (dir, focus) = match n.kind {
                    Kind::Dir => (path, None),
                    Kind::File => (path.parent().map_or_else(|| root.clone(), |p| p.to_path_buf()), Some(item_name(n))),
                    Kind::Rest(_) => (path, None),
                };
                u.shown = false;
                let cfg = self.cfg.clone();
                let pane = self.pane_mut();
                if pane.loc == Location::Local(dir.clone()) {
                    if let Some(i) = focus.and_then(|f| pane.entries.iter().position(|e| e.name == f)) {
                        pane.set_cursor(i);
                    }
                } else {
                    pane.navigate(Location::Local(dir), focus, &cfg);
                }
                self.focus = Focus::Panes;
            }
            _ => {}
        }
    }

    /// The hints bar on the page.
    pub fn usage_hints(&self) -> Vec<(Action, &'static str)> {
        let Some(u) = self.cur_tab().usage.as_deref() else { return Vec::new() };
        match &u.state {
            State::Start { .. } => vec![(Action::Up, tr("Choose")), (Action::Accept, tr("Measure")), (Action::Back, tr("Close"))],
            State::Scanning(_) => vec![(Action::Back, tr("Cancel"))],
            State::Ready(_) if u.mode == Mode::List => vec![
                (Action::Up, tr("Move")),
                (Action::Accept, tr("Look inside")),
                (Action::Select, tr("View")),
                (Action::Actions, tr("Actions")),
                (Action::Back, tr("Up a level")),
            ],
            State::Ready(_) => vec![
                (Action::Left, if u.mode == Mode::Rings { tr("Around the ring") } else { tr("Neighbours") }),
                (Action::Up, tr("Out / in")),
                (Action::Accept, tr("Look inside")),
                (Action::Select, tr("View")),
                (Action::Actions, tr("Actions")),
                (Action::Back, tr("Up a level")),
            ],
        }
    }
}
