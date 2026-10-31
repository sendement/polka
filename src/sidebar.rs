//! The sidebar's own commands: each place's △ menu, drives (mount,
//! unmount, eject) and emptying the trash.

use std::path::PathBuf;

use crate::app::{App, Cmd, Focus, Menu, MenuItem, Msg, Overlay, SideItem, item, trash_files};
use crate::drives::{Done, Drive, Op};
use crate::i18n::{tr, trn};
use crate::vfs::Location;

impl App {
    /// △ on a sidebar entry.
    pub fn side_menu(&mut self, it: SideItem) {
        let i = self.side_cursor;
        let mut items: Vec<MenuItem> = Vec::new();
        let real = !it.offline() && !it.term;
        if it.term {
            items.push(item(tr("New terminal tab"), Cmd::TermNew));
        }
        if let Some(d) = &it.drive {
            if d.mount.is_none() {
                items.push(item(tr("Mount"), Cmd::DriveMount(i)));
            }
        }
        if real {
            items.push(item(tr("New tab"), Cmd::OpenInNewTab(it.loc.clone())));
        }
        if let Some(d) = &it.drive {
            if d.mount.is_some() {
                items.push(item(tr("Unmount"), Cmd::DriveUnmount(i)));
            }
            if d.removable || d.optical {
                items.push(item(tr("Eject"), Cmd::DriveEject(i)));
            }
        }
        if it.loc.local_path().is_some() && trash_files().as_deref() == it.loc.local_path() {
            items.push(MenuItem { enabled: crate::trashbin::count() > 0, ..item(tr("Empty trash…"), Cmd::EmptyTrashAsk) });
        }
        if let Some((key, online)) = &it.phone {
            items.push(if *online { item(tr("Disconnect"), Cmd::PhoneDisconnect(key.clone())) } else { item(tr("Connect"), Cmd::PhoneOpen(key.clone())) });
        }
        if let Some((id, online)) = &it.net {
            items.push(if *online { item(tr("Disconnect"), Cmd::NetDisconnect(id.clone())) } else { item(tr("Connect"), Cmd::NetConnect(id.clone())) });
            items.push(item(tr("Edit connection…"), Cmd::NetEdit(id.clone())));
            items.push(item(tr("Remove connection…"), Cmd::NetRemoveAsk(id.clone())));
            items.push(item(tr("Add network place…"), Cmd::NetAdd));
        }
        // Any entry can wear its own icon; drives keep theirs by UUID.
        let key = match (&it.drive, &it.loc) {
            _ if it.net.is_some() => it.net.as_ref().map(|(id, _)| PathBuf::from(format!("net:{id}"))),
            _ if it.phone.is_some() => it.phone.as_ref().map(|(key, _)| PathBuf::from(key)),
            (Some(d), _) => Some(PathBuf::from(d.key())),
            (None, _) if it.term => Some(PathBuf::from("polka:terminal")),
            (None, Location::Steam(_)) => Some(PathBuf::from("polka:steam")),
            (None, loc) => loc.local_path().map(|p| p.to_path_buf()),
        };
        if let Some(k) = key {
            items.push(item(tr("Choose icon…"), Cmd::ChooseIcon(k)));
        }
        if let (true, Some(p)) = (it.pinned, it.loc.local_path()) {
            items.push(item(tr("Unpin from sidebar"), Cmd::Unpin(p.to_path_buf())));
        }
        self.overlay = Some(Overlay::Menu(Menu::new(it.label, items)));
    }

    pub fn side_cmd(&mut self, cmd: Cmd) {
        let drive = |app: &App, i: usize| app.side_items.get(i).and_then(|it| it.drive.clone());
        match cmd {
            Cmd::EmptyTrashAsk => {
                let n = crate::trashbin::count();
                if n == 0 {
                    return self.toast(tr("The trash is empty"), false);
                }
                let items = vec![item(tr("Empty trash"), Cmd::EmptyTrashNow), item(tr("Cancel"), Cmd::Nothing)];
                self.overlay = Some(Overlay::Menu(Menu::new(trn("Delete %n items in the trash for good?", n), items)));
            }
            Cmd::EmptyTrashNow => {
                let (tx, wake) = (self.msg_tx.clone(), self.wake());
                std::thread::spawn(move || {
                    let (n, failed) = crate::trashbin::empty();
                    let _ = tx.send(Msg::TrashEmptied(n, failed));
                    wake();
                });
            }
            Cmd::DriveMount(i) => {
                if let Some(d) = drive(self, i) {
                    self.drive_op(d, Op::Mount);
                }
            }
            Cmd::DriveUnmount(i) => {
                if let Some(d) = drive(self, i) {
                    self.drive_op(d, Op::Unmount);
                }
            }
            Cmd::DriveEject(i) => {
                if let Some(d) = drive(self, i) {
                    // Every mounted partition of that disk goes first.
                    let mounted = crate::drives::list().into_iter().filter(|x| x.disk == d.disk && x.mount.is_some()).collect();
                    self.drive_op(d, Op::Eject(mounted));
                }
            }
            _ => {}
        }
    }

