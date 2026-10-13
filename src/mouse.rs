//! Mouse extras: dragging items onto a folder, an archive, the other pane
//! or the sidebar, and selecting with a rectangle.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use egui::{Order, Vec2, pos2, vec2};

use crate::app::{App, Cmd, Focus};
use crate::i18n::{tr, trn};
use crate::vfs::{Dest, Location};

/// Items being dragged.
pub struct Drag {
    pub sources: Vec<Location>,
    pub label: String,
    /// Up to three of the items, for the icons following the pointer.
    pub shown: Vec<(crate::vfs::Entry, Option<PathBuf>)>,
    /// Where they lie (tab, pane), drawn faded there meanwhile.
    pub from: (usize, usize),
    pub names: HashSet<String>,
    /// The stack's place (trailing the pointer) and its sway: angle, spin.
    pub pos: Option<egui::Pos2>,
    pub angle: f32,
    pub spin: f32,
    /// Pointer last frame and its smoothed sideways speed.
    pub last: Option<egui::Pos2>,
    pub vx: f32,
}

/// Size of the icons following the pointer.
const ICON: f32 = 48.0;

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
        let shown = targets.iter().take(3).map(|e| (e.clone(), p.entry_path(e))).collect();
        let names = targets.iter().map(|e| e.name.clone()).collect();
        self.drag = Some(Drag { sources, label, shown, from: (tab, pane), names, pos: None, angle: 0.0, spin: 0.0, last: None, vx: 0.0 });
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
                self.drop_done();
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
            self.drop_done();
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
        self.drop_done();
        self.request_transfer(d.sources, dest, mv);
    }

    /// Dropped: the selection is done with, and the cursor stays hidden
    /// instead of jumping to the top of the list in the items' place.
    fn drop_done(&mut self) {
        for p in &mut self.tabs[self.tab].panes {
            p.selected.clear();
            p.cursor_hidden = true;
        }
    }

    /// The dragged items follow the pointer as a little stack that trails
    /// behind and sways on a spring; letting go anywhere else drops nothing.
    pub fn draw_drag(&mut self, ctx: &egui::Context) {
        if self.drag.is_none() {
            return;
        }
        if !ctx.input(|i| i.pointer.primary_down()) {
            self.drag = None;
            return;
        }
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let Some(p) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let (dt, m) = ctx.input(|i| (i.stable_dt.clamp(0.001, 0.05), i.modifiers));
        // Icons first (they need `self.icons`), then the motion.
        let shown = self.drag.as_ref().map(|d| d.shown.clone()).unwrap_or_default();
        let thumbs = self.cfg.thumbnails;
        let px = ((ICON * ctx.pixels_per_point()).ceil() as u16).max(32);
        let pics: Vec<(Option<String>, Option<(String, f32)>)> = shown
            .iter()
            .map(|(e, path)| {
                let thumb = match (path, e.mtime) {
                    (Some(p), Some(t)) if thumbs && !e.is_dir => self.icons.thumb(p, t),
                    _ => None,
                };
                let icon = if thumb.is_some() { None } else { self.icons.for_entry(e, path.as_deref(), px) };
                (icon, thumb)
            })
            .collect();
        let accent = ctx.style_of(ctx.theme()).visuals.selection.bg_fill;
        let Some(d) = self.drag.as_mut() else { return };
        // The stack trails the pointer a little…
        let target = p + vec2(10.0, 14.0);
        let pos = d.pos.map_or(target, |q| q + (target - q) * (1.0 - (-dt * 20.0).exp()));
        d.pos = Some(pos);
        // …and tilts against the motion, swinging back past upright when it
        // stops: a damped spring.
        // The pointer's own speed, smoothed (egui's needs a dense history).
        let vx = d.last.map_or(0.0, |l| (p.x - l.x) / dt);
        d.last = Some(p);
        d.vx += (vx - d.vx) * (1.0 - (-dt * 30.0).exp());
        let want = (-d.vx * 0.0012).clamp(-0.5, 0.5);
        let acc = 170.0 * (want - d.angle) - 9.0 * d.spin;
        d.spin += acc * dt;
        d.angle = (d.angle + d.spin * dt).clamp(-0.8, 0.8);
        let (angle, count, label) = (d.angle, d.sources.len(), d.label.clone());
        let how = if m.command {
            tr("Copy")
        } else if m.shift {
            tr("Move")
        } else {
            ""
        };
        egui::Area::new("drag-ghost".into()).order(Order::Tooltip).fixed_pos(pos).interactable(false).show(ctx, |ui| {
            let (area, _) = ui.allocate_exact_size(vec2(ICON + 40.0, ICON + 46.0), egui::Sense::hover());
            let n = pics.len();
            // Back to front, each a bit lower right and swinging a bit less.
            for (k, (icon, thumb)) in pics.into_iter().enumerate().rev() {
                let r = egui::Rect::from_min_size(area.min + vec2(k as f32 * 7.0, k as f32 * 5.0), egui::Vec2::splat(ICON));
                let a = angle * (1.0 - k as f32 * 0.18);
                let tint = egui::Color32::WHITE.gamma_multiply(if k == 0 { 0.95 } else { 0.8 });
                // Hung from the top middle, like from the pointer.
                let pivot = vec2(0.5, 0.0);
                if let Some((uri, aspect)) = thumb {
                    let size = if aspect >= 1.0 { vec2(ICON, ICON / aspect) } else { vec2(ICON * aspect, ICON) };
                    let r = egui::Rect::from_center_size(r.center(), size);
                    egui::Image::new(uri).corner_radius(3).rotate(a, pivot).tint(tint).paint_at(ui, r);
                } else if let Some(u) = icon {
                    egui::Image::new(u).rotate(a, pivot).tint(tint).paint_at(ui, r);
                }
            }
            if count > 1 {
                let c = area.min + vec2(ICON + (n.saturating_sub(1)) as f32 * 7.0 - 2.0, 4.0);
                ui.painter().circle_filled(c, 11.0, accent);
                ui.painter().text(c, egui::Align2::CENTER_CENTER, count.to_string(), egui::FontId::proportional(12.0), egui::Color32::WHITE);
            }
            let text = if how.is_empty() { label } else { format!("{label} · {how}") };
            let font = egui::FontId::proportional(12.5);
            let g = ui.painter().layout_no_wrap(text, font, ui.visuals().strong_text_color());
            let chip = egui::Rect::from_min_size(pos2(area.min.x, area.min.y + ICON + 18.0), g.size() + vec2(12.0, 6.0));
            ui.painter().rect(chip, 6.0, ui.visuals().window_fill, ui.visuals().window_stroke, egui::StrokeKind::Inside);
            ui.painter().galley(chip.min + vec2(6.0, 3.0), g, ui.visuals().strong_text_color());
        });
        ctx.request_repaint();
    }
}
