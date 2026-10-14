//! Rendering and pointer/keyboard input collection.

use std::sync::Arc;

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Galley, Key, Order, Pos2, Rect, Sense, Shape,
    Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

use crate::app::{
    App, Cmd, Focus, Menu, Osk, OskKey, Overlay, Pane, Props, OSK_LAYOUTS, fmt_time, human_size,
};
use crate::config::{Config, ViewMode};
use crate::i18n::tr;
use crate::icons::Icons;
use crate::input::{Action, Input, PadKind};

/// Top bar button size and the icon inside it.
const TOOL_BTN: f32 = 38.0;
const TOOL_ICON: f32 = 24.0;
/// Tab height; L1/R1 glyphs next to the tabs match it.
const TAB_H: f32 = 30.0;
/// Label sizes follow icon sizes. Sidebar at the default 20px icons:
/// items 13.6, section headers ("Places", "Pinned") 15.2.
fn side_base(icon: f32) -> f32 {
    (10.0 + icon * 0.3).clamp(12.0, 28.0)
}
fn side_font(icon: f32) -> f32 {
    side_base(icon) * 0.85
}
fn side_section_font(icon: f32) -> f32 {
    side_base(icon) * 0.95
}
fn list_font(icon: f32) -> f32 {
    (7.4 + icon * 0.3).clamp(12.0, 26.0)
}
fn grid_font(icon: f32) -> f32 {
    (8.5 + icon * 0.08).clamp(11.0, 22.0)
}
/// Theme size asked for small icons. Many themes (Breeze, Win11) draw their
/// 16–24px sizes as monochrome glyphs; 32 gets the full-colour ones and is
/// sharper on HiDPI anyway.
const SMALL_ICON_LOOKUP: u16 = 32;
const SIDE_W: f32 = 210.0;

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

        // A video full screen: the window too, and no chrome around it.
        let full = self.video_full();
        if full != self.full_sent {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(full));
            self.full_sent = full;
        }
        if !full {
            self.draw_bottom(ui);
            self.draw_top(ui);
            self.draw_sidebar(ui);
        }
        self.draw_center(ui);
        self.draw_drag(&ctx);
        self.draw_overlay(&ctx);

        // Closing the window with unsaved text asks first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit {
            let dirty = (0..self.tabs.len()).find(|&i| self.confirm_unsaved(i, crate::viewer::glue::Then::Quit));
            if dirty.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
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
        // The mouse's back / forward buttons.
        let (back, fwd) = ctx.input(|i| (i.pointer.button_pressed(egui::PointerButton::Extra1), i.pointer.button_pressed(egui::PointerButton::Extra2)));
        if (back || fwd) && self.overlay.is_none() {
            self.focus = Focus::Panes;
            self.handle(Input::Press(if back { Action::HistoryBack } else { Action::HistoryForward }));
        }
        if self.range.as_ref().is_some_and(|r| r.keyboard) && !ctx.input(|i| i.modifiers.shift) {
            self.range = None;
        }
        let mut events = ctx.input(|i| i.events.clone());
        let osk = matches!(self.overlay, Some(Overlay::Osk(_)));
        // A text file open: the editor takes the keys first.
        if self.overlay.is_none() && self.focus == Focus::Panes && self.cur_tab().viewer.as_ref().is_some_and(|v| v.is_text()) {
            events = self.editor_keyboard(events);
        }
        if self.overlay.is_none() && self.focus == Focus::Panes && self.audio_shown() {
            events = self.audio_keyboard(events);
        }
        if self.overlay.is_none() && self.focus == Focus::Panes && self.video_shown() {
            events = self.video_keyboard(events);
        }
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
        // Shift + movement selects a range, like holding □.
        let moving = matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End | Key::PageUp | Key::PageDown);
        if m.shift && moving && self.overlay.is_none() && self.focus == Focus::Panes && self.range.is_none() && self.cur_tab().viewer.is_none() {
            self.begin_range(true);
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
        let mut act: Option<TopAction> = None;
        let dual = cfg.dual;
        let active = self.tabs[self.tab].active;
        let frame = egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 6));
        egui::Panel::top("top").frame(frame).show(ui, |ui| {
            let full = ui.available_rect_before_wrap();
            let row = Rect::from_min_size(full.min, vec2(full.width(), TOOL_BTN));
            ui.allocate_rect(row, Sense::hover());
            let btn = |x: f32| Rect::from_min_size(pos2(x, row.min.y), Vec2::splat(TOOL_BTN));
            let step = TOOL_BTN + 4.0;

            // Left: sidebar toggle, then history / up.
            let side_names: &[&str] = if cfg.sidebar_collapsed {
                &["sidebar-expand-left", "view-left-new", "sidebar-show", "view-sidebar"]
            } else {
                &["sidebar-collapse-left", "view-left-close", "sidebar-show", "view-sidebar"]
            };
            let mut x = row.min.x;
            let left: [(&[&str], &str, &str, TopAction); 4] = [
                (side_names, "☰", tr("Sidebar"), TopAction::Sidebar),
                (&["go-previous", "arrow-left"], "◀", tr("Back"), TopAction::Back),
                (&["go-next", "arrow-right"], "▶", tr("Forward"), TopAction::Forward),
                (&["go-up", "arrow-up"], "▲", tr("Up"), TopAction::Up),
            ];
            for (i, (names, fb, tip, a)) in left.into_iter().enumerate() {
                if tool_button(ui, btn(x), &mut self.icons, names, fb, tip, false).clicked() {
                    act = Some(a);
                }
                x += step + if i == 0 { 8.0 } else { 0.0 };
            }
            let addr_start = x + 6.0;

            // Right, pinned: search, dual pane, menu.
            let mut rx = row.max.x - TOOL_BTN;
            let right: [(&[&str], &str, &str, bool, TopAction); 3] = [
                (&["open-menu", "application-menu", "view-more-horizontal"], "≡", tr("Menu"), false, TopAction::Menu),
                (&["view-split-left-right", "view-dual"], "◫", tr("Dual pane"), dual, TopAction::Dual),
                (&["edit-find", "system-search"], "⌕", tr("Search"), false, TopAction::Search),
            ];
            for (names, fb, tip, on, a) in right {
                if tool_button(ui, btn(rx), &mut self.icons, names, fb, tip, on).clicked() {
                    act = Some(a);
                }
                rx -= step;
            }
            let addr_end = rx + TOOL_BTN - 6.0;

            // Address field(s); in dual mode split where the panes split.
            let viewing = self.tabs[self.tab].viewer.as_ref().map(|v| (v.origin, v.name.clone()));
            let fields: Vec<(usize, Rect)> = if let Some((origin, _)) = &viewing {
                vec![(*origin, Rect::from_min_max(pos2(addr_start, row.min.y), pos2(addr_end, row.max.y)))]
            } else if dual {
                let mid = self
                    .divider_x
                    .filter(|d| *d > addr_start + 120.0 && *d < addr_end - 120.0)
                    .unwrap_or((addr_start + addr_end) / 2.0);
                vec![
                    (0, Rect::from_min_max(pos2(addr_start, row.min.y), pos2(mid - 4.0, row.max.y))),
                    (1, Rect::from_min_max(pos2(mid + 4.0, row.min.y), pos2(addr_end, row.max.y))),
                ]
            } else {
                vec![(0, Rect::from_min_max(pos2(addr_start, row.min.y), pos2(addr_end, row.max.y)))]
            };
            for (idx, rect) in fields {
                let pane = &self.tabs[self.tab].panes[idx];
                let highlight = dual && idx == active && viewing.is_none();
                let file = viewing.as_ref().map(|(_, n)| n.as_str());
                if let Some(a) = address_field(ui, rect, pane, highlight, idx, file) {
                    act = Some(a);
                }
            }
        });

        let Some(act) = act else { return };
        match act {
            TopAction::Sidebar => self.run(Cmd::ToggleSidebar),
            TopAction::Back | TopAction::Forward | TopAction::Up => {
                self.focus = Focus::Panes;
                let a = match act {
                    TopAction::Back => Action::HistoryBack,
                    TopAction::Forward => Action::HistoryForward,
                    _ => Action::Back,
                };
                if a == Action::Back {
                    self.pane_mut().go_up(&cfg);
                } else {
                    self.handle(Input::Press(a));
                }
            }
            TopAction::Search => {
                self.run(if self.pane().loc.local_path().is_some() { Cmd::Search } else { Cmd::Filter });
            }
            TopAction::Dual => self.run(Cmd::ToggleDual),
            TopAction::Menu => {
                self.focus = Focus::Panes;
                self.handle(Input::Press(Action::Menu));
            }
            TopAction::Focus(i) => {
                self.focus = Focus::Panes;
                self.tabs[self.tab].active = i;
            }
            TopAction::Go(i, loc) => {
                self.focus = Focus::Panes;
                self.tabs[self.tab].active = i;
                self.pane_mut().navigate(loc, None, &cfg);
            }
            TopAction::ClearFilter(i) => {
                self.tabs[self.tab].active = i;
                self.pane_mut().set_filter(None);
            }
        }
    }

    /// Tabs strip at the top of the file area: L1, tabs, "+", R1 centred
    /// in a fixed-height row (laid out by hand so everything shares one
    /// vertical centre).
    /// Returns the click instead of acting on it: the caller is still
    /// drawing the current tab, so closing it now would pull it away
    /// mid-frame (that crashed Polka when the last-but-one tab closed).
    fn draw_tabs(&mut self, ui: &mut Ui) -> Option<TabAct> {
        let dual = self.cfg.dual;
        let mut switch = None;
        let mut close = None;
        let mut new_tab = false;
        let v = ui.visuals().clone();
        let many = self.tabs.len() > 1;
        let gap = 6.0;
        let shoulder_w = TAB_H * 1.4;

        let titles: Vec<_> = self
            .tabs
            .iter()
            .map(|t| {
                let p = &t.panes[if dual { t.active } else { 0 }];
                let usage = t.usage.as_ref().is_some_and(|u| u.shown);
                let title = t.viewer.as_ref().map_or_else(|| if usage { tr("Disk usage").to_string() } else { p.loc.title() }, |v| v.name.clone());
                elided(ui, &title, FontId::proportional(14.0), v.text_color(), 180.0, 1)
            })
            .collect();
        let close_w = if many { 20.0 } else { 0.0 };
        let widths: Vec<f32> = titles.iter().map(|g| (g.size().x + 28.0 + close_w).max(96.0)).collect();
        let total = shoulder_w * 2.0 + widths.iter().sum::<f32>() + TAB_H + gap * (widths.len() + 2) as f32;

        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), TAB_H), Sense::hover());
        let mut x = (row.center().x - total / 2.0).max(row.min.x);
        let at = |x: f32, w: f32| Rect::from_min_size(pos2(x, row.min.y), vec2(w, TAB_H));
        let painter = ui.painter().with_clip_rect(row.expand(2.0));

        shoulder_glyph(&painter, at(x, shoulder_w), Action::PrevTab, self.pad);
        x += shoulder_w + gap;
        for (i, (title, w)) in titles.into_iter().zip(widths).enumerate() {
            let rect = at(x, w);
            x += w + gap;
            let is_active = i == self.tab;
            let r = ui.interact(rect, ui.id().with(("tab", i)), Sense::click());
            let accent = v.selection.bg_fill;
            let (fill, stroke) = if is_active {
                (tint(&v, 0.35), Stroke::new(1.5, accent))
            } else if r.hovered() {
                (v.widgets.hovered.weak_bg_fill, v.widgets.hovered.bg_stroke)
            } else {
                (v.widgets.inactive.weak_bg_fill.gamma_multiply(0.6), v.widgets.noninteractive.bg_stroke)
            };
            painter.rect(rect, 7.0, fill, stroke, StrokeKind::Inside);
            painter.galley(pos2(rect.min.x + 14.0, rect.center().y - title.size().y / 2.0), title, v.text_color());
            if many {
                let cr = Rect::from_center_size(pos2(rect.max.x - 14.0, rect.center().y), Vec2::splat(18.0));
                let cresp = ui.interact(cr, ui.id().with(("tab-close", i)), Sense::click());
                if is_active || r.hovered() || cresp.hovered() {
                    if cresp.hovered() {
                        painter.circle_filled(cr.center(), 9.0, v.widgets.hovered.bg_fill);
                    }
                    painter.text(cr.center(), Align2::CENTER_CENTER, "×", FontId::proportional(15.0), v.weak_text_color());
                }
                if cresp.clicked() {
                    close = Some(i);
                    continue;
                }
            }
            if r.clicked() {
                switch = Some(i);
            }
            if r.middle_clicked() {
                close = Some(i);
            }
        }
        let plus = at(x, TAB_H);
        x += TAB_H + gap;
        let r = ui.interact(plus, ui.id().with("tab-new"), Sense::click());
        painter.rect(
            plus,
            7.0,
            if r.hovered() { v.widgets.hovered.weak_bg_fill } else { Color32::TRANSPARENT },
            v.widgets.noninteractive.bg_stroke,
            StrokeKind::Inside,
        );
        painter.text(plus.center(), Align2::CENTER_CENTER, "+", FontId::proportional(18.0), v.text_color());
        if r.on_hover_text(tr("New tab")).clicked() {
            new_tab = true;
        }
        shoulder_glyph(&painter, at(x, shoulder_w), Action::NextTab, self.pad);
        ui.add_space(6.0);

        if let Some(i) = close {
            Some(TabAct::Close(i))
        } else if let Some(i) = switch {
            Some(TabAct::Switch(i))
        } else if new_tab {
            Some(TabAct::New)
        } else {
            None
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
                if let Some(m) = p.current().and_then(|e| p.volumes.get(&e.name)) {
                    ui.separator();
                    let t = format!("{} {} {} {} · {}", tr("Volume"), m.index + 1, tr("of"), m.count, m.archive);
                    ui.label(egui::RichText::new(t).color(volume_color(ui.visuals())));
                    if m.gap {
                        ui.label(egui::RichText::new(tr("some volumes are missing")).color(ui.visuals().error_fg_color));
                    }
                }
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
            Some(Overlay::Menu(_)) => vec![(Action::Accept, tr("OK")), (Action::Back, tr("Back"))],
            Some(Overlay::Osk(_)) => vec![
                (Action::Accept, "⌨"),
                (Action::Select, tr("Backspace")),
                (Action::Actions, tr("Space")),
                (Action::LeftTrigger, tr("Shift")),
                (Action::View, tr("Layout")),
                (Action::Menu, tr("OK")),
                (Action::Back, tr("Cancel")),
            ],
            Some(Overlay::Settings(st)) if st.editor.is_some() => {
                vec![(Action::Left, tr("Adjust")), (Action::Accept, tr("OK")), (Action::Back, tr("Cancel"))]
            }
            Some(Overlay::Settings(_)) => vec![
                (Action::Left, tr("Adjust")),
                (Action::Accept, tr("OK")),
                (Action::Actions, tr("Delete color")),
                (Action::Back, tr("Close")),
            ],
            Some(Overlay::ViewerSettings(_)) => vec![
                (Action::Left, tr("Adjust")),
                (Action::Accept, tr("OK")),
                (Action::Back, tr("Close")),
            ],
            Some(Overlay::Compress(_)) => vec![
                (Action::Left, tr("Adjust")),
                (Action::Accept, tr("OK")),
                (Action::Menu, tr("Create")),
                (Action::Back, tr("Cancel")),
            ],
            Some(Overlay::IconPicker(_)) => vec![
                (Action::PrevTab, tr("Color")),
                (Action::Accept, tr("OK")),
                (Action::Actions, tr("Own picture…")),
                (Action::Select, tr("Reset")),
                (Action::Back, tr("Cancel")),
            ],
            Some(_) => vec![(Action::Back, tr("Close"))],
            None if self.focus == Focus::Panes && self.text_editing() => vec![
                (Action::Up, tr("Move")),
                (Action::Accept, tr("Type in")),
                (Action::Select, tr("Erase / select")),
                (Action::Actions, tr("Actions")),
                (Action::Back, tr("Done")),
            ],
            None if self.cur_tab().viewer.as_ref().is_some_and(|v| v.is_text()) && self.focus == Focus::Panes => vec![
                (Action::Accept, tr("Edit")),
                (Action::Up, tr("Scroll")),
                (Action::LeftTrigger, tr("Page")),
                (Action::Actions, tr("Actions")),
                (Action::Menu, tr("Settings")),
                (Action::Back, tr("Close")),
            ],
            None if self.video_shown() && self.focus == Focus::Panes => vec![
                (Action::Accept, tr("Play / pause")),
                (Action::Left, tr("Seek")),
                (Action::Up, tr("Sound volume")),
                (Action::LeftTrigger, tr("Video")),
                (Action::View, tr("Full screen")),
                (Action::Select, tr("Subtitles")),
                (Action::Actions, tr("Actions")),
                (Action::Back, tr("Close")),
            ],
            None if self.audio_shown() && self.focus == Focus::Panes => vec![
                (Action::Accept, tr("Play / pause")),
                (Action::Left, tr("Seek")),
                (Action::LeftTrigger, tr("Track")),
                (Action::Select, tr("Repeat; hold: volume")),
                (Action::Actions, tr("Actions")),
                (Action::Back, tr("Close")),
            ],
            None if self.cur_tab().viewer.is_some() && self.focus == Focus::Panes => vec![
                (Action::Left, tr("Previous / next")),
                (Action::LeftTrigger, tr("Zoom")),
                (Action::Accept, "1:1"),
                (Action::Actions, tr("Actions")),
                (Action::Back, tr("Close")),
            ],
            None if self.usage_shown() && self.focus == Focus::Panes => self.usage_hints(),
            None if self.icon_pick.is_some() => vec![(Action::Accept, tr("Choose")), (Action::Actions, tr("Cancel")), (Action::Back, tr("Back"))],
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
        let usage_here = self.usage_shown();
        let dragging = self.drag.is_some();
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let released = ui.input(|i| i.pointer.primary_released());
        let mut dropped = None;
        let mut clicked = None;
        let mut actions = None;
        let side_icon = self.cfg.side_icon;
        let side_font = side_font(side_icon);
        egui::Panel::left("sidebar")
            .resizable(false)
            .exact_size(if collapsed { side_icon + 32.0 } else { SIDE_W + (side_icon - 20.0).max(0.0) * 3.0 })
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
                                ui.label(egui::RichText::new(tr(section)).size(side_section_font(side_icon)).weak());
                            }
                        }
                        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), (side_icon + 12.0).max(side_font + 18.0)), Sense::click());
                        let is_cursor = focused && i == self.side_cursor;
                        let here = if it.usage { usage_here } else { it.loc == cur_loc && !usage_here };
                        paint_row_bg(ui, rect, is_cursor, here, r.hovered());
                        if dragging && !it.usage && pointer.is_some_and(|p| rect.contains(p)) {
                            ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, ui.visuals().selection.bg_fill), StrokeKind::Inside);
                            if released {
                                dropped = Some(i);
                            }
                        }
                        let under = dragging && !it.usage && pointer.is_some_and(|p| rect.contains(p));
                        let grow = 1.0 + 0.15 * ui.ctx().animate_bool_with_time(ui.id().with(("side-grow", i)), under, 0.12);
                        let icon_rect = Rect::from_center_size(pos2(rect.min.x + 8.0 + side_icon / 2.0, rect.center().y), Vec2::splat(side_icon * grow));
                        let px = lookup_px(ui, side_icon);
                        // Custom icons are theme names or image files we stored.
                        let custom = it.custom_icon.as_deref().and_then(|c| {
                            let p = std::path::Path::new(c);
                            if p.is_absolute() { p.exists().then(|| crate::icons::uri(p)) } else { self.icons.named(&[c], px) }
                        });
                        if let Some(u) = custom.or_else(|| self.icons.named(it.icon, px)) {
                            egui::Image::new(u).paint_at(ui, icon_rect);
                        }
                        if !collapsed {
                            let tx = side_icon + 16.0;
                            let g = elided(ui, &it.label, FontId::proportional(side_font), ui.visuals().text_color(), rect.width() - tx - 8.0, 1);
                            ui.painter().galley(pos2(rect.min.x + tx, rect.center().y - g.size().y / 2.0), g, Color32::WHITE);
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
        if let Some(i) = dropped {
            self.drop_on_side(i);
        }
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
        let mut divider = None;
        let mut tab_act = None;
        let mut viewer_act = None;
        let mut usage_act = None;
        let full = self.video_full();
        let frame = if full { egui::Frame::NONE.fill(egui::Color32::BLACK) } else { egui::Frame::central_panel(ui.style()) };
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            if !full {
                self.draw_icon_pick_banner(ui);
                tab_act = self.draw_tabs(ui);
            }
            if let Some(v) = self.tabs[tab].viewer.as_mut() {
                let theme = crate::viewer::theme::resolve(&self.cfg.viewer.theme, ui.visuals().dark_mode);
                viewer_act = crate::viewer::draw::draw(ui, v, &self.cfg.viewer, &theme);
                return;
            }
            if let Some(u) = self.tabs[tab].usage.as_deref_mut().filter(|u| u.shown) {
                usage_act = crate::usage::draw::draw(ui, u, &mut self.icons);
                return;
            }
            let dragging = self.drag.is_some();
            let App { tabs, icons, cfg, drag, .. } = self;
            let lifted = |pane: usize| drag.as_ref().filter(|d| d.from == (tab, pane)).map(|d| &d.names);
            let [a, b] = &mut tabs[tab].panes;
            if dual {
                let full = ui.available_rect_before_wrap();
                let gap = 6.0;
                let w = (full.width() - gap) / 2.0;
                let ra = Rect::from_min_size(full.min, vec2(w, full.height()));
                let rb = Rect::from_min_size(pos2(full.min.x + w + gap, full.min.y), vec2(w, full.height()));
                divider = Some(full.min.x + w + gap / 2.0);
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
                    for e in draw_pane(&mut inner, pane, icons, cfg, is_active && focused, (tab, idx), dragging, lifted(idx)) {
                        events.push((idx, e));
                    }
                }
            } else {
                for e in draw_pane(ui, a, icons, cfg, focused, (tab, 0), dragging, lifted(0)) {
                    events.push((0, e));
                }
            }
        });
        if let Some(a) = viewer_act {
            use crate::viewer::draw::HeaderAct;
            match a {
                HeaderAct::Close => self.request_close_viewer(),
                HeaderAct::Settings => self.run(Cmd::ViewerSettings),
                HeaderAct::External => self.run(Cmd::OpenExternal),
                HeaderAct::Save => self.run(Cmd::ViewerSave),
                HeaderAct::ToggleMd => self.run(Cmd::ViewerToggleMd),
                HeaderAct::Link(l) => self.viewer_link(l),
                HeaderAct::Run(c) => self.run(c),
                HeaderAct::Volume(v) => self.audio_set_volume(v),
            }
        }
        if let Some(a) = usage_act {
            self.usage_act(a);
        }
        let usage = self.tabs[tab].usage.as_ref().is_some_and(|u| u.shown);
        let divider = if self.tabs[tab].viewer.is_some() || usage { self.divider_x } else { divider };
        if divider != self.divider_x {
            // The address bar lines up with this on the next frame.
            self.divider_x = divider;
            ui.ctx().request_repaint();
        }
        for (idx, e) in events {
            self.focus = Focus::Panes;
            self.tabs[tab].active = idx;
            match e {
                PaneEvent::Click(i, toggle, range) => {
                    let p = self.pane_mut();
                    p.cursor_hidden = false;
                    if range {
                        let (a, b) = (p.cursor.min(i), p.cursor.max(i));
                        if !toggle {
                            p.selected.clear();
                        }
                        let names: Vec<String> = p.entries[a..=b.min(p.entries.len() - 1)].iter().map(|e| e.name.clone()).collect();
                        p.selected.extend(names);
                    }
                    p.cursor = i;
                    if toggle && !range {
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
                PaneEvent::DragStart(i) => self.drag_start(idx, i),
                PaneEvent::Drop(i) => self.drop_on_pane(idx, i),
                PaneEvent::ClearSelection => self.pane_mut().selected.clear(),
                PaneEvent::NewTab(i) => {
                    let p = self.pane();
                    if let Some(loc) = p.entries.get(i).map(|e| p.entry_loc(e)) {
                        // A middle click opens it beside, not instead.
                        let here = self.tab;
                        self.run(Cmd::OpenInNewTab(loc));
                        self.tab = here;
                    }
                }
            }
        }
        match tab_act {
            Some(TabAct::Switch(i)) => self.tab = i,
            Some(TabAct::Close(i)) => self.run(Cmd::CloseTabAt(i)),
            Some(TabAct::New) => self.run(Cmd::NewTab),
            None => {}
        }
    }

    /// Shown while choosing a picture for a pinned folder.
    fn draw_icon_pick_banner(&mut self, ui: &mut Ui) {
        let Some(pick) = &self.icon_pick else { return };
        let name = pick.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut cancel = false;
        let v = ui.visuals().clone();
        egui::Frame::new()
            .fill(tint(&v, 0.3))
            .stroke(Stroke::new(1.0, v.selection.bg_fill))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{} «{name}»", tr("Choose a picture for"))).size(15.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(tr("Cancel")).clicked() {
                            cancel = true;
                        }
                        glyph(ui, Action::Actions, self.pad);
                        ui.add_space(10.0);
                        ui.label(tr("Choose"));
                        glyph(ui, Action::Accept, self.pad);
                    });
                });
            });
        ui.add_space(6.0);
        if cancel {
            self.finish_icon_pick(None);
        }
    }

    // ------------------------------------------------------------ overlays

    fn draw_overlay(&mut self, ctx: &egui::Context) {
        if self.overlay.is_none() {
            return;
        }
        let screen = ctx.content_rect();
        let mut close = false;
        // Viewer settings sit beside the file so changes show live.
        let side = matches!(self.overlay, Some(Overlay::ViewerSettings(_)));
        egui::Area::new("backdrop".into()).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(if side { 0 } else { 140 }));
            close = resp.clicked();
        });
        if close {
            self.overlay = None;
            return;
        }
        let pad = self.pad;
        let mut run: Option<Cmd> = None;
        let mut menu_click: Option<(usize, usize)> = None;
        let mut osk_key: Option<OskKey> = None;
        let mut dismiss = false;
        let mut settings_ev: Option<SettingsEvent> = None;
        let mut compress_ev: Option<(usize, isize)> = None;
        let mut viewer_ev: Option<(usize, isize)> = None;
        let (overlay, icons, cfg) = (&mut self.overlay, &mut self.icons, &self.cfg);
        let (anchor, offset) = if side { (Align2::RIGHT_TOP, vec2(-16.0, 110.0)) } else { (Align2::CENTER_CENTER, vec2(0.0, 0.0)) };
        let vprefs = self.cfg.viewer.clone();
        // One area per kind of overlay: an area keeps its last size, and a
        // small menu's would squeeze the settings' scroll area after it.
        let kind = overlay.as_ref().map(std::mem::discriminant);
        egui::Area::new(egui::Id::new(("overlay", kind)))
            .order(Order::Foreground)
            .anchor(anchor, offset)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(14.0).corner_radius(10.0).show(ui, |ui| match overlay.as_mut() {
                    Some(Overlay::Menu(m)) => menu_click = draw_menu_cascade(ui, m),
                    Some(Overlay::Osk(k)) => osk_key = draw_osk(ui, k),
                    Some(Overlay::Props(p)) => dismiss = draw_props(ui, p),
                    Some(Overlay::Help) => dismiss = draw_help(ui, pad),
                    Some(Overlay::Themes(t)) => run = draw_themes(ui, t, icons),
                    Some(Overlay::Settings(st)) => {
                        // Taller than small windows: scroll.
                        let max_h = (screen.height() - 120.0).max(240.0);
                        settings_ev = egui::ScrollArea::vertical().id_salt("settings-scroll").max_height(max_h).show(ui, |ui| draw_settings(ui, st, cfg, icons)).inner;
                    }
                    Some(Overlay::IconPicker(p)) => run = draw_icon_picker(ui, p, icons, pad),
                    Some(Overlay::Compress(d)) => compress_ev = draw_compress(ui, d, pad),
                    Some(Overlay::ViewerSettings(st)) => viewer_ev = crate::viewer::draw::draw_settings(ui, st, &vprefs),
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
        if let Some((depth, index)) = menu_click {
            self.menu_click(depth, index);
        }
        if let Some(ev) = settings_ev {
            self.settings_event(ev);
        }
        if let Some((row, d)) = viewer_ev {
            self.viewer_step(row, d);
        }
        if let Some((row, d)) = compress_ev {
            if d == 0 {
                self.compress_activate(row);
            } else if let Some(Overlay::Compress(dlg)) = self.overlay.as_mut() {
                dlg.row = row;
                dlg.step(row, d);
            }
        }
    }
}