    /// Run a drive operation off the UI thread (polkit may ask for a
    /// password meanwhile).
    pub fn drive_op(&mut self, d: Drive, op: Op) {
        let msg = match op {
            Op::Mount => tr("Mounting…"),
            Op::Unmount => tr("Unmounting…"),
            Op::Eject(_) => tr("Ejecting…"),
        };
        self.toast(msg, false);
        let (tx, wake) = (self.msg_tx.clone(), self.wake());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Drive(crate::drives::apply(&d, &op)));
            wake();
        });
    }

    pub fn drive_done(&mut self, r: Result<Done, String>) {
        self.refresh_sidebar();
        let gone = match r {
            Err(e) => return self.toast(format!("{}: {e}", tr("Failed")), true),
            Ok(Done::Mounted(p)) => {
                // Opened right away, in place of the disk usage page if any.
                let cfg = self.cfg.clone();
                if let Some(u) = self.tabs[self.tab].usage.as_deref_mut() {
                    u.shown = false;
                }
                self.close_viewer();
                self.pane_mut().navigate(Location::Local(p), None, &cfg);
                self.focus = Focus::Panes;
                // "Mounting…" is done with.
                self.toast = None;
                return;
            }
            Ok(Done::Unmounted(gone)) => {
                self.toast(tr("Unmounted"), false);
                gone
            }
            Ok(Done::Ejected(gone)) => {
                self.toast(tr("The drive can be removed now"), false);
                gone
            }
        };
        // Panes on what's gone go home.
        let home = dirs::home_dir().unwrap_or_else(|| "/".into());
        let cfg = self.cfg.clone();
        for t in &mut self.tabs {
            {
            let p = &mut t.pane;
                if p.loc.local_path().is_some_and(|l| gone.iter().any(|g| l.starts_with(g))) {
                    p.navigate(Location::Local(home.clone()), None, &cfg);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- the places, in the settings

/// Settings → Files → Sidebar places: each one shown or not, in the order
/// chosen (dragged with the mouse, or L1 / R1 on the pad).
pub struct PlacesDlg {
    pub cursor: usize,
    pub scrolled: Option<usize>,
}

pub enum PlacesEv {
    Toggle(String),
    Pick(String),
    Close,
}

/// The list: a check box, the icon and the name of each place, under its
/// section. Returns its rows' rects (all the places, in order) and what
/// was done.
pub fn draw_places(ui: &mut egui::Ui, d: &mut PlacesDlg, all: &[SideItem], hidden: &[String], dragged: Option<&str>, icons: &mut crate::icons::Icons) -> (Vec<egui::Rect>, Option<PlacesEv>) {
    use egui::{Align2, FontId, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
    let mut ev = None;
    let mut rows = Vec::new();
    ui.set_width(crate::view::dialog_w().min(440.0));
    ui.label(egui::RichText::new(tr("Sidebar places")).strong().size(18.0));
    ui.label(egui::RichText::new(tr("Untick to hide; drag to put in another order.")).weak());
    ui.add_space(6.0);
    let max_h = (ui.ctx().content_rect().height() - 200.0).max(200.0);
    let scroll = d.scrolled != Some(d.cursor);
    egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
        let mut section = "";
        for (i, it) in all.iter().enumerate() {
            if it.section != section {
                section = it.section;
                ui.add_space(4.0);
                ui.label(egui::RichText::new(tr(section)).weak());
            }
            let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click_and_drag());
            rows.push(rect);
            let v = ui.visuals();
            let key = it.key();
            let on = !hidden.contains(&key);
            if i == d.cursor {
                ui.painter().rect(rect, 6.0, crate::view::tint(&v, 0.3), Stroke::new(1.5, v.selection.bg_fill), StrokeKind::Inside);
                if scroll {
                    r.scroll_to_me(None);
                }
            } else if r.hovered() || dragged == Some(key.as_str()) {
                ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
            }
            let b = Rect::from_center_size(pos2(rect.min.x + 18.0, rect.center().y), Vec2::splat(18.0));
            ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
            if on {
                ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(14.0), v.strong_text_color());
            }
            let px = crate::view::lookup_px(ui, 22.0);
            if let Some(u) = icons.named(it.icon, px) {
                egui::Image::new(u).tint(if on { egui::Color32::WHITE } else { egui::Color32::from_white_alpha(110) }).paint_at(ui, Rect::from_center_size(pos2(rect.min.x + 48.0, rect.center().y), Vec2::splat(22.0)));
            }
            let c = if on { v.text_color() } else { v.weak_text_color() };
            ui.painter().text(pos2(rect.min.x + 66.0, rect.center().y), Align2::LEFT_CENTER, &it.label, FontId::proportional(15.0), c);
            // The grip: what to drag by.
            ui.painter().text(pos2(rect.max.x - 14.0, rect.center().y), Align2::CENTER_CENTER, "⠿", FontId::proportional(16.0), v.weak_text_color());
            r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, &it.label));
            if r.drag_started_by(egui::PointerButton::Primary) {
                ev = Some(PlacesEv::Pick(key.clone()));
            } else if r.clicked() {
                d.cursor = i;
                ev = Some(PlacesEv::Toggle(key.clone()));
            }
        }
    });
    if scroll {
        d.scrolled = Some(d.cursor);
    }
    ui.add_space(6.0);
    // (In a row of its own: laid out right to left in the window itself,
    // it would take all the height there is.)
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(egui::RichText::new(tr("Done")).strong())).clicked() {
                ev = Some(PlacesEv::Close);
            }
        });
    });
    (rows, ev)
}

