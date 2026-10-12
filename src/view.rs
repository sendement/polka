//! Rendering and pointer/keyboard input collection.

use std::sync::Arc;

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Galley, Key, Order, Rect, Sense, Shape,
    Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

use crate::app::{
    App, Cmd, Focus, Menu, Osk, OskKey, Overlay, Pane, Props, OSK_LAYOUTS, fmt_time, human_size,
};
use crate::config::{Config, ViewMode};
use crate::i18n::tr;
use crate::icons::Icons;
use crate::input::{Action, Input, PadKind};

const LIST_ROW: f32 = 30.0;
const LIST_ICON: f32 = 22.0;
const GRID_W: f32 = 108.0;
const GRID_H: f32 = 108.0;
const GRID_ICON: f32 = 56.0;
const SIDE_W: f32 = 210.0;
const SIDE_W_COLLAPSED: f32 = 52.0;

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let inputs: Vec<Input> = self.pad_rx.try_iter().collect();
        // The pad is read globally; never act on it while another window
        // (e.g. a game) has focus.
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        for i in inputs {
            if focused || matches!(i, Input::Pad(_) | Input::Release(_)) {
                self.handle(i);
            }
        }
        self.keyboard(&ctx);
        self.tick();
        self.icons.trim(&ctx);

        self.draw_bottom(ui);
        self.draw_top(ui);
        self.draw_sidebar(ui);
        self.draw_center(ui);
        self.draw_overlay(&ctx);

        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if !self.jobs.is_empty() || self.toast.is_some() || self.tabs.iter().any(|t| t.panes.iter().any(|p| p.loading.is_some())) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

impl App {
    // ------------------------------------------------------------ keyboard

    fn keyboard(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let osk = matches!(self.overlay, Some(Overlay::Osk(_)));
        for ev in events {
            match ev {
                egui::Event::Text(t) if osk => {
                    if let Some(Overlay::Osk(k)) = self.overlay.as_mut() {
                        k.insert(&t);
                    }
                }
                egui::Event::Key { key, pressed: true, modifiers: m, .. } => {
                    if osk {
                        self.osk_keyboard(key);
                    } else {
                        self.key(key, m);
                    }
                }
                _ => {}
            }
        }
    }

    fn osk_keyboard(&mut self, key: Key) {
        let Some(Overlay::Osk(k)) = self.overlay.as_mut() else { return };
        match key {
            Key::Backspace => k.backspace(),
            Key::Delete => k.delete(),
            Key::ArrowLeft => k.caret_by(-1),
            Key::ArrowRight => k.caret_by(1),
            Key::Home => k.caret = 0,
            Key::End => k.caret = k.text.len(),
            Key::Enter => self.osk_submit(),
            Key::Escape => self.osk_cancel(),
            _ => {}
        }
    }

    fn key(&mut self, key: Key, m: egui::Modifiers) {
        let press = |s: &mut Self, a| s.handle(Input::Press(a));
        if m.command {
            let cmd = match key {
                Key::C => Some(Cmd::Copy),
                Key::X => Some(Cmd::Cut),
                Key::V => Some(Cmd::Paste),
                Key::A => Some(Cmd::SelectAll),
                Key::T => Some(Cmd::NewTab),
                Key::W => Some(Cmd::CloseTab),
                Key::H => Some(Cmd::ToggleHidden),
                Key::B => Some(Cmd::ToggleSidebar),
                Key::L => Some(Cmd::GoTo),
                Key::F if m.shift => Some(Cmd::Search),
                Key::F => Some(Cmd::Filter),
                Key::Q => Some(Cmd::Quit),
                Key::N if m.shift => Some(Cmd::NewFolder),
                Key::Tab | Key::PageDown => return press(self, if m.shift && key == Key::Tab { Action::PrevTab } else { Action::NextTab }),
                Key::PageUp => return press(self, Action::PrevTab),
                Key::Plus | Key::Equals => return self.zoom(0.1),
                Key::Minus => return self.zoom(-0.1),
                Key::Num0 => return self.zoom(1.0 - self.cfg.zoom),
                _ => None,
            };
            if let Some(c) = cmd {
                if self.overlay.is_none() {
                    self.run(c);
                }
            }
            return;
        }
        if m.alt {
            match key {
                Key::ArrowLeft => press(self, Action::HistoryBack),
                Key::ArrowRight => press(self, Action::HistoryForward),
                Key::ArrowUp => press(self, Action::Back),
                _ => {}
            }
            return;
        }
        let a = match key {
            Key::ArrowUp => Action::Up,
            Key::ArrowDown => Action::Down,
            Key::ArrowLeft => Action::Left,
            Key::ArrowRight => Action::Right,
            Key::Enter => Action::Accept,
            Key::Escape | Key::Backspace => Action::Back,
            Key::Space | Key::Insert => {
                self.handle(Input::Press(Action::Select));
                self.handle(Input::Release(Action::Select));
                if key == Key::Insert {
                    self.handle(Input::Press(Action::Down));
                }
                return;
            }
            Key::F10 if m.shift => Action::Actions,
            Key::F10 => Action::Menu,
            Key::F1 => return if self.overlay.is_none() { self.run(Cmd::Help) },
            Key::F2 => return if self.overlay.is_none() { self.run(Cmd::Rename) },
            Key::F3 => Action::View,
            Key::Delete => {
                if self.overlay.is_none() {
                    self.run(if m.shift { Cmd::DeleteAsk } else { Cmd::TrashAsk });
                }
                return;
            }
            Key::Tab => {
                if self.cfg.dual && self.overlay.is_none() {
                    let t = &mut self.tabs[self.tab];
                    t.active = 1 - t.active;
                    self.focus = Focus::Panes;
                }
                return;
            }
            Key::PageUp => Action::LeftTrigger,
            Key::PageDown => Action::RightTrigger,
            Key::Home => Action::Home,
            Key::End => Action::End,
            _ => return,
        };
        // In dual mode PageUp/Down would switch panes; keep them as paging.
        if self.cfg.dual && matches!(a, Action::LeftTrigger | Action::RightTrigger) && self.overlay.is_none() && self.focus == Focus::Panes {
            let rows = self.pane().page_rows.max(1) as isize;
            let cols = if self.cfg.view == ViewMode::Grid { self.pane().cols.max(1) as isize } else { 1 };
            self.pane_mut().move_cursor(if a == Action::RightTrigger { rows * cols } else { -rows * cols });
            return;
        }
        press(self, a);
    }