// ---------------------------------------------------------------- pane view

enum PaneEvent {
    /// index, Ctrl (toggle), Shift (range from the cursor)
    Click(usize, bool, bool),
    /// Middle click: a folder in a new tab.
    NewTab(usize),
    Activate(usize),
    Context(Option<usize>),
    /// Items picked up by the mouse (from this entry).
    DragStart(usize),
    /// Dragged items let go over this pane (onto an entry, or not).
    Drop(Option<usize>),
    /// A click on empty space.
    ClearSelection,
}

fn draw_pane(ui: &mut Ui, pane: &mut Pane, icons: &mut Icons, cfg: &Config, focused: bool, id: (usize, usize), dragging: bool, lifted: Option<&std::collections::HashSet<String>>) -> Vec<PaneEvent> {
    let mut events = Vec::new();
    // Dragged items over this pane: dropped on release.
    let whole = ui.max_rect();
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let over = dragging && pointer.is_some_and(|p| whole.contains(p));
    let released = ui.input(|i| i.pointer.primary_released());
    let mut drop_entry = None;
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
        if over {
            ui.painter().rect_stroke(whole, 6.0, Stroke::new(2.0, ui.visuals().selection.bg_fill), StrokeKind::Inside);
            if released {
                events.push(PaneEvent::Drop(None));
            }
        }
        return events;
    }

    let grid = cfg.view == ViewMode::Grid;
    let width = ui.available_width();
    let (list_icon, grid_icon) = (cfg.list_icon, cfg.grid_icon);
    let (list_font, grid_font) = (list_font(list_icon), grid_font(grid_icon));
    // Size/date columns scale with the font so wide fonts still fit.
    let k = list_font / 14.0;
    let grid_w = (grid_icon + 44.0).max(96.0).max(grid_font * 7.5);
    let grid_h = grid_icon + 18.0 + 2.0 * grid_font * 1.3;
    let list_row = (list_icon + 8.0).max(list_font + 14.0);
    let name_x = list_icon + 20.0;
    // Narrow panes (dual mode, big fonts) drop the date column first, so
    // names keep room. Column x positions are offsets from the right edge.
    let show_date = width > 560.0 * k;
    let size_right = if show_date { 160.0 * k } else { 12.0 };
    let name_room = if show_date { 260.0 * k } else { 100.0 * k };
    let cols = if grid { ((width - 8.0) / grid_w).floor().max(1.0) as usize } else { 1 };
    pane.cols = cols;
    let row_h = if grid { grid_h } else { list_row };
    let rows = pane.entries.len().div_ceil(cols);

    if !grid {
        // Column header.
        let (r, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
        let c = ui.visuals().weak_text_color();
        let f = FontId::proportional(12.0);
        ui.painter().text(pos2(r.min.x + name_x, r.center().y), Align2::LEFT_CENTER, tr("Name"), f.clone(), c);
        ui.painter().text(pos2(r.max.x - size_right, r.center().y), Align2::RIGHT_CENTER, tr("Size"), f.clone(), c);
        if show_date {
            ui.painter().text(pos2(r.max.x - 145.0 * k, r.center().y), Align2::LEFT_CENTER, tr("Modified"), f, c);
        }
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
    // The volume set under the mouse (last frame), else under the cursor.
    let active_set = pane.hover_volume.take().or_else(|| {
        focused.then(|| pane.current().and_then(|e| pane.volumes.get(&e.name)).map(|m| m.key.clone())).flatten()
    });
    let mut hover_set = None;
    let vol_c = volume_color(ui.visuals());
    let local = pane.loc.local_path().is_some();
    pane.drawn.clear();
    let mut band_from: Option<Pos2> = None;
    let out = sa.show_rows(ui, row_h, rows, |ui, range| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        // Content coordinates: row r starts r * row_h below this.
        let origin = ui.cursor().min - vec2(0.0, range.start as f32 * row_h);
        // (A vertical scroll area's clip spans the window's width; the other
        // pane must stay clickable.)
        let view = ui.clip_rect().intersect(whole);
        // Empty space under the entries: clicks clear, drags draw a band.
        let bg = ui.interact(view, ui.id().with(("pane-bg", id)), Sense::click_and_drag());
        if bg.drag_started() {
            band_from = ui.input(|i| i.pointer.press_origin());
        } else if bg.clicked() && !ui.input(|i| i.modifiers.command) {
            events.push(PaneEvent::ClearSelection);
        } else if bg.secondary_clicked() {
            events.push(PaneEvent::Context(None));
        }
        for row in range {
            let (row_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
            for col in 0..cols {
                let i = row * cols + col;
                let Some(e) = pane.entries.get(i) else { break };
                let rect = if grid {
                    Rect::from_min_size(row_rect.min + vec2(col as f32 * grid_w + 4.0, 0.0), vec2(grid_w - 4.0, grid_h - 4.0))
                } else {
                    row_rect
                };
                let r = ui.interact(rect, ui.id().with(("entry", i)), Sense::click_and_drag());
                let sel = pane.selected.contains(&e.name);
                let is_cursor = i == pane.cursor && !pane.cursor_hidden;
                let vol = pane.volumes.get(&e.name);
                // A folded volume set: the archive's name, all volumes' size.
                let folded = pane.collapsed.get(&e.name);
                let label = folded.map_or(e.name.as_str(), |f| f.0.archive.as_str());
                let in_set = vol.is_some_and(|m| active_set.as_ref() == Some(&m.key)) && folded.is_none();
                if let (Some(m), true) = (vol, r.hovered()) {
                    hover_set = Some(m.key.clone());
                }
                if in_set {
                    ui.painter().rect_filled(rect, 6.0, vol_c.gamma_multiply(0.22));
                }
                paint_entry_bg(ui, rect, is_cursor, focused, sel, r.hovered() && !in_set);
                if in_set {
                    if grid {
                        ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.5, vol_c), StrokeKind::Inside);
                    } else {
                        let bar = Rect::from_min_size(rect.min + vec2(1.0, 2.0), vec2(4.0, rect.height() - 4.0));
                        ui.painter().rect_filled(bar, 2.0, vol_c);
                    }
                }
                let path = pane.loc.entry_path(&e.name);
                let hot;
                let icon_size = if grid { grid_icon } else { list_icon };
                let thumb = match (&path, e.mtime) {
                    (Some(p), Some(t)) if cfg.thumbnails && !e.is_dir => icons.thumb(p, t),
                    _ => None,
                };
                let icon = if thumb.is_some() { None } else { icons.for_entry(e, path.as_deref(), lookup_px(ui, icon_size)) };
                let text_c = if e.hidden() { ui.visuals().weak_text_color() } else { ui.visuals().text_color() };
                // While dragging: the items picked up fade where they lie, and
                // the icon under the pointer grows a little.
                let lifted = lifted.is_some_and(|l| l.contains(&e.name));
                let alpha = if lifted { 0.35 } else { 1.0 };
                let under = dragging && !lifted && pointer.is_some_and(|p| rect.contains(p));
                let grow = 1.0 + 0.15 * ui.ctx().animate_bool_with_time(ui.id().with(("grow", i)), under, 0.12);
                let big = |r: Rect| Rect::from_center_size(r.center(), r.size() * grow);
                if grid {
                    let ir = Rect::from_center_size(pos2(rect.center().x, rect.min.y + 6.0 + icon_size / 2.0), Vec2::splat(icon_size));
                    paint_icon(ui, big(ir), icon, thumb, alpha);
                    lock_badge(ui, ir, e, path.as_deref());
                    let g = elided_centered(ui, label, FontId::proportional(grid_font), text_c, rect.width() - 6.0, 2);
                    hot = ir.union(Rect::from_center_size(pos2(rect.center().x, ir.max.y + 4.0 + g.size().y / 2.0), g.size()));
                    ui.painter().galley(pos2(rect.center().x, ir.max.y + 4.0), g, text_c);
                    if let Some(f) = folded {
                        let font = FontId::proportional((grid_font - 2.0).max(10.0));
                        stack_tag(ui, f.2.len(), pos2(ir.max.x + 2.0, ir.min.y + 2.0), Align2::RIGHT_TOP, font, vol_c);
                    } else if let Some(m) = vol {
                        let font = FontId::proportional((grid_font - 2.0).max(10.0));
                        let at = pos2(ir.max.x + 2.0, ir.min.y + 2.0);
                        volume_tag(ui, m, None, at, Align2::RIGHT_TOP, font, in_set, vol_c);
                    }
                } else {
                    let ir = Rect::from_min_size(pos2(rect.min.x + 10.0, rect.center().y - icon_size / 2.0), Vec2::splat(icon_size));
                    paint_icon(ui, big(ir), icon, thumb, alpha);
                    lock_badge(ui, ir, e, path.as_deref());
                    let tag_font = FontId::proportional((list_font - 2.0).max(10.0));
                    // Room for the volume tag: "archive · 2/4" on the active set.
                    let tag_w = match (folded, vol) {
                        (Some(f), _) => ui.painter().layout_no_wrap(crate::i18n::volumes(f.2.len()), tag_font.clone(), Color32::WHITE).size().x + 22.0,
                        (None, Some(m)) => {
                            let t = volume_tag_text(m, in_set.then_some(m.archive.as_str()));
                            ui.painter().layout_no_wrap(t, tag_font.clone(), Color32::WHITE).size().x + 22.0
                        }
                        _ => 0.0,
                    };
                    let name_w = rect.width() - name_x - name_room - tag_w;
                    let g = elided(ui, label, FontId::proportional(list_font), text_c, name_w.max(60.0), 1);
                    let name_end = rect.min.x + name_x + g.size().x;
                    hot = Rect::from_min_max(rect.min, pos2(name_end + 12.0, rect.max.y));
                    ui.painter().galley(pos2(rect.min.x + name_x, rect.center().y - g.size().y / 2.0), g, text_c);
                    if let Some(f) = folded {
                        stack_tag(ui, f.2.len(), pos2(name_end + 10.0, rect.center().y), Align2::LEFT_CENTER, tag_font.clone(), vol_c);
                    } else if let Some(m) = vol {
                        let at = pos2(name_end + 10.0, rect.center().y);
                        volume_tag(ui, m, in_set.then_some(m.archive.as_str()), at, Align2::LEFT_CENTER, tag_font, in_set, vol_c);
                    }
                    let weak = ui.visuals().weak_text_color();
                    let f = FontId::proportional(list_font - 1.0);
                    let size = if e.is_dir { String::new() } else { human_size(folded.map_or(e.size, |f| f.1)) };
                    ui.painter().text(pos2(rect.max.x - size_right, rect.center().y), Align2::RIGHT_CENTER, size, f.clone(), weak);
                    if let (true, Some(t)) = (show_date, e.mtime) {
                        ui.painter().text(pos2(rect.max.x - 145.0 * k, rect.center().y), Align2::LEFT_CENTER, fmt_time(t), f, weak);
                    }
                }
                pane.drawn.push((i, rect, hot));
                // Picked up by the icon or name; elsewhere a band starts.
                if r.drag_started() {
                    let o = ui.input(|i| i.pointer.press_origin());
                    if o.is_some_and(|o| hot.contains(o)) {
                        events.push(PaneEvent::DragStart(i));
                    } else {
                        band_from = o;
                    }
                }
                if dragging && pointer.is_some_and(|p| rect.contains(p)) && crate::mouse::drop_target(e, local) && !sel {
                    drop_entry = Some(i);
                    let a = ui.visuals().selection.bg_fill;
                    ui.painter().rect(rect, 6.0, tint(ui.visuals(), 0.35), Stroke::new(2.0, a), StrokeKind::Inside);
                }
                if r.double_clicked() {
                    events.push(PaneEvent::Activate(i));
                } else if r.clicked() {
                    let m = ui.input(|i| i.modifiers);
                    events.push(PaneEvent::Click(i, m.command, m.shift));
                } else if r.middle_clicked() && e.is_dir {
                    events.push(PaneEvent::NewTab(i));
                } else if r.secondary_clicked() {
                    events.push(PaneEvent::Context(Some(i)));
                }
            }
        }
        // The selection rectangle.
        let ctrl = ui.input(|i| i.modifiers.command);
        if let Some(o) = band_from.take() {
            pane.band = Some(crate::mouse::Band { start: o.to_vec2() - origin.to_vec2(), base: if ctrl { pane.selected.clone() } else { Default::default() } });
        }
        if let Some(b) = &pane.band {
            match ui.input(|i| i.pointer.primary_down().then(|| i.pointer.latest_pos()).flatten()) {
                Some(p) => {
                    let area = Rect::from_two_pos(b.start.to_pos2(), (p - origin).to_pos2());
                    let mut picked = b.base.clone();
                    for (i, e) in pane.entries.iter().enumerate() {
                        let (r, c) = ((i / cols) as f32, (i % cols) as f32);
                        let item = if grid {
                            Rect::from_min_size(pos2(c * grid_w + 4.0, r * row_h), vec2(grid_w - 4.0, grid_h - 4.0))
                        } else {
                            Rect::from_min_size(pos2(0.0, r * row_h), vec2(view.width(), row_h))
                        };
                        if item.intersects(area) {
                            picked.insert(e.name.clone());
                        }
                    }
                    pane.selected = picked;
                    let screen = area.translate(origin.to_vec2());
                    let a = ui.visuals().selection.bg_fill;
                    ui.painter().rect(screen, 2.0, a.gamma_multiply(0.15), Stroke::new(1.0, a), StrokeKind::Inside);
                    // Near the edges the list scrolls along.
                    let edge = 28.0;
                    let d = if p.y > view.max.y - edge { -(p.y - view.max.y + edge) } else if p.y < view.min.y + edge { view.min.y + edge - p.y } else { 0.0 };
                    if d != 0.0 {
                        ui.scroll_with_delta(vec2(0.0, d.clamp(-24.0, 24.0)));
                    }
                    ui.ctx().request_repaint();
                }
                None => pane.band = None,
            }
        }
    });
    if over {
        if drop_entry.is_none() {
            ui.painter().rect_stroke(whole, 6.0, Stroke::new(2.0, ui.visuals().selection.bg_fill), StrokeKind::Inside);
        }
        if released {
            events.push(PaneEvent::Drop(drop_entry));
        }
    }
    // Sets under the mouse or the cursor unfold (from the next frame).
    let mut open: Vec<String> = hover_set.iter().cloned().collect();
    if focused {
        if let Some(k) = pane.current().and_then(|e| pane.volumes.get(&e.name)).map(|m| m.key.clone()) {
            if !open.contains(&k) {
                open.push(k);
            }
        }
    }
    pane.set_expanded(open);
    pane.hover_volume = hover_set;
    pane.scroll_y = out.state.offset.y;
    pane.view_h = out.inner_rect.height();
    pane.page_rows = ((pane.view_h / row_h) as usize).saturating_sub(1).max(1);
    events
}

