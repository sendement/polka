//! Mouse extras: dragging items onto a folder, an archive, the other pane
//! or the sidebar, and selecting with a rectangle.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use egui::{Order, Vec2, vec2};

use crate::app::{App, Cmd, Focus};
use crate::i18n::{tr, trn};
use crate::vfs::{Dest, Location};

/// Items being dragged.
pub struct Drag {
    pub sources: Vec<Location>,
    pub label: String,
}

/// A selection rectangle being drawn in a pane, in content coordinates
/// (so it stays put while the list scrolls).
pub struct Band {
    pub start: Vec2,
    /// What was selected before (kept with Ctrl).
    pub base: HashSet<String>,
}

/// Can items be dropped onto this entry? Folders, and archives Polka can
/// write into.
pub fn drop_target(e: &crate::vfs::Entry, local: bool) -> bool {
    e.is_dir || (local && crate::archive_rw::writable_name(&e.name))
}

impl App {
    pub fn drag_start(&mut self, pane: usize, i: usize) {
        let tab = self.tab;
        self.tabs[tab].active = pane;
        self.focus = Focus::Panes;
        let p = &mut self.tabs[tab].panes[pane];
        let Some(name) = p.entries.get(i).map(|e| e.name.clone()) else { return };
        // Dragging something unselected drags just that.
        if !p.selected.contains(&name) {
            p.selected.clear();
            p.set_cursor(i);
        }
        let targets = p.targets();
        if targets.is_empty() {
            return;
        }
        let sources: Vec<Location> = targets.iter().map(|e| p.entry_loc(e)).collect();
        let label = match targets.len() {
            1 => targets[0].name.clone(),
            n => trn("%n items", n),
        };
        self.drag = Some(Drag { sources, label });
    }

    /// Dropped on a pane: onto one of its entries, or anywhere else in it.
    pub fn drop_on_pane(&mut self, pane: usize, entry: Option<usize>) {
        let p = &self.tabs[self.tab].panes[pane];
        let local = p.loc.local_path().is_some();
        let dest = match entry.and_then(|i| p.entries.get(i)).filter(|e| drop_target(e, local)) {
            Some(e) if e.is_dir => p.entry_loc(e).dest(),
            Some(e) => p.entry_path(e).and_then(|a| Location::Archive { archive: a, inner: String::new() }.dest()),
            None => p.loc.dest(),
        };
        self.tabs[self.tab].active = pane;
        match dest {
            Some(d) => self.drop_to(d),
            None => {
                self.drag = None;
                self.toast(tr("Can't put anything here"), true);
            }
        }
    }

    /// Dropped on a sidebar entry: the trash throws away, places receive.
    pub fn drop_on_side(&mut self, item: usize) {
        let Some(it) = self.side_items.get(item).cloned() else { return };
        let trash = dirs::data_dir().map(|d| d.join("Trash/files"));
        if it.loc.local_path().is_some_and(|p| Some(p) == trash.as_deref()) {
            let Some(d) = self.drag.take() else { return };
            let paths: Vec<PathBuf> = d.sources.iter().filter_map(|l| l.local_path().map(|p| p.to_path_buf())).collect();
            if paths.len() == d.sources.len() {
                self.run(Cmd::TrashNow(paths));
            } else {
                self.toast(tr("Only files on disk can go to the trash"), true);
            }
            return;
        }
        match it.loc.dest().filter(|_| !it.usage) {
            Some(d) => self.drop_to(d),
            None => self.drag = None,
        }
    }

    fn drop_to(&mut self, dest: Dest) {
        let Some(d) = self.drag.take() else { return };
        let Dest::Dir(dir) = &dest else {
            // Into an archive: always a copy.
            return self.request_transfer(d.sources, dest, false);
        };
        let local: Vec<&std::path::Path> = d.sources.iter().filter_map(|l| l.local_path()).collect();
        let all_local = local.len() == d.sources.len();
        // Dropped back where it lies: nothing to do.
        if all_local && local.iter().all(|s| s.parent() == Some(dir.as_path())) {
            return;
        }
        if local.iter().any(|s| dir.starts_with(s)) {
            return self.toast(tr("A folder can't go inside itself"), true);
        }
        // Ctrl copies, Shift moves; otherwise a move within one disk.
        let m = self.ctx.input(|i| i.modifiers);
        let same_disk = || {
            let dev = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.dev()).ok();
            local.first().and_then(|s| dev(s)).is_some_and(|a| dev(dir) == Some(a))
        };
        let mv = !m.command && all_local && (m.shift || same_disk());
        self.request_transfer(d.sources, dest, mv);
    }

    /// The dragged items follow the pointer; letting go anywhere else drops
    /// nothing.
    pub fn draw_drag(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.drag else { return };
        if !ctx.input(|i| i.pointer.primary_down()) {
            self.drag = None;
            return;
        }
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let Some(p) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let m = ctx.input(|i| i.modifiers);
        let how = if m.command {
            tr("Copy")
        } else if m.shift {
            tr("Move")
        } else {
            ""
        };
        egui::Area::new("drag-ghost".into()).order(Order::Tooltip).fixed_pos(p + vec2(16.0, 12.0)).interactable(false).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&d.label).strong());
                    if !how.is_empty() {
                        ui.label(egui::RichText::new(how).weak());
                    }
                });
            });
        });
        ctx.request_repaint();
    }
}