    fn zoom(&mut self, d: f32) {
        self.cfg.zoom = (self.cfg.zoom + d).clamp(0.6, 3.0);
        self.ctx.set_zoom_factor(self.cfg.zoom);
        self.cfg.save();
    }

    // ------------------------------------------------------------ top

    fn draw_top(&mut self, ui: &mut Ui) {
        let cfg = self.cfg.clone();
        let mut go: Option<crate::vfs::Location> = None;
        let mut nav: Option<Action> = None;
        let mut switch_tab = None;
        let mut close_tab = None;
        let mut new_tab = false;
        let mut clear_filter = false;
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if ui.button("⬅").on_hover_text(tr("History")).clicked() {
                    nav = Some(Action::HistoryBack);
                }
                if ui.button("➡").clicked() {
                    nav = Some(Action::HistoryForward);
                }
                if ui.button("⬆").clicked() {
                    nav = Some(Action::Back);
                }
                ui.add_space(8.0);
                let crumbs = self.pane().loc.crumbs();
                let n = crumbs.len();
                for (i, (label, loc)) in crumbs.into_iter().enumerate() {
                    let last = i + 1 == n;
                    let text = egui::RichText::new(label).size(15.0);
                    let b = egui::Button::new(if last { text.strong() } else { text }).frame(last);
                    if ui.add(b).clicked() {
                        go = Some(loc);
                    }
                    if !last {
                        ui.label(egui::RichText::new("›").weak());
                    }
                }
                if self.pane().loc.is_archive() && self.pane().loc.dest().is_none() {
                    ui.label(egui::RichText::new("🔒").weak()).on_hover_text(tr("This archive format is read-only"));
                }
                if let Some(f) = self.pane().filter.clone() {
                    ui.add_space(12.0);
                    let chip = egui::Button::new(egui::RichText::new(format!("{}: {f}  ✖", tr("Filter"))).size(14.0))
                        .fill(ui.visuals().selection.bg_fill.gamma_multiply(0.5));
                    if ui.add(chip).clicked() {
                        clear_filter = true;
                    }
                }
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                glyph(ui, Action::PrevTab, self.pad);
                for (i, t) in self.tabs.iter().enumerate() {
                    let p = &t.panes[if cfg.dual { t.active } else { 0 }];
                    let active = i == self.tab;
                    let r = ui.add(
                        egui::Button::selectable(active, egui::RichText::new(format!("  {}  ", p.loc.title())).size(14.0))
                            .min_size(vec2(90.0, 26.0)),
                    );
                    if r.clicked() {
                        switch_tab = Some(i);
                    }
                    if r.middle_clicked() {
                        close_tab = Some(i);
                    }
                }
                if ui.button(" + ").on_hover_text(tr("New tab")).clicked() {
                    new_tab = true;
                }
                glyph(ui, Action::NextTab, self.pad);
            });
            ui.add_space(4.0);
        });
        if let Some(a) = nav {
            self.focus = Focus::Panes;
            self.handle(Input::Press(a));
        }
        if let Some(loc) = go {
            self.pane_mut().navigate(loc, None, &cfg);
        }
        if let Some(i) = switch_tab {
            self.tab = i;
        }
        if let Some(i) = close_tab {
            self.tab = i;
            self.run(Cmd::CloseTab);
        }
        if new_tab {
            self.run(Cmd::NewTab);
        }
        if clear_filter {
            self.pane_mut().set_filter(None);
        }
    }

    // ------------------------------------------------------------ bottom

    fn draw_bottom(&mut self, ui: &mut Ui) {
        let mut cancel = None;
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                let p = self.pane();
                let mut s = format!("{} {}", p.entries.len(), tr("items"));
                if !p.selected.is_empty() {
                    s += &format!(" · {} {}", p.selected.len(), tr("selected"));
                }
                ui.label(s);
                for (i, j) in self.jobs.iter().enumerate() {
                    ui.separator();
                    ui.label(&j.label);
                    ui.add(egui::ProgressBar::new(j.fraction()).desired_width(140.0).show_percentage());
                    if ui.small_button("✖").clicked() {
                        cancel = Some(i);
                    }
                }
                if let Some((t, err, _)) = &self.toast {
                    ui.separator();
                    let c = if *err { ui.visuals().error_fg_color } else { ui.visuals().strong_text_color() };
                    ui.label(egui::RichText::new(t).color(c));
                }
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    for (a, label) in self.hints().into_iter().rev() {
                        ui.label(label);
                        glyph(ui, a, self.pad);
                        ui.add_space(6.0);
                    }
                });
            });
            ui.add_space(3.0);
        });
        if let Some(i) = cancel {
            self.jobs[i].cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn hints(&self) -> Vec<(Action, &'static str)> {
        match &self.overlay {
            Some(Overlay::Menu(_)) => vec![(Action::Accept, tr("OK")), (Action::Back, tr("Close"))],
            Some(Overlay::Osk(_)) => vec![
                (Action::Accept, "⌨"),
                (Action::Select, tr("Backspace")),
                (Action::Actions, tr("Space")),
                (Action::LeftTrigger, tr("Shift")),
                (Action::View, tr("Layout")),
                (Action::Menu, tr("OK")),
                (Action::Back, tr("Cancel")),
            ],
            Some(_) => vec![(Action::Back, tr("Close"))],
            None if self.focus == Focus::Sidebar => {
                vec![(Action::Accept, tr("Open")), (Action::Actions, tr("Actions")), (Action::Back, tr("Back"))]
            }
            None => vec![
                (Action::Accept, tr("Open")),
                (Action::Back, tr("Back")),
                (Action::Select, tr("Select")),
                (Action::Actions, tr("Actions")),
                (Action::Menu, tr("Menu")),
            ],
        }
    }

    // ------------------------------------------------------------ sidebar

    fn draw_sidebar(&mut self, ui: &mut Ui) {
        let collapsed = self.cfg.sidebar_collapsed;
        let focused = self.focus == Focus::Sidebar && self.overlay.is_none();
        let cur_loc = self.pane().loc.clone();
        let mut clicked = None;
        let mut actions = None;
        egui::Panel::left("sidebar")
            .resizable(false)
            .exact_size(if collapsed { SIDE_W_COLLAPSED } else { SIDE_W })
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let mut section = "";
                    for (i, it) in self.side_items.iter().enumerate() {
                        if it.section != section {
                            section = it.section;
                            ui.add_space(6.0);
                            if collapsed {
                                ui.separator();
                            } else {
                                ui.label(egui::RichText::new(tr(section)).small().weak());
                            }
                        }
                        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                        let is_cursor = focused && i == self.side_cursor;
                        let here = it.loc == cur_loc;
                        paint_row_bg(ui, rect, is_cursor, here, r.hovered());
                        let icon_rect = Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2(20.0, 20.0));
                        if let Some(u) = self.icons.named(it.icon, 24) {
                            egui::Image::new(u).paint_at(ui, icon_rect);
                        }
                        if !collapsed {
                            let g = elided(ui, &it.label, FontId::proportional(14.0), ui.visuals().text_color(), rect.width() - 44.0, 1);
                            ui.painter().galley(pos2(rect.min.x + 36.0, rect.center().y - g.size().y / 2.0), g, Color32::WHITE);
                        }
                        let r = if collapsed { r.on_hover_text(&it.label) } else { r };
                        if r.clicked() {
                            clicked = Some(i);
                        }
                        if r.secondary_clicked() {
                            actions = Some(i);
                        }
                        if is_cursor {
                            r.scroll_to_me(None);
                        }
                    }
                });
            });
        if let Some(i) = clicked {
            self.side_cursor = i;
            self.focus = Focus::Sidebar;
            self.handle(Input::Press(Action::Accept));
        }
        if let Some(i) = actions {
            self.side_cursor = i;
            self.focus = Focus::Sidebar;
            self.handle(Input::Press(Action::Actions));
        }
    }

    // ------------------------------------------------------------ panes

    fn draw_center(&mut self, ui: &mut Ui) {
        let dual = self.cfg.dual;
        let tab = self.tab;
        let focused = self.focus == Focus::Panes && self.overlay.is_none();
        let active = self.tabs[tab].active;
        let mut events: Vec<(usize, PaneEvent)> = Vec::new();
        egui::CentralPanel::default().show(ui, |ui| {
            let App { tabs, icons, cfg, .. } = self;
            let [a, b] = &mut tabs[tab].panes;
            if dual {
                let full = ui.available_rect_before_wrap();
                let gap = 6.0;
                let w = (full.width() - gap) / 2.0;
                let ra = Rect::from_min_size(full.min, vec2(w, full.height()));
                let rb = Rect::from_min_size(pos2(full.min.x + w + gap, full.min.y), vec2(w, full.height()));
                for (idx, (pane, rect)) in [(a, ra), (b, rb)].into_iter().enumerate() {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(("pane", idx)));
                    let is_active = active == idx;
                    let stroke = if is_active && focused {
                        Stroke::new(1.5, child.visuals().selection.bg_fill)
                    } else {
                        Stroke::new(1.0, child.visuals().widgets.noninteractive.bg_stroke.color)
                    };
                    child.painter().rect_stroke(rect, 6.0, stroke, StrokeKind::Inside);
                    let mut inner = child.new_child(egui::UiBuilder::new().max_rect(rect.shrink(4.0)));
                    let header = pane.loc.display();
                    inner.label(egui::RichText::new(header).small().color(if is_active { inner.visuals().strong_text_color() } else { inner.visuals().weak_text_color() }));
                    for e in draw_pane(&mut inner, pane, icons, cfg, is_active && focused, (tab, idx)) {
                        events.push((idx, e));
                    }
                }
            } else {
                for e in draw_pane(ui, a, icons, cfg, focused, (tab, 0)) {
                    events.push((0, e));
                }
            }
        });
        for (idx, e) in events {
            self.focus = Focus::Panes;
            self.tabs[tab].active = idx;
            match e {
                PaneEvent::Click(i, toggle) => {
                    let p = self.pane_mut();
                    p.cursor = i;
                    if toggle {
                        if let Some(n) = p.entries.get(i).map(|e| e.name.clone()) {
                            if !p.selected.remove(&n) {
                                p.selected.insert(n);
                            }
                        }
                    }
                }
                PaneEvent::Activate(i) => {
                    self.pane_mut().cursor = i;
                    self.activate();
                }
                PaneEvent::Context(i) => {
                    if let Some(i) = i {
                        self.pane_mut().cursor = i;
                    }
                    self.handle(Input::Press(Action::Actions));
                }
            }
        }
    }

    // ------------------------------------------------------------ overlays

    fn draw_overlay(&mut self, ctx: &egui::Context) {
        if self.overlay.is_none() {
            return;
        }
        let screen = ctx.content_rect();
        let mut close = false;
        egui::Area::new("backdrop".into()).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(140));
            close = resp.clicked();
        });
        if close {
            self.overlay = None;
            return;
        }
        let pad = self.pad;
        let mut run: Option<Cmd> = None;
        let mut osk_key: Option<OskKey> = None;
        let mut dismiss = false;
        egui::Area::new("overlay".into())
            .order(Order::Foreground)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(14.0).corner_radius(10.0).show(ui, |ui| match self.overlay.as_mut() {
                    Some(Overlay::Menu(m)) => run = draw_menu(ui, m),
                    Some(Overlay::Osk(k)) => osk_key = draw_osk(ui, k),
                    Some(Overlay::Props(p)) => dismiss = draw_props(ui, p),
                    Some(Overlay::Help) => dismiss = draw_help(ui, pad),
                    None => {}
                });
            });
        if dismiss {
            self.overlay = None;
        }
        if let Some(c) = run {
            self.overlay = None;
            self.run(c);
        }
        if let Some(k) = osk_key {
            self.osk_key(k);
        }
    }
}