/// The accent as a fill behind text: lighter on a light background, where
/// the dark look's translucency would turn grey.
pub(crate) fn tint(v: &egui::Visuals, k: f32) -> Color32 {
    v.selection.bg_fill.gamma_multiply(if v.dark_mode { k } else { k * 0.5 })
}

/// Highlight for the volumes of one archive: the accent's complement, so it
/// never reads as selection.
fn volume_color(v: &egui::Visuals) -> Color32 {
    let mut c = egui::ecolor::Hsva::from(v.selection.bg_fill);
    c.h = (c.h + 0.5).fract();
    c.s = c.s.max(0.6);
    c.v = c.v.max(0.85);
    c.into()
}

/// "▤ 4 volumes" on a folded set.
fn stack_tag(ui: &Ui, n: usize, at: Pos2, align: Align2, font: FontId, c: Color32) {
    let v = ui.visuals();
    let g = ui.painter().layout_no_wrap(crate::i18n::volumes(n), font, v.text_color());
    let size = g.size() + vec2(22.0, 4.0);
    let rect = align.anchor_size(at, size);
    ui.painter().rect(rect, rect.height() / 2.0, v.window_fill, Stroke::new(1.0, c.gamma_multiply(0.7)), StrokeKind::Inside);
    let cy = rect.center().y;
    for k in 0..3 {
        let r = Rect::from_min_size(pos2(rect.min.x + 5.0 + k as f32 * 2.0, cy - 5.0 + k as f32), vec2(6.0, 7.0));
        ui.painter().rect(r, 1.0, c, Stroke::new(0.8, v.extreme_bg_color), StrokeKind::Middle);
    }
    ui.painter().galley(pos2(rect.min.x + 17.0, cy - g.size().y / 2.0), g, v.text_color());
}