impl App {
    /// The pad in the places' settings: ↑ ↓ go, ✕ shows or hides, L1 / R1
    /// move the place up or down its section, ○ closes.
    pub fn places_input(&mut self, a: crate::input::Action) {
        use crate::input::Action;
        let Some(Overlay::Places(d)) = self.overlay.as_mut() else { return };
        let n = self.side_all.len();
        match a {
            Action::Up => d.cursor = d.cursor.saturating_sub(1),
            Action::Down => d.cursor = (d.cursor + 1).min(n.saturating_sub(1)),
            Action::Accept => {
                if let Some(k) = self.side_all.get(d.cursor).map(|i| i.key()) {
                    self.side_toggle(&k);
                }
            }
            Action::PrevTab | Action::NextTab => {
                let i = d.cursor;
                let Some(it) = self.side_all.get(i) else { return };
                let (key, section) = (it.key(), it.section);
                let first = self.side_all.iter().position(|x| x.section == section).unwrap_or(0);
                let len = self.side_all.iter().filter(|x| x.section == section).count();
                let at = i - first;
                let to = if a == Action::PrevTab { at.checked_sub(1) } else { (at + 1 < len).then_some(at + 1) };
                if let Some(to) = to {
                    self.side_move(&key, to);
                    if let Some(Overlay::Places(d)) = self.overlay.as_mut() {
                        d.cursor = first + to;
                    }
                }
            }
            Action::Back | Action::Menu => self.overlay = None,
            _ => {}
        }
    }

    /// The mouse let go of a place in the settings' list: there it goes.
    pub fn places_drop(&mut self, rows: &[egui::Rect]) {
        let Some((key, _)) = self.side_drag.clone() else { return };
        let y = self.ctx.input(|i| i.pointer.latest_pos()).map_or(0.0, |p| p.y);
        let Some(section) = self.side_all.iter().find(|i| i.key() == key).map(|i| i.section) else { return };
        let others: Vec<(String, egui::Rect)> = self.side_all.iter().zip(rows).filter(|(i, _)| i.section == section && i.key() != key).map(|(i, r)| (i.key(), *r)).collect();
        let to = others.iter().filter(|(_, r)| r.center().y < y).count();
        self.side_drag = None;
        self.side_move(&key, to);
        if let Some(Overlay::Places(d)) = self.overlay.as_mut() {
            if let Some(i) = self.side_all.iter().position(|x| x.key() == key) {
                d.cursor = i;
            }
        }
    }
}