// ---------------------------------------------------------------- pane view

enum PaneEvent {
    /// index, ctrl-toggle
    Click(usize, bool),
    Activate(usize),
    Context(Option<usize>),
}

fn draw_pane(ui: &mut Ui, pane: &mut Pane, icons: &mut Icons, cfg: &Config, focused: bool, id: (usize, usize)) -> Vec<PaneEvent> {
    let mut events = Vec::new();
    if pane.loading.is_some() {
        ui.centered_and_justified(|ui| ui.spinner());
        return events;
    }
    if let Some(err) = &pane.error {
        ui.centered_and_justified(|ui| ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color)));
        return events;
    }
    if pane.entries.is_empty() {
        let empty = if pane.filter.is_some() || matches!(pane.loc, crate::vfs::Location::Search { .. }) {
            tr("Nothing found")
        } else {
            tr("Empty folder")
        };
        let r = ui.centered_and_justified(|ui| ui.label(egui::RichText::new(empty).weak())).response;
        if r.secondary_clicked() {
            events.push(PaneEvent::Context(None));
        }
        return events;
    }

    let grid = cfg.view == ViewMode::Grid;
    let width = ui.available_width();
    let cols = if grid { ((width - 8.0) / GRID_W).floor().max(1.0) as usize } else { 1 };
    pane.cols = cols;
    let row_h = if grid { GRID_H } else { LIST_ROW };
    let rows = pane.entries.len().div_ceil(cols);

    if !grid {
        // Column header.
        let (r, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
        let c = ui.visuals().weak_text_color();
        let f = FontId::proportional(12.0);
        ui.painter().text(pos2(r.min.x + 40.0, r.center().y), Align2::LEFT_CENTER, tr("Name"), f.clone(), c);
        ui.painter().text(pos2(r.max.x - 160.0, r.center().y), Align2::RIGHT_CENTER, tr("Size"), f.clone(), c);
        ui.painter().text(pos2(r.max.x - 145.0, r.center().y), Align2::LEFT_CENTER, tr("Modified"), f, c);
    }

    let mut sa = egui::ScrollArea::vertical().id_salt(("pane-scroll", id)).auto_shrink([false, false]);
    if pane.scroll_to_cursor {
        let top = (pane.cursor / cols) as f32 * row_h;
        let mut off = pane.scroll_y;
        if top < off {
            off = top;
        } else if top + row_h > off + pane.view_h {
            off = top + row_h - pane.view_h;
        }
        sa = sa.vertical_scroll_offset(off.max(0.0));
        pane.scroll_to_cursor = false;
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    let out = sa.show_rows(ui, row_h, rows, |ui, range| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        for row in range {
            let (row_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
            for col in 0..cols {
                let i = row * cols + col;
                let Some(e) = pane.entries.get(i) else { break };
                let rect = if grid {
                    Rect::from_min_size(row_rect.min + vec2(col as f32 * GRID_W + 4.0, 0.0), vec2(GRID_W - 4.0, GRID_H - 4.0))
                } else {
                    row_rect
                };
                let r = ui.interact(rect, ui.id().with(("entry", i)), Sense::click());
                let sel = pane.selected.contains(&e.name);
                let is_cursor = i == pane.cursor;
                paint_entry_bg(ui, rect, is_cursor, focused, sel, r.hovered());
                let path = pane.loc.entry_path(&e.name);
                let icon_size = if grid { GRID_ICON } else { LIST_ICON };
                let thumb = match (&path, e.mtime) {
                    (Some(p), Some(t)) if cfg.thumbnails && !e.is_dir => icons.thumb(p, t),
                    _ => None,
                };
                let icon = if thumb.is_some() { None } else { icons.for_entry(e, path.as_deref(), if grid { 64 } else { 24 }) };
                let text_c = if e.hidden() { ui.visuals().weak_text_color() } else { ui.visuals().text_color() };
                if grid {
                    let ir = Rect::from_center_size(pos2(rect.center().x, rect.min.y + 6.0 + icon_size / 2.0), Vec2::splat(icon_size));
                    paint_icon(ui, ir, icon, thumb);
                    let g = elided_centered(ui, &e.name, FontId::proportional(13.0), text_c, rect.width() - 6.0, 2);
                    ui.painter().galley(pos2(rect.center().x, ir.max.y + 4.0), g, text_c);
                } else {
                    let ir = Rect::from_min_size(pos2(rect.min.x + 10.0, rect.center().y - icon_size / 2.0), Vec2::splat(icon_size));
                    paint_icon(ui, ir, icon, thumb);
                    let name_w = rect.width() - 40.0 - 260.0;
                    let g = elided(ui, &e.name, FontId::proportional(14.0), text_c, name_w.max(60.0), 1);
                    ui.painter().galley(pos2(rect.min.x + 40.0, rect.center().y - g.size().y / 2.0), g, text_c);
                    let weak = ui.visuals().weak_text_color();
                    let f = FontId::proportional(13.0);
                    let size = if e.is_dir { String::new() } else { human_size(e.size) };
                    ui.painter().text(pos2(rect.max.x - 160.0, rect.center().y), Align2::RIGHT_CENTER, size, f.clone(), weak);
                    if let Some(t) = e.mtime {
                        ui.painter().text(pos2(rect.max.x - 145.0, rect.center().y), Align2::LEFT_CENTER, fmt_time(t), f, weak);
                    }
                }
                if r.double_clicked() {
                    events.push(PaneEvent::Activate(i));
                } else if r.clicked() {
                    events.push(PaneEvent::Click(i, ui.input(|i| i.modifiers.command)));
                } else if r.secondary_clicked() {
                    events.push(PaneEvent::Context(Some(i)));
                }
            }
        }
    });
    pane.scroll_y = out.state.offset.y;
    pane.view_h = out.inner_rect.height();
    pane.page_rows = ((pane.view_h / row_h) as usize).saturating_sub(1).max(1);
    events
}

/// Theme icon, or a thumbnail fitted into the square keeping its aspect.
fn paint_icon(ui: &Ui, rect: Rect, icon: Option<String>, thumb: Option<(String, f32)>) {
    if let Some((uri, aspect)) = thumb {
        let size = if aspect >= 1.0 {
            vec2(rect.width(), rect.width() / aspect)
        } else {
            vec2(rect.height() * aspect, rect.height())
        };
        let r = Rect::from_center_size(rect.center(), size);
        egui::Image::new(uri).corner_radius(3).paint_at(ui, r);
    } else if let Some(u) = icon {
        egui::Image::new(u).paint_at(ui, rect);
    }
}

fn paint_entry_bg(ui: &Ui, rect: Rect, cursor: bool, focused: bool, selected: bool, hovered: bool) {
    let v = ui.visuals();
    let accent = v.selection.bg_fill;
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 6.0, accent.gamma_multiply(0.45));
    } else if hovered {
        p.rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill.gamma_multiply(0.6));
    }
    if cursor {
        if focused {
            p.rect_filled(rect, 6.0, accent.gamma_multiply(if selected { 0.35 } else { 0.55 }));
            p.rect_stroke(rect, 6.0, Stroke::new(2.0, accent), StrokeKind::Inside);
        } else {
            p.rect_stroke(rect, 6.0, Stroke::new(1.0, accent.gamma_multiply(0.7)), StrokeKind::Inside);
        }
    }
}