fn volume_tag_text(m: &crate::volumes::Member, archive: Option<&str>) -> String {
    let n = format!("{}/{}", m.index + 1, m.count);
    match archive {
        Some(a) => format!("{a} · {n}"),
        None => n,
    }
}

/// "2/4" pill on a volume; on the highlighted set it names the archive too.
#[allow(clippy::too_many_arguments)]
fn volume_tag(ui: &Ui, m: &crate::volumes::Member, archive: Option<&str>, at: Pos2, align: Align2, font: FontId, on: bool, c: Color32) {
    let v = ui.visuals();
    let fg = if m.gap { v.error_fg_color } else if on { v.strong_text_color() } else { v.weak_text_color() };
    let g = ui.painter().layout_no_wrap(volume_tag_text(m, archive), font, fg);
    let size = g.size() + vec2(if on { 22.0 } else { 10.0 }, 4.0);
    let rect = align.anchor_size(at, size);
    let (fill, stroke) = if on { (v.window_fill.lerp_to_gamma(c, 0.35), Stroke::new(1.0, c)) } else { (v.window_fill, Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color)) };
    ui.painter().rect(rect, rect.height() / 2.0, fill, stroke, StrokeKind::Inside);
    let mut x = rect.min.x + 5.0;
    if on {
        // A little stack of volumes.
        let cy = rect.center().y;
        for k in 0..3 {
            let r = Rect::from_min_size(pos2(x + k as f32 * 2.0, cy - 4.0 + k as f32 * 1.0 - 1.0), vec2(6.0, 7.0));
            ui.painter().rect(r, 1.0, c, Stroke::new(0.8, v.extreme_bg_color), StrokeKind::Middle);
        }
        x += 12.0;
    }
    ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, fg);
}

/// A small padlock on archives that need a password.
fn lock_badge(ui: &Ui, icon: Rect, e: &crate::vfs::Entry, path: Option<&std::path::Path>) {
    let (Some(path), Some(mtime)) = (path, e.mtime) else { return };
    if e.is_dir || !crate::vfs::is_archive_name(&e.name) || crate::locks::locked(path, mtime) != Some(true) {
        return;
    }
    let s = (icon.width() * 0.5).clamp(11.0, 26.0);
    let r = Rect::from_min_size(icon.max - Vec2::splat(s * 0.85), Vec2::splat(s));
    let p = ui.painter();
    let v = ui.visuals();
    p.circle_filled(r.center(), s * 0.55, v.extreme_bg_color);
    // Shackle over a body.
    let body = Rect::from_center_size(r.center() + vec2(0.0, s * 0.12), vec2(s * 0.62, s * 0.46));
    let gold = Color32::from_rgb(0xf0, 0xc0, 0x40);
    let arc = Rect::from_center_size(pos2(body.center().x, body.min.y), vec2(s * 0.38, s * 0.5));
    let pts: Vec<Pos2> = (0..=12).map(|i| {
        let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
        pos2(arc.center().x + arc.width() / 2.0 * a.cos(), arc.center().y + arc.height() / 2.0 * a.sin())
    }).collect();
    p.add(egui::Shape::line(pts, Stroke::new((s * 0.11).max(1.2), gold)));
    p.rect_filled(body, s * 0.08, gold);
    p.circle_filled(body.center(), s * 0.06, v.extreme_bg_color);
}

/// Theme icon, or a thumbnail fitted into the square keeping its aspect.
fn paint_icon(ui: &Ui, rect: Rect, icon: Option<String>, thumb: Option<(String, f32)>, alpha: f32) {
    let tint = Color32::WHITE.gamma_multiply(alpha);
    if let Some((uri, aspect)) = thumb {
        let size = if aspect >= 1.0 {
            vec2(rect.width(), rect.width() / aspect)
        } else {
            vec2(rect.height() * aspect, rect.height())
        };
        let r = Rect::from_center_size(rect.center(), size);
        egui::Image::new(uri).corner_radius(3).tint(tint).paint_at(ui, r);
    } else if let Some(u) = icon {
        egui::Image::new(u).tint(tint).paint_at(ui, rect);
    }
}

fn paint_entry_bg(ui: &Ui, rect: Rect, cursor: bool, focused: bool, selected: bool, hovered: bool) {
    let v = ui.visuals();
    let accent = v.selection.bg_fill;
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 6.0, tint(&v, 0.45));
    } else if hovered {
        p.rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill.gamma_multiply(0.6));
    }
    if cursor {
        if focused {
            p.rect_filled(rect, 6.0, tint(&v, if selected { 0.35 } else { 0.55 }));
            p.rect_stroke(rect, 6.0, Stroke::new(2.0, accent), StrokeKind::Inside);
        } else {
            p.rect_stroke(rect, 6.0, Stroke::new(1.0, accent.gamma_multiply(0.7)), StrokeKind::Inside);
        }
    }
}

pub(crate) fn paint_row_bg(ui: &Ui, rect: Rect, cursor: bool, current: bool, hovered: bool) {
    let v = ui.visuals();
    let accent = v.selection.bg_fill;
    if cursor {
        ui.painter().rect_filled(rect, 6.0, tint(&v, 0.55));
        ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, accent), StrokeKind::Inside);
    } else if current {
        ui.painter().rect_filled(rect, 6.0, tint(&v, 0.25));
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

/// One line of text cut with "…" to `width`.
pub fn elided_text(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    layout(ui, text, font, color, width, 1, false)
}

fn elided(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32, rows: usize) -> Arc<Galley> {
    layout(ui, text, font, color, width, rows, false)
}

fn elided_centered(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32, rows: usize) -> Arc<Galley> {
    layout(ui, text, font, color, width, rows, true)
}

// ---------------------------------------------------------------- overlays

/// A menu and its open submenus side by side. Returns a click as
/// (depth, item index).
fn draw_menu_cascade(ui: &mut Ui, root: &mut Menu) -> Option<(usize, usize)> {
    let mut click = None;
    ui.horizontal_top(|ui| {
        let mut depth = 0;
        let mut m = Some(root);
        while let Some(menu) = m {
            let focused = menu.sub.is_none();
            let height = ui
                .vertical(|ui| {
                    if let Some(i) = draw_menu(ui, menu, focused) {
                        click = Some((depth, i));
                    }
                })
                .response
                .rect
                .height();
            if menu.sub.is_some() {
                let (line, _) = ui.allocate_exact_size(vec2(13.0, height), Sense::hover());
                ui.painter().vline(line.center().x, line.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
            }
            depth += 1;
            m = menu.sub.as_deref_mut();
        }
    });
    click
}

/// One menu column. `focused` is false while a submenu to its right has
/// the cursor, which dims this column's highlight.
fn draw_menu(ui: &mut Ui, m: &mut Menu, focused: bool) -> Option<usize> {
    let mut click = None;
    ui.set_min_width(300.0);
    ui.label(egui::RichText::new(&m.title).strong().size(16.0));
    ui.add_space(6.0);
    ui.spacing_mut().item_spacing.y = 2.0;
    // Line labels up when some items have checkboxes and others don't.
    let has_checks = m.items.iter().any(|i| i.checked.is_some());
    for (i, it) in m.items.iter().enumerate() {
        let (rect, r) = ui.allocate_exact_size(vec2(300.0, 34.0), if it.enabled { Sense::click() } else { Sense::hover() });
        if r.hovered() && it.enabled && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
            m.cursor = i;
        }
        let cur = i == m.cursor;
        if cur {
            let a = ui.visuals().selection.bg_fill;
            if focused {
                ui.painter().rect_filled(rect, 6.0, a.gamma_multiply(0.55));
                ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, a), StrokeKind::Inside);
            } else {
                ui.painter().rect_filled(rect, 6.0, a.gamma_multiply(0.3));
            }
        }
        let color = if it.enabled { ui.visuals().text_color() } else { ui.visuals().weak_text_color().gamma_multiply(0.6) };
        let mut x = rect.min.x + 12.0;
        if let Some(on) = it.checked {
            let b = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(14.0));
            ui.painter().rect_stroke(b, 3.0, Stroke::new(1.5, color), StrokeKind::Inside);
            if on {
                ui.painter().rect_filled(b.shrink(3.5), 2.0, ui.visuals().selection.bg_fill.gamma_multiply(1.6));
            }
        }
        if has_checks {
            x += 24.0;
        }
        ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, &it.label, FontId::proportional(15.0), color);
        if matches!(it.cmd, Cmd::Submenu(_)) {
            ui.painter().text(pos2(rect.max.x - 12.0, rect.center().y), Align2::RIGHT_CENTER, "›", FontId::proportional(18.0), color);
        }
        if r.clicked() && it.enabled {
            click = Some(i);
        }
    }
    click
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
    let shown = |cs: &[char]| -> String { if k.secret { "•".repeat(cs.len()) } else { cs.iter().collect() } };
    let text = shown(&k.text);
    let before = shown(&k.text[..k.caret]);
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

/// Sample icons for the theme preview: (caption, candidates).
const THEME_SAMPLES: [(&str, &[&str]); 16] = [
    ("Folder", &["folder", "inode-directory"]),
    ("Home", &["user-home", "folder-home"]),
    ("Downloads", &["folder-download", "folder-downloads"]),
    ("Documents", &["folder-documents"]),
    ("Pictures", &["folder-pictures"]),
    ("Music", &["folder-music"]),
    ("Videos", &["folder-videos"]),
    ("Trash", &["user-trash"]),
    ("Text", &["text-x-generic"]),
    ("Image", &["image-png", "image-x-generic"]),
    ("Audio", &["audio-x-generic", "audio-mpeg"]),
    ("Video", &["video-x-generic", "video-mp4"]),
    ("Archive", &["application-x-compressed-tar", "package-x-generic"]),
    ("PDF", &["application-pdf"]),
    ("Program", &["application-x-ms-dos-executable", "application-x-executable"]),
    ("Disk", &["drive-harddisk"]),
];

fn draw_themes(ui: &mut Ui, t: &mut crate::app::ThemePicker, icons: &mut Icons) -> Option<Cmd> {
    let mut run = None;
    ui.label(egui::RichText::new(tr("Icon theme")).strong().size(18.0));
    ui.add_space(8.0);
    ui.horizontal_top(|ui| {
        // Theme list.
        ui.vertical(|ui| {
            ui.set_width(250.0);
            let scroll_to = t.take_scroll();
            egui::ScrollArea::vertical().id_salt("themes").max_height(380.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (i, (id, label)) in t.items.iter().enumerate() {
                    let (rect, r) = ui.allocate_exact_size(vec2(240.0, 34.0), Sense::click());
                    if r.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                        t.cursor = i;
                    }
                    let cur = i == t.cursor;
                    let v = ui.visuals();
                    if cur {
                        ui.painter().rect_filled(rect, 6.0, tint(&v, 0.55));
                        ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, v.selection.bg_fill), StrokeKind::Inside);
                    }
                    // Each row shows its own folder icon.
                    let ir = Rect::from_min_size(rect.min + vec2(8.0, 5.0), Vec2::splat(24.0));
                    if let Some(u) = icons.named_in(&t.preview[i], &["folder", "inode-directory"], SMALL_ICON_LOOKUP) {
                        egui::Image::new(u).paint_at(ui, ir);
                    }
                    let g = elided(ui, label, FontId::proportional(15.0), v.text_color(), rect.width() - 48.0, 1);
                    ui.painter().galley(pos2(rect.min.x + 40.0, rect.center().y - g.size().y / 2.0), g, Color32::WHITE);
                    if scroll_to == Some(i) {
                        r.scroll_to_me(None);
                    }
                    if r.clicked() {
                        run = Some(Cmd::SetIconTheme(id.clone()));
                    }
                }
            });
        });
        // A plain ui.separator() here would stretch to the window's height
        // from the last frame, and the window would grow every frame.
        let (line, _) = ui.allocate_exact_size(vec2(9.0, 380.0), Sense::hover());
        ui.painter().vline(line.center().x, line.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
        // Preview of the highlighted theme.
        ui.vertical(|ui| {
            let theme = t.preview[t.cursor].clone();
            ui.label(egui::RichText::new(&t.items[t.cursor].1).weak());
            ui.add_space(4.0);
            egui::Grid::new("theme-preview").spacing([6.0, 6.0]).show(ui, |ui| {
                for (i, (caption, names)) in THEME_SAMPLES.iter().enumerate() {
                    let (rect, _) = ui.allocate_exact_size(vec2(78.0, 78.0), Sense::hover());
                    let ir = Rect::from_center_size(pos2(rect.center().x, rect.min.y + 28.0), Vec2::splat(48.0));
                    if let Some(u) = icons.named_in(&theme, names, 64) {
                        egui::Image::new(u).paint_at(ui, ir);
                    }
                    let c = ui.visuals().weak_text_color();
                    let g = elided_centered(ui, tr(caption), FontId::proportional(11.0), c, 76.0, 1);
                    ui.painter().galley(pos2(rect.center().x, ir.max.y + 4.0), g, c);
                    if i % 4 == 3 {
                        ui.end_row();
                    }
                }
            });
        });
    });
    ui.add_space(6.0);
    run
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
    let rows: [(Action, &str, &str); 13] = [
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
        (Action::HistoryBack, tr("History"), "Alt+← →"),
        (Action::Home, tr("Rename"), "F2"),
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
                } else if a == Action::HistoryBack {
                    ui.label(egui::RichText::new("—").weak()); // keyboard / toolbar only
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
    ui.add_space(8.0);
    ui.button(tr("Close")).clicked()
}

// ---------------------------------------------------------------- glyphs