fn paint_row_bg(ui: &Ui, rect: Rect, cursor: bool, current: bool, hovered: bool) {
    let v = ui.visuals();
    let accent = v.selection.bg_fill;
    if cursor {
        ui.painter().rect_filled(rect, 6.0, accent.gamma_multiply(0.55));
        ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, accent), StrokeKind::Inside);
    } else if current {
        ui.painter().rect_filled(rect, 6.0, accent.gamma_multiply(0.25));
    } else if hovered {
        ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill.gamma_multiply(0.6));
    }
}

fn layout(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32, rows: usize, center: bool) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat { font_id: font, color, ..Default::default() },
    );
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: rows,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    if center {
        job.halign = Align::Center;
    }
    ui.painter().layout_job(job)
}

fn elided(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32, rows: usize) -> Arc<Galley> {
    layout(ui, text, font, color, width, rows, false)
}

fn elided_centered(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32, rows: usize) -> Arc<Galley> {
    layout(ui, text, font, color, width, rows, true)
}

// ---------------------------------------------------------------- overlays

fn draw_menu(ui: &mut Ui, m: &mut Menu) -> Option<Cmd> {
    let mut run = None;
    ui.set_min_width(340.0);
    ui.label(egui::RichText::new(&m.title).strong().size(16.0));
    ui.add_space(6.0);
    ui.spacing_mut().item_spacing.y = 2.0;
    for (i, it) in m.items.iter().enumerate() {
        let (rect, r) = ui.allocate_exact_size(vec2(340.0, 34.0), if it.enabled { Sense::click() } else { Sense::hover() });
        if r.hovered() && it.enabled && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
            m.cursor = i;
        }
        let cur = i == m.cursor;
        if cur {
            let a = ui.visuals().selection.bg_fill;
            ui.painter().rect_filled(rect, 6.0, a.gamma_multiply(0.55));
            ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, a), StrokeKind::Inside);
        }
        let color = if it.enabled { ui.visuals().text_color() } else { ui.visuals().weak_text_color().gamma_multiply(0.6) };
        let mut x = rect.min.x + 12.0;
        if let Some(on) = it.checked {
            let b = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(14.0));
            ui.painter().rect_stroke(b, 3.0, Stroke::new(1.5, color), StrokeKind::Inside);
            if on {
                ui.painter().rect_filled(b.shrink(3.5), 2.0, ui.visuals().selection.bg_fill.gamma_multiply(1.6));
            }
            x += 24.0;
        }
        ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, &it.label, FontId::proportional(15.0), color);
        if r.clicked() && it.enabled {
            run = Some(it.cmd.clone());
        }
    }
    run
}