/// Draw a gamepad button glyph (font independent).
fn shoulder_label(a: Action, pad: PadKind) -> Option<&'static str> {
    let ps = pad == PadKind::PlayStation;
    let pick = |l: &'static str, x: &'static str| if ps { l } else { x };
    Some(match a {
        Action::PrevTab => pick("L1", "LB"),
        Action::NextTab => pick("R1", "RB"),
        Action::LeftTrigger => pick("L2", "LT"),
        Action::RightTrigger => pick("R2", "RT"),
        Action::LeftStick => pick("L3", "LS"),
        _ => return None,
    })
}

/// Shoulder/trigger button glyph filling `rect`; the label scales with it.
fn shoulder_glyph(painter: &egui::Painter, rect: Rect, a: Action, pad: PadKind) {
    let Some(l) = shoulder_label(a, pad) else { return };
    let radius = (rect.height() * 0.28).round() as u8;
    painter.rect_filled(rect, CornerRadius::same(radius), Color32::from_gray(60));
    painter.text(rect.center(), Align2::CENTER_CENTER, l, FontId::proportional(rect.height() * 0.6), Color32::from_gray(230));
}

pub fn glyph(ui: &mut Ui, a: Action, pad: PadKind) {
    let ps = pad == PadKind::PlayStation;
    if shoulder_label(a, pad).is_some() {
        let (rect, _) = ui.allocate_exact_size(vec2(28.0, 18.0), Sense::hover());
        shoulder_glyph(ui.painter(), rect, a, pad);
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

// ---------------------------------------------------------------- top bar

enum TabAct {
    Switch(usize),
    Close(usize),
    New,
}

enum TopAction {
    Sidebar,
    Back,
    Forward,
    Up,
    Search,
    Dual,
    Menu,
    /// Click on an address field's background: make that pane active.
    Focus(usize),
    Go(usize, crate::vfs::Location),
    ClearFilter(usize),
}

/// Theme size to ask for an icon drawn `pt` points wide, rounded up to a
/// size themes ship natively. Odd sizes (48, 56…) make lookups fall back to
/// scaled-up small icons (Breeze's `16@3x`), which are monochrome glyphs.
pub(crate) fn lookup_px(ui: &Ui, pt: f32) -> u16 {
    let px = (pt * ui.pixels_per_point()).ceil() as u16;
    [SMALL_ICON_LOOKUP, 64, 128, 256].into_iter().find(|s| *s >= px).unwrap_or(256)
}

/// Square toolbar button with a themed icon (text fallback).
fn tool_button(ui: &mut Ui, rect: Rect, icons: &mut Icons, names: &[&str], fallback: &str, tip: &str, on: bool) -> egui::Response {
    let id = ui.id().with(("tool", names.first().copied().unwrap_or(fallback)));
    let r = ui.interact(rect, id, Sense::click());
    let v = ui.visuals();
    let accent = v.selection.bg_fill;
    if on {
        ui.painter().rect(rect, 8.0, tint(&v, 0.4), Stroke::new(1.0, accent), StrokeKind::Inside);
    } else if r.hovered() {
        ui.painter().rect_filled(rect, 8.0, v.widgets.hovered.weak_bg_fill);
    }
    let color = v.text_color();
    let px = ((TOOL_ICON * ui.pixels_per_point()).ceil() as u16).max(22);
    match icons.ui_icon(names, px, color) {
        Some(u) => egui::Image::new(u).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(TOOL_ICON))),
        None => {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, fallback, FontId::proportional(18.0), color);
        }
    }
    r.on_hover_text(tip)
}

/// One address field: breadcrumbs (trimmed from the left when long), a
/// read-only lock and the filter chip.
fn address_field(ui: &mut Ui, rect: Rect, pane: &Pane, highlight: bool, idx: usize, file: Option<&str>) -> Option<TopAction> {
    let mut act = None;
    let bg = ui.interact(rect, ui.id().with(("addr", idx)), Sense::click());
    let v = ui.visuals().clone();
    let stroke = if highlight { Stroke::new(1.5, v.selection.bg_fill) } else { v.widgets.noninteractive.bg_stroke };
    ui.painter().rect(rect, 8.0, v.extreme_bg_color, stroke, StrokeKind::Inside);
    if bg.clicked() {
        act = Some(TopAction::Focus(idx));
    }
    let inner = rect.shrink2(vec2(6.0, 4.0));
    let painter = ui.painter().with_clip_rect(rect.shrink(1.0));
    let mut right = inner.max.x;

    if let Some(f) = &pane.filter {
        let font = FontId::proportional(14.0);
        let g = painter.layout_no_wrap(format!("{}: {f}  ✕", tr("Filter")), font, v.strong_text_color());
        let chip = Rect::from_min_max(pos2(right - g.size().x - 16.0, inner.min.y + 2.0), pos2(right, inner.max.y - 2.0));
        let r = ui.interact(chip, ui.id().with(("filter-chip", idx)), Sense::click());
        painter.rect_filled(chip, 6.0, tint(&v, if r.hovered() { 0.8 } else { 0.5 }));
        painter.galley(pos2(chip.min.x + 8.0, chip.center().y - g.size().y / 2.0), g, Color32::WHITE);
        if r.clicked() {
            act = Some(TopAction::ClearFilter(idx));
        }
        right = chip.min.x - 6.0;
    }
    if pane.loc.is_archive() && pane.loc.dest().is_none() {
        let g = painter.layout_no_wrap("🔒".into(), FontId::proportional(15.0), v.weak_text_color());
        let p = pos2(right - g.size().x, inner.center().y - g.size().y / 2.0);
        let r = ui.interact(Rect::from_min_size(p, g.size()), ui.id().with(("lock", idx)), Sense::hover());
        r.on_hover_text(tr("This archive format is read-only"));
        right -= g.size().x + 6.0;
        painter.galley(p, g, v.weak_text_color());
    }

    // Breadcrumbs: keep the last ones that fit.
    let mut crumbs = pane.loc.crumbs();
    // The viewed file ends the path.
    if let Some(f) = file {
        crumbs.push((f.to_string(), pane.loc.clone()));
    }
    let n = crumbs.len();
    let font = FontId::proportional(16.0);
    let galleys: Vec<_> = crumbs
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            let c = if i + 1 == n { v.strong_text_color() } else { v.text_color() };
            painter.layout_no_wrap(label.clone(), font.clone(), c)
        })
        .collect();
    let sep = 18.0;
    let pad = 14.0;
    let avail = right - inner.min.x;
    let mut first = 0;
    let width = |from: usize| -> f32 {
        let w: f32 = galleys[from..].iter().map(|g| g.size().x + pad).sum::<f32>() + sep * (n - from - 1) as f32;
        if from > 0 { w + 24.0 + sep } else { w }
    };
    while first + 1 < n && width(first) > avail {
        first += 1;
    }
    let mut x = inner.min.x;
    if first > 0 {
        painter.text(pos2(x + 8.0, inner.center().y), Align2::LEFT_CENTER, "…", font.clone(), v.weak_text_color());
        x += 24.0;
        painter.text(pos2(x + sep / 2.0, inner.center().y), Align2::CENTER_CENTER, "›", font.clone(), v.weak_text_color());
        x += sep;
    }
    for (i, g) in galleys.into_iter().enumerate().skip(first) {
        let w = g.size().x + pad;
        let r = Rect::from_min_size(pos2(x, inner.min.y), vec2(w, inner.height()));
        let resp = ui.interact(r, ui.id().with(("crumb", idx, i)), Sense::click());
        if resp.hovered() {
            painter.rect_filled(r, 6.0, v.widgets.hovered.weak_bg_fill);
        }
        painter.galley(pos2(r.min.x + pad / 2.0, r.center().y - g.size().y / 2.0), g, v.text_color());
        if resp.clicked() {
            act = Some(TopAction::Go(idx, crumbs[i].1.clone()));
        }
        x += w;
        if i + 1 < n {
            painter.text(pos2(x + sep / 2.0, inner.center().y), Align2::CENTER_CENTER, "›", font.clone(), v.weak_text_color());
            x += sep;
        }
    }
    act
}

// ---------------------------------------------------------------- settings

pub enum SettingsEvent {
    Row(usize),
    Step(usize, f32),
    Pick(usize),
    Theme,
    Close,
    Chip(usize),
    Toggle(usize),
    /// Light/dark choice clicked.
    Appearance(u8),
    EditorRow(usize),
    EditorSet(usize, f32),
    Save,
    CancelEditor,
}

impl App {
    fn settings_event(&mut self, ev: SettingsEvent) {
        use crate::app::row;
        let Some(Overlay::Settings(st)) = self.overlay.as_mut() else { return };
        match ev {
            SettingsEvent::Row(r) => st.row = r,
            SettingsEvent::Step(r, d) => {
                st.row = r;
                self.settings_step(r, d);
            }
            SettingsEvent::Pick(i) => self.settings_pick(i),
            SettingsEvent::Theme => {
                st.row = row::THEME;
                self.run(Cmd::IconThemes);
            }
            SettingsEvent::Close => self.overlay = None,
            SettingsEvent::Chip(i) => self.settings_toggle_open(i),
            SettingsEvent::Appearance(i) => {
                st.row = row::APPEARANCE;
                self.cfg.appearance = i;
                self.cfg.save();
                self.apply_appearance();
            }
            SettingsEvent::Toggle(r) => {
                st.row = r;
                self.settings_step(r, 0.0);
            }
            SettingsEvent::EditorRow(r) => {
                if let Some(ed) = st.editor.as_mut() {
                    ed.row = r;
                }
            }
            SettingsEvent::EditorSet(r, v) => {
                if let Some(ed) = st.editor.as_mut() {
                    ed.row = r;
                    ed.hsv[r] = v.clamp(0.0, 1.0);
                }
            }
            SettingsEvent::Save => self.settings_save_color(),
            SettingsEvent::CancelEditor => st.editor = None,
        }
    }
}

const SETTINGS_W: f32 = 560.0;

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Row background; the focused row gets the highlight.
fn settings_row(ui: &mut Ui, h: f32, focused: bool, id: impl std::hash::Hash) -> (Rect, egui::Response) {
    let (rect, r) = ui.allocate_exact_size(vec2(SETTINGS_W, h), Sense::click());
    let _ = id;
    // Up/Down in long settings scroll the focused row into view.
    if focused && ui.ctx().data_mut(|d| d.remove_temp::<bool>(egui::Id::new("settings-reveal"))).unwrap_or(false) {
        ui.scroll_to_rect(rect.expand(8.0), None);
    }
    let v = ui.visuals();
    if focused {
        ui.painter().rect(rect, 8.0, tint(&v, 0.25), Stroke::new(1.5, v.selection.bg_fill), StrokeKind::Inside);
    } else if r.hovered() {
        ui.painter().rect_filled(rect, 8.0, v.widgets.hovered.weak_bg_fill.gamma_multiply(0.5));
    }
    (rect, r)
}

fn draw_settings(ui: &mut Ui, st: &mut crate::app::Settings, cfg: &Config, icons: &mut Icons) -> Option<SettingsEvent> {
    use crate::app::row;
    if let Some(ed) = &st.editor {
        return draw_color_editor(ui, ed);
    }
    let mut ev = None;
    ui.label(egui::RichText::new(tr("Settings")).strong().size(18.0));
    ui.add_space(8.0);
    ui.spacing_mut().item_spacing.y = 4.0;
    let v = ui.visuals().clone();
    let label_font = FontId::proportional(15.0);

    // Highlight colour: swatches, wrapped.
    let sw = crate::app::swatches(cfg);
    let per_line = 14;
    let lines = (sw.len() + 1).div_ceil(per_line);
    // Label sits ~10pt below the top; the last swatch line (with its cursor
    // ring) ends 19pt below its centre. Same 10pt below it.
    let (rect, r) = settings_row(ui, 52.0 + 19.0 + 10.0 + (lines - 1) as f32 * 34.0, st.row == row::ACCENT, "accent");
    if r.clicked() {
        ev = Some(SettingsEvent::Row(row::ACCENT));
    }
    ui.painter().text(rect.min + vec2(14.0, 18.0), Align2::LEFT_CENTER, tr("Highlight color"), label_font.clone(), v.text_color());
    for i in 0..=sw.len() {
        let (line, col) = (i / per_line, i % per_line);
        let c = pos2(rect.min.x + 30.0 + col as f32 * 36.0, rect.min.y + 52.0 + line as f32 * 34.0);
        let hit = Rect::from_center_size(c, Vec2::splat(32.0));
        let resp = ui.interact(hit, ui.id().with(("swatch", i)), Sense::click());
        let cursor = st.row == row::ACCENT && st.swatch == i;
        if let Some(sc) = sw.get(i) {
            let fill = rgb(sc.unwrap_or(crate::config::DEFAULT_ACCENT));
            ui.painter().circle_filled(c, 13.0, fill);
            if *sc == cfg.accent {
                ui.painter().circle_stroke(c, 13.0, Stroke::new(2.0, Color32::WHITE));
                ui.painter().circle_filled(c, 3.5, Color32::WHITE);
            }
            let r = resp.on_hover_text(match sc {
                None => tr("Default").to_string(),
                Some([r, g, b]) => format!("#{r:02x}{g:02x}{b:02x}"),
            });
            if r.clicked() {
                ev = Some(SettingsEvent::Pick(i));
            }
        } else {
            ui.painter().circle_stroke(c, 12.0, Stroke::new(1.5, v.text_color()));
            ui.painter().text(c, Align2::CENTER_CENTER, "+", FontId::proportional(18.0), v.text_color());
            if resp.on_hover_text(tr("New color")).clicked() {
                ev = Some(SettingsEvent::Pick(i));
            }
        }
        if cursor {
            ui.painter().circle_stroke(c, 17.0, Stroke::new(2.0, v.strong_text_color()));
        }
    }

    // Light or dark: three segments.
    let (rect, r) = settings_row(ui, 44.0, st.row == row::APPEARANCE, "appearance");
    if r.clicked() {
        ev = Some(SettingsEvent::Row(row::APPEARANCE));
    }
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Appearance"), label_font.clone(), v.text_color());
    let names = [tr("System"), tr("Dark"), tr("Light")];
    let seg_w = 96.0;
    let x0 = rect.max.x - 10.0 - seg_w * 3.0;
    let whole = Rect::from_min_size(pos2(x0, rect.center().y - 15.0), vec2(seg_w * 3.0, 30.0));
    ui.painter().rect_filled(whole, 8.0, v.extreme_bg_color);
    for (i, name) in names.iter().enumerate() {
        let seg = Rect::from_min_size(pos2(x0 + i as f32 * seg_w, whole.min.y), vec2(seg_w, 30.0)).shrink(2.0);
        let resp = ui.interact(seg, ui.id().with(("appearance", i)), Sense::click());
        let on = cfg.appearance as usize == i;
        if on {
            ui.painter().rect_filled(seg, 6.0, v.selection.bg_fill);
        } else if resp.hovered() {
            ui.painter().rect_filled(seg, 6.0, v.widgets.hovered.weak_bg_fill);
        }
        let c = if on { v.selection.stroke.color } else { v.text_color() };
        ui.painter().text(seg.center(), Align2::CENTER_CENTER, *name, FontId::proportional(14.0), c);
        if resp.clicked() {
            ev = Some(SettingsEvent::Appearance(i as u8));
        }
    }

    // Language: ‹ name ›.
    let (rect, r) = settings_row(ui, 44.0, st.row == row::LANGUAGE, "language");
    if r.clicked() {
        ev = Some(SettingsEvent::Row(row::LANGUAGE));
    }
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Language"), label_font.clone(), v.text_color());
    let name = match cfg.language.as_deref().and_then(|c| crate::i18n::LANGUAGES.iter().find(|(l, _, _)| *l == c)) {
        Some((_, own, _)) => own.to_string(),
        None => format!("{} ({})", tr("As the system"), crate::i18n::LANGUAGES[crate::i18n::detect()].1),
    };
    let (minus, plus) = (
        Rect::from_center_size(pos2(rect.max.x - 300.0, rect.center().y), Vec2::splat(28.0)),
        Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(28.0)),
    );
    for (b, label, d) in [(minus, "‹", -1.0), (plus, "›", 1.0)] {
        let resp = ui.interact(b, ui.id().with(("lang", label)), Sense::click());
        ui.painter().rect_filled(b, 6.0, if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { v.widgets.inactive.weak_bg_fill });
        ui.painter().text(b.center(), Align2::CENTER_CENTER, label, FontId::proportional(18.0), v.text_color());
        if resp.clicked() {
            ev = Some(SettingsEvent::Step(row::LANGUAGE, d));
        }
    }
    ui.painter().text(pos2((minus.max.x + plus.min.x) / 2.0, rect.center().y), Align2::CENTER_CENTER, name, label_font.clone(), v.strong_text_color());

    // Steppers.
    let steppers: [(usize, &str, String); 4] = [
        (row::SIDE_ICON, tr("Sidebar icons"), format!("{} px", cfg.side_icon)),
        (row::LIST_ICON, tr("List icons"), format!("{} px", cfg.list_icon)),
        (row::GRID_ICON, tr("Grid icons"), format!("{} px", cfg.grid_icon)),
        (row::ZOOM, tr("Interface scale"), format!("{:.0}%", cfg.zoom * 100.0)),
    ];
    for (r_i, label, value) in steppers {
        let (rect, r) = settings_row(ui, 44.0, st.row == r_i, r_i);
        if r.clicked() {
            ev = Some(SettingsEvent::Row(r_i));
        }
        ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, label, label_font.clone(), v.text_color());
        let plus = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(30.0));
        let minus = Rect::from_center_size(pos2(rect.max.x - 136.0, rect.center().y), Vec2::splat(30.0));
        for (b, t, d) in [(minus, "−", -1.0), (plus, "+", 1.0)] {
            let br = ui.interact(b, ui.id().with(("step", r_i, t)), Sense::click());
            ui.painter().rect(b, 6.0, if br.hovered() { v.widgets.hovered.bg_fill } else { v.widgets.inactive.bg_fill }, Stroke::NONE, StrokeKind::Inside);
            ui.painter().text(b.center(), Align2::CENTER_CENTER, t, FontId::proportional(18.0), v.text_color());
            if br.clicked() {
                ev = Some(SettingsEvent::Step(r_i, d));
            }
        }
        ui.painter().text(pos2((plus.min.x + minus.max.x) / 2.0, rect.center().y), Align2::CENTER_CENTER, value, label_font.clone(), v.strong_text_color());
    }

    // Icon theme.
    let (rect, r) = settings_row(ui, 44.0, st.row == row::THEME, "theme");
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Icon theme…"), label_font.clone(), v.text_color());
    let ir = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(24.0));
    if let Some(u) = icons.named(&["folder"], lookup_px(ui, 24.0)) {
        egui::Image::new(u).paint_at(ui, ir);
    }
    ui.painter().text(pos2(ir.min.x - 10.0, rect.center().y), Align2::RIGHT_CENTER, cfg.icon_theme.as_deref().unwrap_or(tr("System")), label_font.clone(), v.weak_text_color());
    if r.clicked() {
        ev = Some(SettingsEvent::Theme);
    }

    // Open in Polka: a chip per kind.
    let (rect, r) = settings_row(ui, 86.0, st.row == row::OPEN_IN, "open-in");
    if r.clicked() {
        ev = Some(SettingsEvent::Row(row::OPEN_IN));
    }
    ui.painter().text(rect.min + vec2(14.0, 18.0), Align2::LEFT_CENTER, tr("Open in Polka"), label_font.clone(), v.text_color());
    let kinds = [tr("Text"), tr("Pictures"), tr("Music"), tr("Video")];
    let flags = [cfg.open_text, cfg.open_images, cfg.open_music, cfg.open_videos];
    let chip_w = (SETTINGS_W - 28.0 - 3.0 * 8.0) / 4.0;
    for (i, label) in kinds.iter().enumerate() {
        let chip = Rect::from_min_size(pos2(rect.min.x + 14.0 + i as f32 * (chip_w + 8.0), rect.min.y + 38.0), vec2(chip_w, 36.0));
        let resp = ui.interact(chip, ui.id().with(("open-chip", i)), Sense::click());
        let on = flags[i];
        let cursor = st.row == row::OPEN_IN && st.chip == i;
        let fill = if on { tint(&v, 0.45) } else if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { v.widgets.inactive.weak_bg_fill };
        let stroke = if cursor { Stroke::new(2.0, v.strong_text_color()) } else { Stroke::NONE };
        ui.painter().rect(chip, 8.0, fill, stroke, StrokeKind::Inside);
        let color = v.text_color();
        let text = if on { format!("✔ {label}") } else { label.to_string() };
        ui.painter().text(chip.center(), Align2::CENTER_CENTER, text, FontId::proportional(14.0), color);
        if resp.clicked() {
            ev = Some(SettingsEvent::Chip(i));
        }
    }

    // Folding multi-volume archives.
    let (rect, r) = settings_row(ui, 44.0, st.row == row::VOLUMES, "volumes");
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Fold multi-volume archives"), label_font.clone(), v.text_color());
    let b = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(20.0));
    ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
    if cfg.collapse_volumes {
        ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), v.strong_text_color());
    }
    if r.clicked() {
        ev = Some(SettingsEvent::Toggle(row::VOLUMES));
    }

    // Pictures from the internet in Markdown.
    let (rect, r) = settings_row(ui, 44.0, st.row == row::WEB_IMAGES, "web-images");
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Load Markdown pictures from the internet"), label_font.clone(), v.text_color());
    let b = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(20.0));
    ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
    if cfg.viewer.web_images {
        ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), v.strong_text_color());
    }
    if r.clicked() {
        ev = Some(SettingsEvent::Toggle(row::WEB_IMAGES));
    }

    // Close.
    let (rect, r) = settings_row(ui, 40.0, st.row == row::CLOSE, "close");
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, tr("Close"), label_font, v.text_color());
    if r.clicked() {
        ev = Some(SettingsEvent::Close);
    }
    ev
}