fn draw_osk(ui: &mut Ui, k: &mut Osk) -> Option<OskKey> {
    let mut pressed = None;
    ui.label(egui::RichText::new(&k.title).strong().size(16.0));
    ui.add_space(6.0);

    // Text field with caret.
    let width = 560.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
    ui.painter().rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
    ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, ui.visuals().selection.bg_fill), StrokeKind::Inside);
    let font = FontId::proportional(17.0);
    let text = k.string();
    let before: String = k.text[..k.caret].iter().collect();
    let pre_w = ui.painter().layout_no_wrap(before, font.clone(), Color32::WHITE).size().x;
    // Scroll the text so the caret stays visible.
    let shift = (pre_w - (width - 30.0)).max(0.0);
    let clip = ui.painter().with_clip_rect(rect.shrink(4.0));
    clip.text(pos2(rect.min.x + 10.0 - shift, rect.center().y), Align2::LEFT_CENTER, text, font, ui.visuals().strong_text_color());
    let cx = rect.min.x + 10.0 - shift + pre_w;
    let blink = (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0;
    if blink {
        clip.line_segment([pos2(cx, rect.min.y + 8.0), pos2(cx, rect.max.y - 8.0)], Stroke::new(2.0, ui.visuals().strong_text_color()));
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    ui.add_space(10.0);

    let key_w = (width - 10.0 * 4.0) / 11.0;
    for row in 0..k.rows() {
        let n = k.row_len(row);
        let special = row == 4;
        let w = if special { (width - (n as f32 - 1.0) * 4.0) / n as f32 } else { key_w };
        let total = n as f32 * w + (n as f32 - 1.0) * 4.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_space((width - total) / 2.0);
            for col in 0..n {
                let key = k.key_at(row, col);
                let label = match key {
                    OskKey::Char(c) => c.to_string(),
                    OskKey::Shift => if k.shift { "⇧ ABC".into() } else { "⇧ abc".into() },
                    OskKey::Layout => OSK_LAYOUTS[k.layout].0.to_string(),
                    OskKey::Space => tr("Space").into(),
                    OskKey::Backspace => "⌫".into(),
                    OskKey::Left => "◀ ▶".into(),
                    OskKey::Right => "▶".into(),
                    OskKey::Ok => tr("OK").into(),
                };
                let (rect, r) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click());
                let cur = row == k.row && col == k.col;
                let v = ui.visuals();
                let bg = if cur { v.selection.bg_fill } else if r.hovered() { v.widgets.hovered.bg_fill } else { v.widgets.inactive.bg_fill };
                ui.painter().rect_filled(rect, 6.0, bg);
                if cur {
                    ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, v.strong_text_color()), StrokeKind::Inside);
                }
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(16.0), v.strong_text_color());
                if r.clicked() {
                    k.row = row;
                    k.col = col;
                    // The ◀ ▶ key moves the caret by click position.
                    pressed = Some(if key == OskKey::Left {
                        let right = ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| p.x > rect.center().x);
                        if right { OskKey::Right } else { OskKey::Left }
                    } else {
                        key
                    });
                }
            }
        });
        ui.add_space(2.0);
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new("L1/R1: ◀ ▶").small().weak());
    pressed
}

fn draw_props(ui: &mut Ui, p: &Props) -> bool {
    ui.set_min_width(420.0);
    ui.horizontal(|ui| {
        if let Some(u) = &p.icon {
            ui.add(egui::Image::new(u.clone()).fit_to_exact_size(Vec2::splat(64.0)));
        }
        ui.label(egui::RichText::new(tr("Properties")).strong().size(18.0));
    });
    ui.add_space(8.0);
    egui::Grid::new("props").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
        for (k, v) in &p.rows {
            ui.label(egui::RichText::new(k).weak());
            ui.add(egui::Label::new(v).wrap());
            ui.end_row();
        }
    });
    ui.add_space(8.0);
    ui.button(tr("Close")).clicked()
}

fn draw_help(ui: &mut Ui, pad: PadKind) -> bool {
    ui.label(egui::RichText::new(tr("Controls")).strong().size(18.0));
    ui.add_space(8.0);
    let rows: [(Action, &str, &str); 16] = [
        (Action::Up, "", "←↑→↓"),
        (Action::Accept, tr("Open"), "Enter"),
        (Action::Back, tr("Back"), "Backspace / Esc"),
        (Action::Select, tr("Select"), "Space / Insert"),
        (Action::Actions, tr("Actions"), "Shift+F10"),
        (Action::Menu, tr("Menu"), "F10"),
        (Action::View, tr("Dual pane"), "F3"),
        (Action::PrevTab, tr("Tabs"), "Ctrl+Tab"),
        (Action::LeftTrigger, tr("Panes / page"), "Tab / PgUp PgDn"),
        (Action::LeftStick, tr("Sidebar"), "Ctrl+B"),
        (Action::RightStick, tr("Properties"), ""),
        (Action::HistoryBack, tr("History"), "Alt+← →"),
        (Action::FastDown, tr("Fast scroll"), ""),
        (Action::Home, tr("Rename"), "F2"),
        (Action::TouchClick, tr("Touchpad: swipe to move, click for places"), ""),
        (Action::Menu, tr("Filter / search"), "Ctrl+F / Ctrl+Shift+F"),
    ];
    egui::Grid::new("help").num_columns(3).spacing([18.0, 8.0]).show(ui, |ui| {
        ui.label(egui::RichText::new(tr("Gamepad")).weak());
        ui.label("");
        ui.label(egui::RichText::new(tr("Keyboard")).weak());
        ui.end_row();
        for (a, label, keys) in rows {
            ui.horizontal(|ui| {
                if a == Action::Home {
                    glyph(ui, Action::Actions, pad);
                } else {
                    glyph(ui, a, pad);
                    if a == Action::PrevTab {
                        glyph(ui, Action::NextTab, pad);
                    }
                    if a == Action::LeftTrigger {
                        glyph(ui, Action::RightTrigger, pad);
                    }
                }
            });
            ui.label(if a == Action::Up { tr("Move") } else { label });
            ui.label(egui::RichText::new(keys).monospace());
            ui.end_row();
        }
    });
    if crate::touchpad::TOUCHPAD_STATUS.load(std::sync::atomic::Ordering::Relaxed) == crate::touchpad::STATUS_DENIED {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(tr("The touchpad needs the udev rule from the Polka package (see README).")).small().color(ui.visuals().warn_fg_color));
    }
    ui.add_space(8.0);
    ui.button(tr("Close")).clicked()
}