fn draw_color_editor(ui: &mut Ui, ed: &crate::app::ColorEditor) -> Option<SettingsEvent> {
    let mut ev = None;
    let v = ui.visuals().clone();
    ui.label(egui::RichText::new(tr("New color")).strong().size(18.0));
    ui.add_space(8.0);
    let [r, g, b] = ed.rgb();
    let (prev, _) = ui.allocate_exact_size(vec2(SETTINGS_W, 56.0), Sense::hover());
    ui.painter().rect_filled(Rect::from_min_size(prev.min, vec2(120.0, 48.0)), 8.0, Color32::from_rgb(r, g, b));
    ui.painter().text(
        pos2(prev.min.x + 136.0, prev.min.y + 24.0),
        Align2::LEFT_CENTER,
        format!("#{r:02x}{g:02x}{b:02x}"),
        FontId::monospace(16.0),
        v.strong_text_color(),
    );
    let [h, s, val] = ed.hsv;
    let hsv = |h: f32, s: f32, v: f32| -> Color32 { egui::ecolor::Hsva::new(h, s, v, 1.0).into() };
    let bars: [(&str, Box<dyn Fn(f32) -> Color32>); 3] = [
        (tr("Hue"), Box::new(move |t| hsv(t, 1.0, 1.0))),
        (tr("Saturation"), Box::new(move |t| hsv(h, t, val.max(0.2)))),
        (tr("Brightness"), Box::new(move |t| hsv(h, s, t))),
    ];
    for (i, (label, color_at)) in bars.iter().enumerate() {
        let (rect, rr) = settings_row(ui, 54.0, ed.row == i, ("bar", i));
        if rr.clicked() {
            ev = Some(SettingsEvent::EditorRow(i));
        }
        ui.painter().text(rect.min + vec2(14.0, 14.0), Align2::LEFT_CENTER, *label, FontId::proportional(14.0), v.text_color());
        let bar = Rect::from_min_max(pos2(rect.min.x + 14.0, rect.min.y + 28.0), pos2(rect.max.x - 14.0, rect.max.y - 8.0));
        let steps = 48;
        for k in 0..steps {
            let t0 = k as f32 / steps as f32;
            let seg = Rect::from_min_max(pos2(bar.min.x + bar.width() * t0, bar.min.y), pos2(bar.min.x + bar.width() * (k + 1) as f32 / steps as f32 + 0.5, bar.max.y));
            ui.painter().rect_filled(seg, 0.0, color_at(t0));
        }
        let x = bar.min.x + bar.width() * ed.hsv[i];
        ui.painter().rect_stroke(Rect::from_center_size(pos2(x, bar.center().y), vec2(6.0, bar.height() + 6.0)), 2.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Middle);
        let drag = ui.interact(bar.expand(4.0), ui.id().with(("bar-drag", i)), Sense::click_and_drag());
        if drag.dragged() || drag.clicked() {
            if let Some(p) = drag.interact_pointer_pos() {
                ev = Some(SettingsEvent::EditorSet(i, (p.x - bar.min.x) / bar.width()));
            }
        }
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        for (i, label) in [(3, tr("Save")), (4, tr("Cancel"))] {
            let (rect, r) = ui.allocate_exact_size(vec2(SETTINGS_W / 2.0 - 4.0, 40.0), Sense::click());
            let focused = ed.row == i;
            let fill = if focused { tint(&v, 0.5) } else if r.hovered() { v.widgets.hovered.weak_bg_fill } else { v.widgets.inactive.weak_bg_fill };
            ui.painter().rect(rect, 8.0, fill, if focused { Stroke::new(1.5, v.selection.bg_fill) } else { Stroke::NONE }, StrokeKind::Inside);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(15.0), v.text_color());
            if r.clicked() {
                ev = Some(if i == 3 { SettingsEvent::Save } else { SettingsEvent::CancelEditor });
            }
        }
    });
    ev
}

// ---------------------------------------------------------------- icon picker

fn draw_icon_picker(ui: &mut Ui, p: &mut crate::app::IconPicker, icons: &mut Icons, pad: PadKind) -> Option<Cmd> {
    let mut run = None;
    let v = ui.visuals().clone();
    let cell = 72.0;
    let width = cell * p.cols as f32;
    ui.label(egui::RichText::new(tr("Folder icon")).strong().size(18.0));
    ui.add_space(6.0);

    // Colour groups (L1/R1). Each tab shows its group's first icon.
    if p.groups.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(width);
            let (r, _) = ui.allocate_exact_size(vec2(36.0, 26.0), Sense::hover());
            shoulder_glyph(ui.painter(), r, Action::PrevTab, pad);
            for (gi, g) in p.groups.iter().enumerate() {
                let label = g.color.clone().unwrap_or_else(|| tr("Standard").to_string());
                let on = gi == p.group;
                let font = FontId::proportional(13.0);
                let gal = ui.painter().layout_no_wrap(label, font, v.text_color());
                let (rect, resp) = ui.allocate_exact_size(vec2(gal.size().x + 36.0, 26.0), Sense::click());
                let (fill, stroke) = if on {
                    (tint(&v, 0.4), Stroke::new(1.5, v.selection.bg_fill))
                } else if resp.hovered() {
                    (v.widgets.hovered.weak_bg_fill, Stroke::NONE)
                } else {
                    (Color32::TRANSPARENT, v.widgets.noninteractive.bg_stroke)
                };
                ui.painter().rect(rect, 6.0, fill, stroke, StrokeKind::Inside);
                let ir = Rect::from_min_size(rect.min + vec2(5.0, 4.0), Vec2::splat(18.0));
                if let Some(u) = g.names.first().and_then(|n| icons.named(&[n.as_str()], SMALL_ICON_LOOKUP)) {
                    egui::Image::new(u).paint_at(ui, ir);
                }
                ui.painter().galley(pos2(ir.max.x + 6.0, rect.center().y - gal.size().y / 2.0), gal, v.text_color());
                if resp.clicked() {
                    p.group = gi;
                    p.cursor = 0;
                }
            }
            let (r, _) = ui.allocate_exact_size(vec2(36.0, 26.0), Sense::hover());
            shoulder_glyph(ui.painter(), r, Action::NextTab, pad);
        });
        ui.add_space(6.0);
    }

    let scroll_to = p.take_scroll();
    let names = p.names().to_vec();
    let rows = names.len().div_ceil(p.cols);
    // show_rows only lays out visible rows, so keep the cursor in view by
    // setting the offset ourselves (like the file panes do).
    let view_h = cell * 5.0;
    let mut sa = egui::ScrollArea::vertical().id_salt(("icon-picker", p.group)).max_height(view_h);
    if let Some(i) = scroll_to {
        let top = (i / p.cols) as f32 * cell;
        let mut off = p.scroll_y;
        if top < off {
            off = top;
        } else if top + cell > off + view_h {
            off = top + cell - view_h;
        }
        sa = sa.vertical_scroll_offset(off);
    }
    let out = sa.show_rows(ui, cell, rows, |ui, range| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        for row in range {
            let (row_rect, _) = ui.allocate_exact_size(vec2(width, cell), Sense::hover());
            for col in 0..p.cols {
                let i = row * p.cols + col;
                let Some(name) = names.get(i) else { break };
                let rect = Rect::from_min_size(row_rect.min + vec2(col as f32 * cell, 0.0), Vec2::splat(cell)).shrink(3.0);
                let r = ui.interact(rect, ui.id().with(("icon", i)), Sense::click());
                if i == p.cursor {
                    ui.painter().rect(rect, 8.0, tint(&v, 0.4), Stroke::new(2.0, v.selection.bg_fill), StrokeKind::Inside);
                } else if r.hovered() {
                    ui.painter().rect_filled(rect, 8.0, v.widgets.hovered.weak_bg_fill);
                }
                if let Some(u) = icons.named(&[name.as_str()], 64) {
                    egui::Image::new(u).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(44.0)));
                }
                if r.clicked() {
                    run = Some(Cmd::SetPinIcon(p.path.clone(), Some(name.clone())));
                }
                r.on_hover_text(name);
            }
        }
    });
    p.scroll_y = out.state.offset.y;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let name = names.get(p.cursor).cloned().unwrap_or_default();
        ui.label(egui::RichText::new(name).monospace().weak());
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            // Right to left: each button, then its gamepad glyph before it.
            if ui.button(tr("Reset")).clicked() {
                run = Some(Cmd::SetPinIcon(p.path.clone(), None));
            }
            glyph(ui, Action::Select, pad);
            ui.add_space(8.0);
            if ui.button(tr("Own picture…")).clicked() {
                run = Some(Cmd::PickIconFile(p.path.clone()));
            }
            glyph(ui, Action::Actions, pad);
        });
    });
    run
}

// ---------------------------------------------------------------- archive dialog

/// Returns (row, 0) for a click on a row, (row, ±1) for its arrows.
fn draw_compress(ui: &mut Ui, d: &crate::app::CompressDlg, pad: PadKind) -> Option<(usize, isize)> {
    use crate::app::crow;
    use crate::ops::ArchiveFormat as F;
    let mut ev = None;
    let v = ui.visuals().clone();
    let font = FontId::proportional(15.0);
    ui.label(egui::RichText::new(tr("New archive")).strong().size(18.0));
    ui.add_space(8.0);
    ui.spacing_mut().item_spacing.y = 4.0;
    let o = &d.opts;
    let method = match o.format {
        F::TarZst => "Zstandard".to_string(),
        F::TarGz => "Deflate (gzip)".to_string(),
        F::TarXz => "LZMA2 (xz)".to_string(),
        _ => o.method.label().to_string(),
    };
    let password = match &o.password {
        _ if !o.format.encrypts() => tr("zip and 7z only").to_string(),
        None => tr("None").to_string(),
        Some(p) => {
            let dots = "•".repeat(p.chars().count().min(12));
            if o.format == F::Zip { format!("{dots}  AES-256") } else { dots }
        }
    };
    let rows: [(usize, &str, String, bool); 7] = [
        (crow::NAME, tr("Name"), format!("{}{}", d.name, o.format.ext()), false),
        (crow::FORMAT, tr("Format"), o.format.ext().trim_start_matches('.').to_string(), true),
        (crow::METHOD, tr("Method"), method, true),
        (crow::LEVEL, tr("Compression"), o.level.label().to_string(), true),
        (crow::SPLIT, tr("Split into volumes"), crate::app::split_label(o.split), true),
        (crow::PASSWORD, tr("Password"), password, false),
        (crow::NAMES, tr("Encrypt file names"), String::new(), false),
    ];
    for (r_i, label, value, arrows) in rows {
        let on = d.enabled(r_i);
        let (rect, r) = settings_row(ui, 44.0, d.row == r_i, ("compress", r_i));
        if on && r.clicked() {
            ev = Some((r_i, 0));
        }
        let (text, strong) = if on { (v.text_color(), v.strong_text_color()) } else { (v.weak_text_color(), v.weak_text_color()) };
        ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, label, font.clone(), text);
        if r_i == crow::NAMES {
            let b = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(20.0));
            ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, strong), StrokeKind::Inside);
            if o.encrypt_names && on {
                ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), strong);
            }
            continue;
        }
        if arrows && on {
            let right = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(30.0));
            let left = Rect::from_center_size(pos2(rect.max.x - 216.0, rect.center().y), Vec2::splat(30.0));
            for (b, t, step) in [(left, "◀", -1), (right, "▶", 1)] {
                let br = ui.interact(b, ui.id().with(("cstep", r_i, step)), Sense::click());
                ui.painter().rect(b, 6.0, if br.hovered() { v.widgets.hovered.bg_fill } else { v.widgets.inactive.bg_fill }, Stroke::NONE, StrokeKind::Inside);
                ui.painter().text(b.center(), Align2::CENTER_CENTER, t, FontId::proportional(13.0), v.text_color());
                if br.clicked() {
                    ev = Some((r_i, step));
                }
            }
            ui.painter().text(pos2((left.max.x + right.min.x) / 2.0, rect.center().y), Align2::CENTER_CENTER, value, font.clone(), strong);
        } else {
            ui.painter().text(pos2(rect.max.x - 14.0, rect.center().y), Align2::RIGHT_CENTER, value, font.clone(), if on { strong } else { text });
        }
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (i, label, g) in [(crow::CREATE, tr("Create"), Action::Menu), (crow::CANCEL, tr("Cancel"), Action::Back)] {
            let (rect, r) = ui.allocate_exact_size(vec2(SETTINGS_W / 2.0 - 4.0, 40.0), Sense::click());
            let focused = d.row == i;
            let fill = if focused { tint(&v, 0.5) } else if r.hovered() { v.widgets.hovered.weak_bg_fill } else { v.widgets.inactive.weak_bg_fill };
            ui.painter().rect(rect, 8.0, fill, if focused { Stroke::new(1.5, v.selection.bg_fill) } else { Stroke::NONE }, StrokeKind::Inside);
            let gal = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(15.0), v.text_color());
            let w = gal.size().x + 26.0;
            let x0 = rect.center().x - w / 2.0;
            let gr = Rect::from_min_size(pos2(x0, rect.center().y - 10.0), Vec2::splat(20.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(gr));
            glyph(&mut child, g, pad);
            ui.painter().galley(pos2(x0 + 26.0, rect.center().y - gal.size().y / 2.0), gal, v.text_color());
            if r.clicked() {
                ev = Some((i, 0));
            }
        }
    });
    ev
}