// ---------------------------------------------------------------- glyphs

/// Draw a gamepad button glyph (font independent).
pub fn glyph(ui: &mut Ui, a: Action, pad: PadKind) {
    let ps = pad == PadKind::PlayStation;
    let shoulder = |l: &'static str, x: &'static str| if ps { l } else { x };
    let label: Option<&str> = match a {
        Action::PrevTab => Some(shoulder("L1", "LB")),
        Action::NextTab => Some(shoulder("R1", "RB")),
        Action::LeftTrigger => Some(shoulder("L2", "LT")),
        Action::RightTrigger => Some(shoulder("R2", "RT")),
        Action::LeftStick => Some(shoulder("L3", "LS")),
        Action::RightStick => Some(shoulder("R3", "RS")),
        Action::TouchClick => Some("TP"),
        Action::HistoryBack | Action::HistoryForward => Some("R ↔"),
        Action::FastUp | Action::FastDown => Some("R ↕"),
        _ => None,
    };
    if let Some(l) = label {
        let (rect, _) = ui.allocate_exact_size(vec2(28.0, 18.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(5), Color32::from_gray(60));
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, l, FontId::proportional(11.0), Color32::from_gray(230));
        return;
    }
    let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    let bg = Color32::from_gray(55);
    match a {
        Action::Menu | Action::View => {
            p.rect_filled(rect.shrink2(vec2(0.0, 3.0)), 6.0, bg);
            let fg = Color32::from_gray(230);
            if a == Action::Menu {
                for dy in [-3.0, 0.0, 3.0] {
                    p.line_segment([c + vec2(-4.0, dy), c + vec2(4.0, dy)], Stroke::new(1.3, fg));
                }
            } else {
                p.rect_stroke(Rect::from_center_size(c + vec2(-1.5, -1.0), vec2(6.0, 5.0)), 1.0, Stroke::new(1.1, fg), StrokeKind::Inside);
                p.rect_stroke(Rect::from_center_size(c + vec2(1.5, 1.5), vec2(6.0, 5.0)), 1.0, Stroke::new(1.1, fg), StrokeKind::Inside);
            }
        }
        Action::Up | Action::Down | Action::Left | Action::Right | Action::Home | Action::End => {
            let fg = Color32::from_gray(200);
            p.rect_filled(Rect::from_center_size(c, vec2(6.0, 18.0)), 2.0, fg);
            p.rect_filled(Rect::from_center_size(c, vec2(18.0, 6.0)), 2.0, fg);
        }
        _ => {
            p.circle_filled(c, 10.0, bg);
            if ps {
                let st = |col| Stroke::new(1.8, col);
                match a {
                    Action::Accept => {
                        let col = Color32::from_rgb(124, 178, 232);
                        p.line_segment([c + vec2(-4.5, -4.5), c + vec2(4.5, 4.5)], st(col));
                        p.line_segment([c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5)], st(col));
                    }
                    Action::Back => {
                        p.circle_stroke(c, 5.0, st(Color32::from_rgb(232, 100, 100)));
                    }
                    Action::Select => {
                        p.rect_stroke(Rect::from_center_size(c, Vec2::splat(9.0)), 0.0, st(Color32::from_rgb(230, 150, 200)), StrokeKind::Middle);
                    }
                    Action::Actions => {
                        let pts = vec![c + vec2(0.0, -5.5), c + vec2(5.5, 4.0), c + vec2(-5.5, 4.0)];
                        p.add(Shape::closed_line(pts, st(Color32::from_rgb(90, 200, 160))));
                    }
                    _ => {}
                }
            } else {
                let (l, col) = match a {
                    Action::Accept => ("A", Color32::from_rgb(110, 200, 90)),
                    Action::Back => ("B", Color32::from_rgb(232, 90, 90)),
                    Action::Select => ("X", Color32::from_rgb(90, 150, 240)),
                    Action::Actions => ("Y", Color32::from_rgb(240, 200, 70)),
                    _ => ("?", Color32::GRAY),
                };
                p.text(c, Align2::CENTER_CENTER, l, FontId::proportional(12.0), col);
            }
        }
    }
}
