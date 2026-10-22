//! Rendering and pointer/keyboard input collection.

use std::sync::Arc;

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Galley, Key, Order, Pos2, Rect, Sense, Shape,
    Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

use crate::app::{
    App, Cmd, Focus, Menu, Osk, OskKey, Overlay, Pane, Props, fmt_time, human_size,
};
use crate::config::{Config, SortKey, ViewMode};
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
    /// Pointer events of drags from and to other programs.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // A finger on the screen: gestures, not a mouse.
        raw.events = self.touch_feed(std::mem::take(&mut raw.events));
        if self.touch.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        raw.events.append(&mut self.dnd_inject);
        // X11 has the files only at the drop (and no position): the panel.
        if !raw.dropped_files.is_empty() && self.overlay.is_none() {
            let files: Vec<std::path::PathBuf> = raw.dropped_files.drain(..).map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect();
            self.drop_external(files);
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // At most ~120 frames a second when they come back to back
        // (animations, video): without vsync nothing else holds them.
        const FRAME: std::time::Duration = std::time::Duration::from_micros(8_333);
        if let Some(t) = self.last_frame {
            if let Some(rest) = FRAME.checked_sub(t.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        self.last_frame = Some(std::time::Instant::now());
        let ctx = ui.ctx().clone();
        self.touch.grips.clear();
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
        self.draw_term_window(&ctx);
        self.draw_drag(&ctx);
        self.spring_frame_end();
        if !full {
            self.draw_jobs(&ctx);
        }
        self.draw_overlay(&ctx);
        self.draw_touch_hold(&ctx);
        if !self.cfg.system_titlebar && !full {
            crate::titlebar::resize_edges(&ctx);
        }

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
        if !self.jobs.is_empty() || self.toast.is_some() || self.tabs.iter().any(|t| t.pane.loading.is_some()) {
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
        // The terminal's own keyboard: real keys go to the shell too.
        if matches!(&self.overlay, Some(Overlay::Osk(k)) if k.live()) {
            let _ = self.terminal_keyboard(events);
            return;
        }
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
        if self.overlay.is_none() && (self.term_window.is_some() || (self.focus == Focus::Panes && self.term_shown())) {
            events = self.terminal_keyboard(events);
        } else if self.esc_down {
            // Esc held left the terminal: until it's let go it does nothing
            // more (its repeats would bring the focus straight back).
            let mut up = false;
            events.retain(|e| match e {
                egui::Event::Key { key: Key::Escape, pressed, .. } => {
                    up |= !pressed;
                    false
                }
                _ => true,
            });
            if up {
                self.esc_down = false;
            }
        }
        for ev in events {
            match ev {
                egui::Event::Text(t) if osk => {
                    if let Some(Overlay::Osk(k)) = self.overlay.as_mut() {
                        k.insert(&t);
                    }
                }
                // Ctrl+V on a real keyboard.
                egui::Event::Paste(t) if osk => self.osk_paste(Some(t)),
                // Ctrl+C / X / V come as clipboard events, not keys: the
                // files' copy, cut and paste.
                egui::Event::Copy if self.overlay.is_none() => self.run(Cmd::Copy),
                egui::Event::Cut if self.overlay.is_none() => self.run(Cmd::Cut),
                egui::Event::Paste(_) if self.overlay.is_none() => self.run(Cmd::Paste),
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

    /// A command's terminal window over everything but the pad's keyboard
    /// (above which it ends, so the prompt stays in sight).
    fn draw_term_window(&mut self, ctx: &egui::Context) {
        if self.term_window.is_none() {
            return;
        }
        let screen = ctx.content_rect();
        let typing = matches!(&self.overlay, Some(Overlay::Osk(k)) if k.live());
        let bottom = if typing { self.osk_top.unwrap_or(screen.max.y) - 8.0 } else { screen.max.y - 40.0 };
        let w = (screen.width() * 0.86).min(1200.0);
        // Typing: up to the top, every line counts.
        let top = screen.min.y + if typing { 6.0 } else { 30.0 };
        let rect = Rect::from_min_max(pos2(screen.center().x - w / 2.0, top), pos2(screen.center().x + w / 2.0, bottom.max(top + 140.0)));
        let dark = ctx.global_style().visuals.dark_mode;
        let scheme = crate::terminal::scheme::resolve(&self.cfg.terminal.scheme, dark);
        let size = self.cfg.terminal.font_size;
        let title = self.term_window_title.clone();
        let mut act = None;
        egui::Area::new("term-window".into()).order(Order::Middle).fixed_pos(rect.min).show(ctx, |ui| {
            // The dimmed app behind, on the window's own layer (under it).
            ui.painter().clone().with_clip_rect(screen).rect_filled(screen, 0.0, Color32::from_black_alpha(120));
            egui::Frame::popup(ui.style()).inner_margin(12.0).corner_radius(10.0).show(ui, |ui| {
                ui.set_min_size(rect.size() - vec2(24.0, 24.0));
                ui.set_max_size(rect.size() - vec2(24.0, 24.0));
                // Typing on the pad's keyboard: the lines matter more.
                if !typing {
                    ui.label(egui::RichText::new(&title).strong().size(16.0));
                    ui.add_space(4.0);
                }
                if let Some(t) = self.term_window.as_deref_mut() {
                    act = crate::terminal::draw::draw(ui, t, &scheme, size, &mut self.icons, !typing, !typing);
                }
            });
        });
        if let Some(a) = act {
            self.terminal_act(a);
        }
    }

    /// A finger held still: a ring fills up under it till the long press
    /// (the menu, or picking things up).
    fn draw_touch_hold(&self, ctx: &egui::Context) {
        let Some((pos, t)) = self.touch.holding() else { return };
        if t < 0.2 {
            return;
        }
        let p = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, egui::Id::new("touch-hold")));
        let v = ctx.global_style().visuals.clone();
        let r = 30.0;
        p.circle_stroke(pos, r, Stroke::new(3.0, v.widgets.noninteractive.bg_stroke.color.gamma_multiply(0.8)));
        let k = (t - 0.2) / 0.8;
        let n = (48.0 * k).ceil() as usize + 1;
        let pts: Vec<Pos2> = (0..=n).map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * k * i as f32 / n as f32;
            pos + vec2(a.cos(), a.sin()) * r
        }).collect();
        p.add(egui::Shape::line(pts, Stroke::new(4.0, v.selection.bg_fill)));
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
                Key::T if m.shift => Some(Cmd::ReopenTab),
                Key::T => Some(Cmd::NewTab),
                Key::R => return self.reload_all(),
                Key::D => self.pane().loc.local_path().map(|p| Cmd::Pin(p.to_path_buf())),
                Key::Num1 => Some(Cmd::SetView(ViewMode::Grid)),
                Key::Num2 => Some(Cmd::SetView(ViewMode::List)),
                // Tabs: back and forth.
                Key::ArrowLeft => return press(self, Action::PrevTab),
                Key::ArrowRight => return press(self, Action::NextTab),
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
                Key::Plus | Key::Equals => return self.zoom_by(0.1),
                Key::Minus => return self.zoom_by(-0.1),
                Key::Num0 => return self.zoom_by(1.0 - self.zoom()),
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
                Key::Home if self.overlay.is_none() => {
                    let cfg = self.cfg.clone();
                    let home = dirs::home_dir().unwrap_or_else(|| "/".into());
                    self.focus = Focus::Panes;
                    self.pane_mut().navigate(crate::vfs::Location::Local(home), None, &cfg);
                }
                Key::Enter if self.overlay.is_none() => self.run(Cmd::Properties),
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
            Key::F4 => return if self.overlay.is_none() { self.run(Cmd::TermOpen(self.pane().loc.local_path().map(|p| p.to_path_buf()))) },
            Key::F5 => return self.reload_all(),
            Key::F9 => return if self.overlay.is_none() { self.run(Cmd::ToggleSidebar) },
            Key::Delete => {
                if self.overlay.is_none() {
                    self.run(if m.shift { Cmd::DeleteAsk } else { Cmd::TrashAsk });
                }
                return;
            }
            Key::Tab => {
                if self.cfg.dual && self.overlay.is_none() {
                    self.set_side(1 - self.side());
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

    fn zoom_by(&mut self, d: f32) {
        let z = self.zoom_mut();
        *z = (*z + d).clamp(0.6, 3.0);
        self.apply_zoom();
        self.cfg.save();
    }

    // ------------------------------------------------------------ top

    fn draw_top(&mut self, ui: &mut Ui) {
        let cfg = self.cfg.clone();
        let mut act: Option<TopAction> = None;
        let mut resting: Option<(usize, crate::vfs::Location)> = None;
        let dual = cfg.dual;
        let active = self.side();
        let own = !cfg.system_titlebar;
        let look = crate::titlebar::Look::of(&cfg);
        let win_w = if own { crate::titlebar::buttons_width(look.style) } else { 0.0 };
        let frame = egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 6));
        egui::Panel::top("top").frame(frame).show(ui, |ui| {
            let full = ui.available_rect_before_wrap();
            let row = Rect::from_min_size(full.min, vec2(full.width(), TOOL_BTN));
            ui.allocate_rect(row, Sense::hover());
            // Polka's own title bar: the bar between the buttons moves the
            // window, a double click fills the screen with it.
            if own {
                let bar = ui.interact(row.expand(6.0), ui.id().with("titlebar"), Sense::click_and_drag());
                if bar.drag_started_by(egui::PointerButton::Primary) {
                    act = Some(TopAction::MoveWindow);
                } else if bar.double_clicked() {
                    act = Some(TopAction::Maximize);
                }
            }
            let btn = |x: f32| Rect::from_min_size(pos2(x, row.min.y), Vec2::splat(TOOL_BTN));
            let step = TOOL_BTN + 4.0;

            // Left: sidebar toggle, then history / up.
            let side_names: &[&str] = if cfg.sidebar_collapsed {
                &["sidebar-expand-left", "view-left-new", "sidebar-show", "view-sidebar"]
            } else {
                &["sidebar-collapse-left", "view-left-close", "sidebar-show", "view-sidebar"]
            };
            // The window's buttons first when they're on the left.
            let mut x = row.min.x + if look.left { win_w } else { 0.0 };
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

            // Right, pinned: the window's buttons (with no title bar),
            // search, dual pane, menu.
            if own {
                let x0 = if look.left { row.min.x } else { row.max.x - win_w + 10.0 };
                if let Some(b) = crate::titlebar::buttons(ui, &mut self.icons, look, x0, row.center().y) {
                    act = Some(TopAction::Window(b));
                }
            }
            let mut rx = row.max.x - TOOL_BTN - if look.left { 0.0 } else { win_w };
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
                let even = self
                    .divider_x
                    .filter(|d| *d > addr_start + 120.0 && *d < addr_end - 120.0)
                    .unwrap_or((addr_start + addr_end) / 2.0);
                // Resting on one field a moment widens it to show the
                // whole path; the other keeps a strip to come back to.
                let id = ui.id().with("addr-mid");
                let now_mid = ui.ctx().animate_value_with_time(id, ui.ctx().data(|d| d.get_temp::<f32>(id.with("target"))).unwrap_or(even), 0.15);
                let over = ui.input(|i| i.pointer.hover_pos()).filter(|p| row.contains(*p) && p.x >= addr_start && p.x <= addr_end).map(|p| (p.x > now_mid) as usize);
                match (over, self.addr_hover) {
                    (Some(i), Some((h, _))) if i == h => {}
                    (Some(i), _) => self.addr_hover = Some((i, std::time::Instant::now())),
                    (None, _) => self.addr_hover = None,
                }
                // Dragging, it widens at once: the crumbs move once, before
                // the drag rests on one of them, not during the wait.
                let rest_for = if self.drag.is_some() { 0.0 } else { 0.35 };
                let wide = self.addr_hover.filter(|(_, t)| t.elapsed().as_secs_f32() >= rest_for).map(|(i, _)| i);
                if let Some((_, t)) = self.addr_hover.filter(|_| wide.is_none()) {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f32((rest_for - t.elapsed().as_secs_f32()).max(0.0)));
                }
                let strip = 140.0_f32.min((addr_end - addr_start) / 4.0);
                let target = match wide {
                    Some(0) => addr_end - strip,
                    Some(_) => addr_start + strip,
                    None => even,
                };
                ui.ctx().data_mut(|d| d.insert_temp(id.with("target"), target));
                let mid = now_mid;
                self.addr_mid = Some(mid);
                vec![
                    (0, Rect::from_min_max(pos2(addr_start, row.min.y), pos2(mid - 4.0, row.max.y))),
                    (1, Rect::from_min_max(pos2(mid + 4.0, row.min.y), pos2(addr_end, row.max.y))),
                ]
            } else {
                vec![(0, Rect::from_min_max(pos2(addr_start, row.min.y), pos2(addr_end, row.max.y)))]
            };
            let mut drawn = Vec::new();
            for (idx, rect) in fields {
                let pane = self.pane_at(idx);
                let highlight = dual && idx == active && viewing.is_none();
                let file = viewing.as_ref().map(|(_, n)| n.as_str());
                let (a, rest) = address_field(ui, rect, pane, highlight, idx, file, self.drag.is_some(), &self.spring, &mut drawn, own);
                if let Some(a) = a {
                    act = Some(a);
                }
                if rest.is_some() {
                    resting = rest;
                }
            }
            self.crumb_rects = drawn;
        });

        if let Some((side, loc)) = resting {
            self.spring_rest(crate::app::SpringAt::Crumb(side, loc));
        }
        let Some(act) = act else { return };
        match act {
            TopAction::MoveWindow => ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag),
            TopAction::Maximize => crate::titlebar::toggle_maximized(ui.ctx()),
            TopAction::Window(b) => crate::titlebar::press(ui.ctx(), b),
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
                self.set_side(i);
            }
            TopAction::Go(i, loc) => {
                self.focus = Focus::Panes;
                self.set_side(i);
                self.pane_mut().navigate(loc, None, &cfg);
            }
            TopAction::ClearFilter(i) => {
                self.set_side(i);
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
                let p = &t.pane;
                let usage = t.usage.as_ref().is_some_and(|u| u.shown);
                let term = t.term.as_ref().is_some_and(|u| u.shown);
                let title = t.viewer.as_ref().map_or_else(
                    || if term { tr("Terminal").to_string() } else if usage { tr("Disk usage").to_string() } else { p.loc.title() },
                    |v| v.name.clone(),
                );
                elided(ui, &title, FontId::proportional(14.0), v.text_color(), 180.0, 1)
            })
            .collect();
        let close_w = if many { 20.0 } else { 0.0 };
        // Dual mode: the tab each side shows wears its L or R.
        let side_of = |i: usize| if !dual { None } else if i == self.tab_of(0) { Some(0) } else if i == self.tab_of(1) { Some(1) } else { None };
        const BADGE: f32 = 18.0;
        let widths: Vec<f32> = titles.iter().enumerate().map(|(i, g)| (g.size().x + 28.0 + close_w + if side_of(i).is_some() { BADGE + 6.0 } else { 0.0 }).max(96.0)).collect();
        let total = shoulder_w * 2.0 + widths.iter().sum::<f32>() + TAB_H + gap * (widths.len() + 2) as f32;

        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), TAB_H), Sense::hover());
        let mut x = (row.center().x - total / 2.0).max(row.min.x);
        let at = |x: f32, w: f32| Rect::from_min_size(pos2(x, row.min.y), vec2(w, TAB_H));
        let painter = ui.painter().with_clip_rect(row.expand(2.0));
        let dragging = self.drag.is_some();
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let mut resting = None;
        let mut rects = Vec::new();

        shoulder_glyph(&painter, at(x, shoulder_w), Action::PrevTab, self.pad);
        x += shoulder_w + gap;
        for (i, (title, w)) in titles.into_iter().zip(widths).enumerate() {
            let rect = at(x, w);
            x += w + gap;
            rects.push(rect);
            let is_active = i == self.tab;
            let r = ui.interact(rect, ui.id().with(("tab", i)), Sense::click());
            let accent = v.selection.bg_fill;
            // Dragged items held over another tab switch to it in a moment.
            let under = dragging && !is_active && pointer.is_some_and(|p| rect.contains(p));
            if under {
                resting = Some(i);
            }
            let (fill, stroke) = if under {
                (v.widgets.hovered.weak_bg_fill, Stroke::new(2.0, accent))
            } else if is_active {
                (tint(&v, 0.35), Stroke::new(1.5, accent))
            } else if r.hovered() {
                (v.widgets.hovered.weak_bg_fill, v.widgets.hovered.bg_stroke)
            } else {
                (v.widgets.inactive.weak_bg_fill.gamma_multiply(0.6), v.widgets.noninteractive.bg_stroke)
            };
            painter.rect(rect, 7.0, fill, stroke, StrokeKind::Inside);
            let mut tx = rect.min.x + 14.0;
            if let Some(sd) = side_of(i) {
                let b = Rect::from_min_size(pos2(rect.min.x + 8.0, rect.center().y - BADGE / 2.0), Vec2::splat(BADGE));
                let mine = sd == self.side();
                painter.rect_filled(b, 4.0, if mine { accent } else { accent.gamma_multiply(0.35) });
                let fg = if mine { v.strong_text_color() } else { v.text_color() };
                painter.text(b.center(), Align2::CENTER_CENTER, if sd == 0 { "L" } else { "R" }, FontId::proportional(12.0), fg);
                tx = b.max.x + 6.0;
            }
            painter.galley(pos2(tx, rect.center().y - title.size().y / 2.0), title, v.text_color());
            if under && let Some(t) = spring_progress(&self.spring, &crate::app::SpringAt::Tab(i)) {
                spring_bar(ui, rect, t);
            }
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
        self.tab_rects = rects;
        if let Some(i) = resting {
            self.spring_rest(crate::app::SpringAt::Tab(i));
        }

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
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                let p = self.pane();
                let mut s = format!("{} {}", p.entries.len(), tr("items"));
                if !p.selected.is_empty() {
                    s += &format!(" · {} {}", p.selected.len(), tr("selected"));
                }
                ui.label(s);
                self.status_figures(ui);
                let p = self.pane();
                if let Some(m) = p.current().and_then(|e| p.volumes.get(&e.name)) {
                    ui.separator();
                    let t = format!("{} {} {} {} · {}", tr("Volume"), m.index + 1, tr("of"), m.count, m.archive);
                    ui.label(egui::RichText::new(t).color(volume_color(ui.visuals())));
                    if m.gap {
                        ui.label(egui::RichText::new(tr("some volumes are missing")).color(ui.visuals().error_fg_color));
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
    }

    /// Past the item count: the disk's space and what the folder holds, an
    /// icon and a number each (what each is, on hover).
    fn status_figures(&mut self, ui: &mut Ui) {
        use crate::status::{Counts, FIGURES, Figure};
        let shown = self.cfg.status;
        let p = self.pane();
        let n = p.entries.len();
        if !matches!(&self.status_counts, Some((l, k, _)) if *l == p.loc && *k == n) {
            self.status_counts = Some((p.loc.clone(), n, Counts::of(&p.entries)));
        }
        let counts = self.status_counts.as_ref().map(|c| c.2).unwrap_or_default();
        let dir = self.pane().loc.local_path().map(|d| d.to_path_buf());
        let space = match (shown.space, dir) {
            (true, Some(d)) => self.status_space.of(&d, ui.ctx()),
            _ => None,
        };
        let px = lookup_px(ui, 16.0);
        for f in FIGURES {
            if !shown.get(f) {
                continue;
            }
            let (text, tip, used) = match f {
                Figure::Space => {
                    let Some((total, used)) = space else { continue };
                    let free = total.saturating_sub(used);
                    let tip = format!("{}: {} · {}: {} · {}: {}", tr("Used"), human_size(used), tr("Free"), human_size(free), tr("Total"), human_size(total));
                    (format!("{} / {}", human_size(used), human_size(free)), tip, Some(used as f32 / total.max(1) as f32))
                }
                _ => match counts.get(f) {
                    0 => continue,
                    k => (k.to_string(), format!("{}: {k}", f.label()), None),
                },
            };
            ui.add_space(8.0);
            let icon = self.icons.named(f.icons(), px);
            let r = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                match icon {
                    Some(u) => {
                        ui.add(egui::Image::new(u).fit_to_exact_size(Vec2::splat(16.0)));
                    }
                    None => {
                        ui.label("•");
                    }
                }
                // The disk: how full, as a little bar.
                if let Some(t) = used {
                    let (bar, _) = ui.allocate_exact_size(vec2(30.0, 6.0), Sense::hover());
                    let v = ui.visuals();
                    let c = if t > 0.9 { v.error_fg_color } else { v.selection.bg_fill };
                    ui.painter().rect_filled(bar, 3.0, v.widgets.inactive.weak_bg_fill);
                    ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * t.clamp(0.0, 1.0), bar.height())), 3.0, c);
                }
                ui.add(egui::Label::new(text).selectable(false));
            });
            ui.interact(r.response.rect, ui.id().with(("status-figure", f as u8)), Sense::hover()).on_hover_text(tip);
        }
    }

    /// Running operations as cards over the bottom right corner: what goes
    /// where, the item now, how much, how fast, how long still.
    fn draw_jobs(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        // Quick ones (a rename, a move on one disk) don't flash a card.
        let shown: Vec<usize> = (0..self.jobs.len())
            .filter(|&i| {
                let j = &self.jobs[i];
                j.result.lock().map(|r| r.is_none()).unwrap_or(false) && j.started.elapsed() > std::time::Duration::from_millis(350)
            })
            .collect();
        if shown.is_empty() {
            return;
        }
        let mut cancel = None;
        egui::Area::new("jobs".into()).order(Order::Foreground).anchor(Align2::RIGHT_BOTTOM, vec2(-14.0, -40.0)).show(ctx, |ui| {
            for &i in &shown {
                let j = &self.jobs[i];
                egui::Frame::popup(ui.style()).inner_margin(12.0).corner_radius(10.0).show(ui, |ui| {
                    ui.set_width(380.0);
                    let v = ui.visuals().clone();
                    let (done, total) = (j.done.load(Ordering::Relaxed), j.total.load(Ordering::Relaxed));
                    let items = j.items.load(Ordering::Relaxed);
                    ui.horizontal(|ui| {
                        let title = if j.target.is_empty() { j.label.clone() } else { format!("{} → {}", j.label, j.target) };
                        ui.add(egui::Label::new(egui::RichText::new(title).strong()).truncate());
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("✖").on_hover_text(tr("Cancel")).clicked() {
                                cancel = Some(i);
                            }
                            if total > 0 {
                                ui.label(format!("{:.0}%", j.fraction() * 100.0));
                            }
                        });
                    });
                    ui.add_space(4.0);
                    let bar = egui::ProgressBar::new(j.fraction()).desired_height(8.0).fill(v.selection.bg_fill);
                    // Not knowing the total yet: an animated bar.
                    ui.add(if total == 0 { bar.animate(true) } else { bar });
                    ui.add_space(4.0);
                    let cur = j.current.lock().map(|c| c.clone()).unwrap_or_default();
                    if !cur.is_empty() {
                        ui.add(egui::Label::new(egui::RichText::new(cur).weak()).truncate());
                    }
                    // Amounts, speed and time left from the average so far.
                    let secs = j.started.elapsed().as_secs_f64().max(0.001);
                    let mut line = if items {
                        format!("{done} / {total}")
                    } else {
                        format!("{} / {}", human_size(done), human_size(total))
                    };
                    if !items && done > 0 && secs > 0.5 {
                        line += &format!(" · {}/{}", human_size((done as f64 / secs) as u64), tr("sec"));
                    }
                    if total > done && done > 0 && secs > 1.0 {
                        let left = ((total - done) as f64 * secs / done as f64) as u64;
                        line += &format!(" · {} {}", tr("left"), if left >= 3600 { format!("{}:{:02}:{:02}", left / 3600, left / 60 % 60, left % 60) } else { format!("{}:{:02}", left / 60, left % 60) });
                    }
                    // Nothing counted yet: no "0 B / 0 B".
                    if total > 0 {
                        ui.label(egui::RichText::new(line).size(12.5).weak());
                    }
                });
                ui.add_space(6.0);
            }
        });
        if let Some(i) = cancel {
            self.jobs[i].cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn hints(&self) -> Vec<(Action, &'static str)> {
        match &self.overlay {
            Some(Overlay::Menu(_)) => vec![(Action::Accept, tr("OK")), (Action::Back, tr("Back"))],
            Some(Overlay::Osk(k)) if k.live() => vec![
                (Action::Accept, tr("Press")),
                (Action::Select, tr("Backspace")),
                (Action::Actions, tr("Space")),
                (Action::LeftTrigger, tr("Shift")),
                (Action::View, tr("Layout")),
                (Action::PrevTab, "← →"),
                (Action::Back, tr("Close")),
            ],
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
            Some(Overlay::Settings(st)) => vec![
                (Action::Left, tr("Adjust")),
                (Action::Accept, tr("OK")),
                (Action::Actions, if st.row == crate::app::row::SCHEME { tr("Your own schemes (JSON)") } else { tr("Delete color") }),
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
            Some(Overlay::Help) => vec![(Action::Up, tr("Scroll")), (Action::Back, tr("Close"))],
            Some(Overlay::OpenWith(_)) => vec![
                (Action::Up, tr("Move")),
                (Action::Accept, tr("Open")),
                (Action::Select, tr("Remember")),
                (Action::Actions, tr("All programs")),
                (Action::Back, tr("Cancel")),
            ],
            Some(Overlay::Convert(_)) => vec![
                (Action::Up, tr("Move")),
                (Action::Left, tr("Adjust")),
                (Action::Menu, tr("Convert")),
                (Action::Back, tr("Cancel")),
            ],
            Some(Overlay::Net(_)) => vec![
                (Action::Up, tr("Move")),
                (Action::Left, tr("Adjust")),
                (Action::Accept, tr("OK")),
                (Action::Menu, tr("Connect")),
                (Action::Back, tr("Cancel")),
            ],
            Some(Overlay::Tags(ed)) => {
                let mut v = vec![(Action::Up, tr("Move")), (Action::Accept, tr("Mark"))];
                if ed.cursor < ed.tags.len() {
                    v.push((Action::Select, tr("Find")));
                }
                v.extend([(Action::Actions, tr("New tag…")), (Action::Back, tr("Done"))]);
                v
            }
            Some(Overlay::Themes(t)) if t.in_colors => vec![(Action::Up, tr("Color")), (Action::Accept, tr("OK")), (Action::Back, tr("Back"))],
            Some(Overlay::Themes(t)) => {
                let mut v = vec![(Action::Up, tr("Move"))];
                if !t.colors.is_empty() {
                    v.push((Action::Right, tr("Color")));
                }
                v.extend([(Action::Accept, tr("OK")), (Action::Back, tr("Cancel"))]);
                v
            }
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
            None if self.term_shown() && self.focus == Focus::Panes => self.terminal_hints(),
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
        let side_icon = self.cfg.side_icon;
        // Its edge pulled: as wide as it's pulled, folding or unfolding
        // past the middle; let go, it settles.
        let (thin, wide) = (side_icon + 32.0, SIDE_W + (side_icon - 20.0).max(0.0) * 3.0);
        let split = egui::Id::new("side-split");
        let pulled: Option<f32> = ui.ctx().data(|d| d.get_temp(split));
        let collapsed = pulled.map_or(self.cfg.sidebar_collapsed, |w| w < (thin + wide) / 2.0);
        let width = match pulled {
            Some(w) => ui.ctx().animate_value_with_time(split, w.clamp(thin, wide), 0.0),
            None => ui.ctx().animate_value_with_time(split, if collapsed { thin } else { wide }, 0.15),
        };
        let focused = self.focus == Focus::Sidebar && self.overlay.is_none();
        let cur_loc = self.pane().loc.clone();
        let usage_here = self.usage_shown();
        let term_here = self.term_shown();
        let dragging = self.drag.is_some();
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let released = ui.input(|i| i.pointer.primary_released());
        let mut dropped = None;
        let mut clicked = None;
        let mut actions = None;
        let mut resting = None;
        let mut rows = Vec::new();
        let side_font = side_font(side_icon);
        let panel = egui::Panel::left("sidebar")
            .resizable(false)
            .exact_size(width)
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
                        rows.push(rect);
                        let is_cursor = focused && i == self.side_cursor;
                        let here = if it.usage {
                            usage_here
                        } else if it.term {
                            term_here
                        } else {
                            it.loc == cur_loc && !usage_here && !term_here
                        };
                        paint_row_bg(ui, rect, is_cursor, here, r.hovered());
                        if dragging && !it.usage && !it.term && !it.offline() && pointer.is_some_and(|p| rect.contains(p)) {
                            ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, ui.visuals().selection.bg_fill), StrokeKind::Inside);
                            if released {
                                dropped = Some(i);
                            } else {
                                // Resting here opens it (a line fills up meanwhile).
                                resting = Some(i);
                                if let Some(t) = spring_progress(&self.spring, &crate::app::SpringAt::Side(i)) {
                                    spring_bar(ui, rect, t);
                                }
                            }
                        }
                        let under = dragging && !it.usage && !it.term && pointer.is_some_and(|p| rect.contains(p));
                        let grow = 1.0 + 0.15 * ui.ctx().animate_bool_with_time(ui.id().with(("side-grow", i)), under, 0.12);
                        let icon_rect = Rect::from_center_size(pos2(rect.min.x + 8.0 + side_icon / 2.0, rect.center().y), Vec2::splat(side_icon * grow));
                        let px = lookup_px(ui, side_icon);
                        // Custom icons are theme names or image files we stored.
                        let custom = it.custom_icon.as_deref().and_then(|c| {
                            let p = std::path::Path::new(c);
                            if p.is_absolute() { p.exists().then(|| crate::icons::uri(p)) } else { self.icons.named(&[c], px) }
                        });
                        // A network place not connected looks faded; one
                        // connecting spins.
                        let state = it.net.as_ref().or(it.phone.as_ref());
                        let faded = state.is_some_and(|n| !n.1);
                        if let Some(u) = custom.or_else(|| self.icons.named(it.icon, px)) {
                            egui::Image::new(u).tint(if faded { Color32::from_white_alpha(110) } else { Color32::WHITE }).paint_at(ui, icon_rect);
                        }
                        if let Some((id, online)) = state {
                            if self.net_busy.contains(id) {
                                egui::Spinner::new().size(side_icon * 0.6).paint_at(ui, Rect::from_center_size(icon_rect.right_bottom(), Vec2::splat(side_icon * 0.6)));
                            } else if *online {
                                ui.painter().circle(icon_rect.right_bottom() - vec2(2.0, 2.0), 4.0, Color32::from_rgb(80, 200, 120), Stroke::new(1.5, ui.visuals().panel_fill));
                            }
                        }
                        if !collapsed {
                            let tx = side_icon + 16.0;
                            let text_c = if faded { ui.visuals().weak_text_color() } else { ui.visuals().text_color() };
                            let g = elided(ui, &it.label, FontId::proportional(side_font), text_c, rect.width() - tx - 8.0, 1);
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
        self.side_rects = rows;
        if self.overlay.is_none() {
            self.side_grip(ui.ctx(), panel.response.rect, (thin, wide));
        }
        if let Some(i) = dropped {
            self.drop_on_side(i);
        } else if let Some(i) = resting {
            self.spring_rest(crate::app::SpringAt::Side(i));
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

    /// The sidebar's edge: pulled, the sidebar folds or unfolds (as far as
    /// it's pulled meanwhile); clicked, it flips.
    fn side_grip(&mut self, ctx: &egui::Context, panel: Rect, (thin, wide): (f32, f32)) {
        let split = egui::Id::new("side-split");
        let grip = Rect::from_min_max(pos2(panel.max.x - 4.0, panel.min.y), pos2(panel.max.x + 4.0, panel.max.y));
        self.touch.grips.push(grip);
        let mut flip = false;
        egui::Area::new(egui::Id::new("side-grip")).order(Order::Foreground).movable(false).fixed_pos(grip.min).show(ctx, |ui| {
            let (_, r) = ui.allocate_exact_size(grip.size(), Sense::click_and_drag());
            if r.hovered() || r.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                let x = panel.max.x;
                ui.painter().line_segment([pos2(x, grip.min.y), pos2(x, grip.max.y)], Stroke::new(3.0, ui.visuals().selection.bg_fill));
            }
            if r.dragged() {
                if let Some(p) = ctx.pointer_latest_pos() {
                    ctx.data_mut(|d| d.insert_temp(split, p.x - panel.min.x));
                }
            }
            if r.drag_stopped() {
                if let Some(w) = ctx.data_mut(|d| d.remove_temp::<f32>(split)) {
                    flip = (w < (thin + wide) / 2.0) != self.cfg.sidebar_collapsed;
                }
            } else if r.clicked() {
                flip = true;
            }
        });
        if flip {
            self.run(Cmd::ToggleSidebar);
        }
    }

    // ------------------------------------------------------------ panes

    fn draw_center(&mut self, ui: &mut Ui) {
        let dual = self.cfg.dual;
        let tab = self.tab;
        let focused = self.focus == Focus::Panes && self.overlay.is_none();
        let active = self.side();
        // Each side shows its own tab.
        let tab_of = [self.tab_of(0), self.tab_of(1)];
        let mut events: Vec<(usize, PaneEvent)> = Vec::new();
        let mut divider = None;
        let mut tab_act = None;
        let mut viewer_act = None;
        let mut usage_act = None;
        let mut term_act = None;
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
            if let Some(t) = self.tabs[tab].term.as_deref_mut().filter(|t| t.shown) {
                let scheme = crate::terminal::scheme::resolve(&self.cfg.terminal.scheme, ui.visuals().dark_mode);
                // The pad's keyboard below: the terminal ends above it, so the
                // prompt stays in sight.
                let above = match &self.overlay {
                    Some(Overlay::Osk(k)) if k.live() => self.osk_top,
                    _ => None,
                };
                let typing = above.is_some();
                if let Some(y) = above {
                    let r = ui.available_rect_before_wrap();
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(r.min, pos2(r.max.x, (y - 8.0).max(r.min.y + 120.0)))));
                    term_act = crate::terminal::draw::draw(&mut child, t, &scheme, self.cfg.terminal.font_size, &mut self.icons, focused || typing, !typing);
                } else {
                    term_act = crate::terminal::draw::draw(ui, t, &scheme, self.cfg.terminal.font_size, &mut self.icons, focused, true);
                }
                return;
            }
            let dragging = self.drag.is_some();
            let tap = self.touch.tapped.then_some(self.touch.double);
            let finger = self.touch.active;
            let App { tabs, icons, cfg, drag, spring, .. } = self;
            let lifted = |pane: usize| drag.as_ref().filter(|d| d.from == (tab_of[pane], pane)).map(|d| &d.names);
            // The entry a drag rests on in this side, and how long it has.
            let spring = &*spring;
            let springing = |pane: usize, shown: &crate::vfs::Location| match spring {
                Some((at @ crate::app::SpringAt::Entry(s, l, n), _, _)) if *s == pane && l == shown => spring_progress(spring, at).map(|t| (n.as_str(), t)),
                _ => None,
            };
            if dual {
                let (a, b) = crate::app::two_panes(tabs, tab_of[0], tab_of[1]);
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
                    let shown = pane.loc.clone();
                    for e in draw_pane(&mut inner, pane, icons, cfg, is_active && focused, (tab_of[idx], idx), dragging, lifted(idx), springing(idx, &shown), tap, finger) {
                        events.push((idx, e));
                    }
                }
            } else {
                let shown = tabs[tab].pane.loc.clone();
                for e in draw_pane(ui, &mut tabs[tab].pane, icons, cfg, focused, (tab_of[0], 0), dragging, lifted(0), springing(0, &shown), tap, finger) {
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
        if let Some(a) = term_act {
            self.terminal_act(a);
        }
        let usage = self.tabs[tab].usage.as_ref().is_some_and(|u| u.shown) || self.tabs[tab].term.as_ref().is_some_and(|u| u.shown);
        let divider = if self.tabs[tab].viewer.is_some() || usage { self.divider_x } else { divider };
        if divider != self.divider_x {
            // The address bar lines up with this on the next frame.
            self.divider_x = divider;
            ui.ctx().request_repaint();
        }
        for (idx, e) in events {
            // Only resting there: the drag goes on, the sides stay as they are.
            if let PaneEvent::Rest(i) = e {
                let p = self.pane_at(idx);
                if let Some(name) = p.entries.get(i).map(|e| e.name.clone()) {
                    self.spring_rest(crate::app::SpringAt::Entry(idx, p.loc.clone(), name));
                }
                continue;
            }
            self.focus = Focus::Panes;
            self.set_side(idx);
            match e {
                PaneEvent::Toggle(i) => {
                    let p = self.pane_mut();
                    if let Some(name) = p.entries.get(i).map(|e| e.name.clone()) {
                        // A folded volume set goes in or out whole.
                        let members = p.collapsed.get(&name).map(|c| c.2.clone()).unwrap_or_default();
                        if p.selected.remove(&name) {
                            for m in &members {
                                p.selected.remove(m);
                            }
                        } else {
                            p.selected.insert(name);
                            p.selected.extend(members);
                        }
                    }
                }
                PaneEvent::SortBy(key) => {
                    if self.cfg.sort == key {
                        self.run(Cmd::ToggleSortReverse);
                    } else {
                        self.cfg.sort_reverse = false;
                        self.run(Cmd::Sort(key));
                    }
                }
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
                            let on = !p.selected.contains(&n);
                            p.mark(&n, on);
                        }
                    }
                }
                PaneEvent::Activate(i) => {
                    self.pane_mut().cursor = i;
                    self.activate();
                }
                PaneEvent::DoubleTap(i) => {
                    let p = self.pane_mut();
                    p.cursor = i;
                    p.cursor_hidden = false;
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
                PaneEvent::Rest(_) => {}
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
            Some(TabAct::Switch(i)) => self.pick_tab(i),
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
        let live = matches!(&self.overlay, Some(Overlay::Osk(k)) if k.live());
        let side = live || matches!(self.overlay, Some(Overlay::ViewerSettings(_) | Overlay::TermSettings(_) | Overlay::TermKeys(_)));
        let bottom = live || matches!(self.overlay, Some(Overlay::TermKeys(_)));
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
        let mut tag_ev = None;
        let mut net_ev = None;
        let mut conv_ev = None;
        let mut ow_ev = None;
        let mut viewer_ev: Option<(usize, isize)> = None;
        let mut term_set_ev: Option<(usize, isize)> = None;
        let mut qkey_ev: Option<(usize, bool)> = None;
        let dark = ctx.global_style().visuals.dark_mode;
        let (overlay, icons, cfg, schemes) = (&mut self.overlay, &mut self.icons, &self.cfg, &self.schemes);
        let (anchor, offset) = if bottom {
            // Just above the hints bar.
            (Align2::CENTER_BOTTOM, vec2(0.0, -36.0))
        } else if side {
            (Align2::RIGHT_TOP, vec2(-16.0, 110.0))
        } else {
            (Align2::CENTER_CENTER, vec2(0.0, 0.0))
        };
        let vprefs = self.cfg.viewer.clone();
        // One area per kind of overlay, and per menu: an area keeps its last
        // size, and a small one's would squeeze the scroll area of a bigger
        // one after it (two items of the app menu after a sidebar menu).
        let kind = overlay.as_ref().map(std::mem::discriminant);
        let menu = match overlay.as_ref() {
            Some(Overlay::Menu(m)) => Some((m.title.clone(), m.items.len())),
            _ => None,
        };
        let shown = egui::Area::new(egui::Id::new(("overlay", kind, menu)))
            .order(Order::Foreground)
            .anchor(anchor, offset)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(if live { 8.0 } else { 14.0 }).corner_radius(10.0).show(ui, |ui| match overlay.as_mut() {
                    Some(Overlay::Menu(m)) => menu_click = draw_menu_cascade(ui, m),
                    Some(Overlay::Osk(k)) => osk_key = draw_osk(ui, k, cfg.password_hidden, osk_scale(screen, k.live())),
                    Some(Overlay::Props(p)) => dismiss = draw_props(ui, p),
                    Some(Overlay::Help) => dismiss = draw_help(ui, pad, self.help_scroll.take()),
                    Some(Overlay::Themes(t)) => run = draw_themes(ui, t, icons),
                    Some(Overlay::Tags(ed)) => tag_ev = crate::tags_ui::draw(ui, ed),
                    Some(Overlay::Net(d)) => net_ev = crate::net_ui::draw(ui, d),
                    Some(Overlay::Convert(d)) => conv_ev = crate::convert_ui::draw(ui, d, icons),
                    Some(Overlay::OpenWith(d)) => ow_ev = crate::openwith::draw(ui, d, icons),
                    Some(Overlay::Settings(st)) => {
                        // Taller than small windows: scroll.
                        let max_h = (screen.height() - 120.0).max(240.0);
                        settings_ev = egui::ScrollArea::vertical().id_salt("settings-scroll").max_height(max_h).show(ui, |ui| draw_settings(ui, st, cfg, icons, schemes)).inner;
                    }
                    Some(Overlay::IconPicker(p)) => run = draw_icon_picker(ui, p, icons, pad),
                    Some(Overlay::Compress(d)) => compress_ev = draw_compress(ui, d, pad),
                    Some(Overlay::ViewerSettings(st)) => viewer_ev = crate::viewer::draw::draw_settings(ui, st, &vprefs),
                    Some(Overlay::TermKeys(q)) => qkey_ev = crate::terminal::draw::draw_keys(ui, q, pad),
                    Some(Overlay::TermSettings(st)) => term_set_ev = crate::terminal::draw::draw_settings(ui, st, &cfg.terminal, vprefs.font.as_deref(), dark),
                    None => {}
                });
            });
        self.osk_top = live.then_some(shown.response.rect.min.y);
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
        if let Some(ev) = tag_ev {
            self.tags_event(ev);
        }
        if let Some((row, step)) = net_ev {
            crate::net_ui::clicked(self, row, step);
        }
        if let Some((row, step)) = conv_ev {
            crate::convert_ui::clicked(self, row, step);
        }
        if let Some(ev) = ow_ev {
            crate::openwith::event(self, ev);
        }
        if let Some((row, d)) = viewer_ev {
            self.viewer_step(row, d);
        }
        if let Some((row, d)) = term_set_ev {
            self.term_settings_step(row, d);
        }
        if let Some((i, close)) = qkey_ev {
            self.quick_key(i, close);
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
    /// Its check box: in or out of the selection, the rest as it is.
    Toggle(usize),
    /// A column header: sort by it (again: the other way).
    SortBy(SortKey),
    /// Middle click: a folder in a new tab.
    NewTab(usize),
    Activate(usize),
    /// A finger's double tap: opens it.
    DoubleTap(usize),
    Context(Option<usize>),
    /// Items picked up by the mouse (from this entry).
    DragStart(usize),
    /// Dragged items let go over this pane (onto an entry, or not).
    Drop(Option<usize>),
    /// Dragged items held over a folder or archive: it opens after a moment.
    Rest(usize),
    /// A click on empty space.
    ClearSelection,
}

#[allow(clippy::too_many_arguments)]
fn draw_pane(ui: &mut Ui, pane: &mut Pane, icons: &mut Icons, cfg: &Config, focused: bool, id: (usize, usize), dragging: bool, lifted: Option<&std::collections::HashSet<String>>, springing: Option<(&str, f32)>, tap: Option<bool>, finger: bool) -> Vec<PaneEvent> {
    let mut events = Vec::new();
    // Dragged items over this pane: dropped on release.
    let whole = ui.max_rect();
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let over = dragging && pointer.is_some_and(|p| whole.contains(p));
    let released = ui.input(|i| i.pointer.primary_released());
    let mut drop_entry = None;
    // Nothing to list (loading, an error, empty): the whole pane takes the
    // click, so it becomes the active side all the same.
    if pane.loading.is_some() || pane.error.is_some() || pane.entries.is_empty() {
        let bg = ui.interact(whole, ui.id().with(("pane-bare", id)), Sense::click());
        if bg.clicked() {
            events.push(PaneEvent::ClearSelection);
        }
        if bg.secondary_clicked() {
            events.push(PaneEvent::Context(None));
        }
    }
    if pane.loading.is_some() {
        ui.centered_and_justified(|ui| ui.spinner());
        return events;
    }
    if let Some(err) = &pane.error {
        ui.centered_and_justified(|ui| ui.add(egui::Label::new(egui::RichText::new(err).color(ui.visuals().error_fg_color)).selectable(false)));
        return events;
    }
    if pane.entries.is_empty() {
        let empty = if pane.filter.is_some() || matches!(pane.loc, crate::vfs::Location::Search { .. }) {
            tr("Nothing found")
        } else {
            tr("Empty folder")
        };
        ui.centered_and_justified(|ui| ui.add(egui::Label::new(egui::RichText::new(empty).weak()).selectable(false)));
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
    // A check box, the icon, the name.
    let check_w = if finger { 40.0 } else { 22.0 };
    let name_x = check_w + list_icon + 20.0;
    // Narrow panes (dual mode, big fonts) drop the type column first, then
    // the date, so names keep room. Column x positions are offsets from the
    // right edge.
    let show_date = width > 560.0 * k;
    let show_type = width > 780.0 * k;
    let size_right = if show_date { 160.0 * k } else { 12.0 };
    let type_w = 150.0 * k;
    let type_right = size_right + 100.0 * k + type_w;
    // Tags: a column of their own when anything here has some, left of the
    // type (or the size).
    let show_tags = width > 520.0 * k && pane.entries.iter().any(|e| !e.tags.is_empty());
    let tags_w = 150.0 * k;
    let tags_right = if show_type { type_right } else { size_right + 100.0 * k };
    let name_room = if show_tags {
        tags_right + tags_w + 10.0
    } else if show_type {
        type_right + 10.0
    } else if show_date {
        260.0 * k
    } else {
        100.0 * k
    };
    let cols = if grid { ((width - 8.0) / grid_w).floor().max(1.0) as usize } else { 1 };
    // The room a further column doesn't fit in goes between the columns.
    let grid_w = if grid { (width - 8.0) / cols as f32 } else { grid_w };
    pane.cols = cols;
    let row_h = if grid { grid_h } else { list_row };
    let rows = pane.entries.len().div_ceil(cols);

    if !grid {
        // Column header: a click sorts by the column, again the other way.
        let (r, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
        let mut heads = Vec::new();
        let f = FontId::proportional(12.0);
        let arrow = if cfg.sort_reverse { " ▼" } else { " ▲" };
        let mut head = |x0: f32, x1: f32, label: &str, key: SortKey, right: bool| {
            let cell = Rect::from_min_max(pos2(x0, r.min.y), pos2(x1, r.max.y));
            let resp = ui.interact(cell, ui.id().with(("sort-head", id, key as u8)), Sense::click());
            let on = cfg.sort == key;
            let c = if on || resp.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
            if resp.hovered() {
                ui.painter().rect_filled(cell.shrink2(vec2(0.0, 1.0)), 4.0, ui.visuals().widgets.hovered.weak_bg_fill.gamma_multiply(0.6));
            }
            let text = if on { format!("{label}{arrow}") } else { label.to_string() };
            let (at, align) = if right { (pos2(x1 - 4.0, r.center().y), Align2::RIGHT_CENTER) } else { (pos2(x0 + 4.0, r.center().y), Align2::LEFT_CENTER) };
            ui.painter().text(at, align, text, f.clone(), c);
            if resp.clicked() {
                events.push(PaneEvent::SortBy(key));
            }
            heads.push((key, cell));
        };
        let name_end = if show_tags {
            r.max.x - tags_right - tags_w
        } else if show_type {
            r.max.x - type_right
        } else {
            r.max.x - size_right - 100.0 * k
        };
        head(r.min.x + name_x - 4.0, name_end, tr("Name"), SortKey::Name, false);
        if show_tags {
            ui.painter().text(pos2(r.max.x - tags_right - tags_w + 4.0, r.center().y), Align2::LEFT_CENTER, tr("Tags"), f.clone(), ui.visuals().weak_text_color());
        }
        if show_type {
            head(r.max.x - type_right, r.max.x - size_right - 100.0 * k, tr("Type"), SortKey::Type, false);
        }
        head(r.max.x - size_right - 100.0 * k, r.max.x - size_right + 4.0, tr("Size"), SortKey::Size, true);
        if show_date {
            head(r.max.x - 149.0 * k, r.max.x, tr("Modified"), SortKey::Date, false);
        }
        pane.heads = heads;
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
                    package_badge(ui, ir, e, path.as_deref());
                    tag_dots(ui, &e.tags, pos2(ir.min.x + 4.0, ir.max.y - 4.0));
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
                    let ir = Rect::from_min_size(pos2(rect.min.x + check_w + 10.0, rect.center().y - icon_size / 2.0), Vec2::splat(icon_size));
                    paint_icon(ui, big(ir), icon, thumb, alpha);
                    lock_badge(ui, ir, e, path.as_deref());
                    package_badge(ui, ir, e, path.as_deref());
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
                    if show_tags {
                        let x0 = rect.max.x - tags_right - tags_w;
                        tag_pills(ui, &e.tags, Rect::from_min_max(pos2(x0, rect.min.y), pos2(x0 + tags_w - 8.0, rect.max.y)), FontId::proportional((list_font - 3.0).max(10.0)));
                    }
                    if show_type {
                        let g = elided(ui, &type_name(e, path.as_deref()), f.clone(), weak, type_w - 8.0, 1);
                        ui.painter().galley(pos2(rect.max.x - type_right, rect.center().y - g.size().y / 2.0), g, weak);
                    }
                    if let (true, Some(t)) = (show_date, e.mtime) {
                        ui.painter().text(pos2(rect.max.x - 145.0 * k, rect.center().y), Align2::LEFT_CENTER, fmt_time(t), f, weak);
                    }
                }
                // The check box: shown on the selected, under the mouse, and
                // all along while anything is selected; a click takes the item
                // in or out, the rest stays (and isn't a click on the row).
                // With a finger (no pointer to hover) they're always there,
                // big enough to hit.
                let mut checked = false;
                if finger || sel || pointer.is_some_and(|p| rect.contains(p)) || !pane.selected.is_empty() {
                    let side = if finger { if grid { 28.0 } else { 26.0 } } else if grid { 16.0 } else { 15.0 };
                    let cb = if grid {
                        // Beside the icon when the cell has room for it.
                        let x = (rect.center().x - icon_size / 2.0 - side - 2.0).clamp(rect.min.x + 4.0, rect.min.x + 6.0 + side);
                        Rect::from_min_size(pos2(x, rect.min.y + 6.0), Vec2::splat(side))
                    } else {
                        Rect::from_center_size(pos2(rect.min.x + 6.0 + check_w / 2.0, rect.center().y), Vec2::splat(side))
                    };
                    // (Drags too: pressing it never picks the row up.)
                    let cr = ui.interact(cb.expand(if finger { 8.0 } else { 3.0 }), ui.id().with(("check", i)), Sense::click_and_drag());
                    let v = ui.visuals();
                    let a = v.selection.bg_fill;
                    if sel {
                        ui.painter().rect_filled(cb, 3.0, a);
                        let k = side / 15.0;
                        let s = Stroke::new(2.0 * k.sqrt(), Color32::WHITE);
                        let (p0, p1, p2) = (cb.left_center() + vec2(3.5, 0.5) * k, cb.center_bottom() + vec2(-1.0, -4.0) * k, cb.right_top() + vec2(-3.5, 4.0) * k);
                        ui.painter().line_segment([p0, p1], s);
                        ui.painter().line_segment([p1, p2], s);
                    } else {
                        if grid {
                            ui.painter().rect_filled(cb, 3.0, v.extreme_bg_color.gamma_multiply(0.6));
                        }
                        ui.painter().rect_stroke(cb, 3.0, Stroke::new(1.3, if cr.hovered() { a } else { v.weak_text_color() }), StrokeKind::Inside);
                    }
                    if cr.clicked() {
                        events.push(PaneEvent::Toggle(i));
                        checked = true;
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
                    if let Some((_, t)) = springing.filter(|(n, _)| *n == e.name) {
                        spring_bar(ui, rect, t);
                    }
                }
                if checked {
                } else if let (true, Some(double)) = (r.clicked(), tap) {
                    // A finger: it opens it, or, opening on two taps, points
                    // at it (only its check box chooses).
                    events.push(if double && cfg.touch_double {
                        PaneEvent::DoubleTap(i)
                    } else if cfg.touch_double {
                        PaneEvent::Click(i, false, false)
                    } else {
                        PaneEvent::Activate(i)
                    });
                } else if r.double_clicked() {
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
                            // A folded volume set: all of its volumes.
                            if let Some((_, _, members)) = pane.collapsed.get(&e.name) {
                                picked.extend(members.iter().cloned());
                            }
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
        } else if let Some(i) = drop_entry.filter(|&i| pane.entries.get(i).is_some_and(|e| e.is_dir || crate::vfs::is_archive_name(&e.name))) {
            events.push(PaneEvent::Rest(i));
        }
    }
    // Sets under the mouse or the cursor unfold (from the next frame) — not
    // while a rectangle is drawn, so it takes folded sets whole.
    let mut open: Vec<String> = hover_set.iter().filter(|_| pane.band.is_none()).cloned().collect();
    pane.sync_sets();
    // Sets partly selected stay open: folded, they'd hide which volumes.
    let mut counts: std::collections::HashMap<&str, (usize, usize)> = Default::default();
    for (n, m) in &pane.volumes {
        let c = counts.entry(m.key.as_str()).or_default();
        c.1 += 1;
        if pane.selected.contains(n) {
            c.0 += 1;
        }
    }
    for (k, (sel, all)) in counts {
        if sel > 0 && sel < all && !open.iter().any(|o| o == k) {
            open.push(k.to_string());
        }
    }
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

/// Tags as small coloured pills from the left of `room`; what doesn't fit
/// becomes "+n".
fn tag_pills(ui: &Ui, tags: &[String], room: Rect, font: FontId) {
    let dark = ui.visuals().dark_mode;
    let mut x = room.min.x;
    let h = font.size + 6.0;
    for (i, t) in tags.iter().enumerate() {
        let g = ui.painter().layout_no_wrap(t.clone(), font.clone(), Color32::WHITE);
        let w = g.size().x + 12.0;
        let left = tags.len() - i;
        let more = if left > 1 { ui.painter().layout_no_wrap(format!("+{}", left - 1), font.clone(), Color32::WHITE).size().x + 6.0 } else { 0.0 };
        if x + w + more > room.max.x {
            ui.painter().text(pos2(x, room.center().y), Align2::LEFT_CENTER, format!("+{left}"), font, ui.visuals().weak_text_color());
            return;
        }
        let pill = Rect::from_min_size(pos2(x, room.center().y - h / 2.0), vec2(w, h));
        let c = crate::tags::color(t, dark);
        ui.painter().rect(pill, h / 2.0, c.gamma_multiply(0.25), Stroke::new(1.0, c), StrokeKind::Inside);
        ui.painter().galley(pos2(pill.min.x + 6.0, pill.center().y - g.size().y / 2.0), g, ui.visuals().text_color());
        x += w + 4.0;
    }
}

/// Tags on an icon (the grid): a dot each, up to four.
fn tag_dots(ui: &Ui, tags: &[String], at: Pos2) {
    let dark = ui.visuals().dark_mode;
    for (i, t) in tags.iter().take(4).enumerate() {
        let c = pos2(at.x + i as f32 * 9.0, at.y);
        ui.painter().circle(c, 4.0, crate::tags::color(t, dark), Stroke::new(1.0, ui.visuals().panel_fill));
    }
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

/// Installable packages show their format: a tag under a big icon, a dot
/// with an arrow on a small one.
fn package_badge(ui: &Ui, icon: Rect, e: &crate::vfs::Entry, path: Option<&std::path::Path>) {
    let (Some(path), Some(mtime)) = (path, e.mtime) else { return };
    if e.is_dir {
        return;
    }
    let Some(k) = crate::packages::package(path, &e.name, mtime) else { return };
    let (name, [r, g, b]) = crate::packages::badge(k);
    let color = Color32::from_rgb(r, g, b);
    let p = ui.painter();
    if icon.width() >= 40.0 {
        let font = FontId::proportional((icon.width() * 0.2).clamp(10.0, 13.0));
        let gal = p.layout_no_wrap(name.to_string(), font, Color32::WHITE);
        let tag = Rect::from_min_size(pos2(icon.min.x - 2.0, icon.max.y - gal.size().y - 4.0), gal.size() + vec2(8.0, 4.0));
        p.rect(tag, 4.0, color, Stroke::new(1.0, ui.visuals().extreme_bg_color), StrokeKind::Outside);
        p.galley(tag.min + vec2(4.0, 2.0), gal, Color32::WHITE);
    } else {
        let rad = (icon.width() * 0.28).max(5.0);
        let c = pos2(icon.min.x + rad * 0.7, icon.max.y - rad * 0.7);
        p.circle(c, rad, color, Stroke::new(1.0, ui.visuals().extreme_bg_color));
        // An arrow into a tray: "installs".
        let st = Stroke::new((rad * 0.22).max(1.0), Color32::WHITE);
        p.line_segment([c + vec2(0.0, -rad * 0.55), c + vec2(0.0, rad * 0.35)], st);
        p.line_segment([c + vec2(-rad * 0.38, 0.0), c + vec2(0.0, rad * 0.38)], st);
        p.line_segment([c + vec2(rad * 0.38, 0.0), c + vec2(0.0, rad * 0.38)], st);
    }
}

/// What kind of file it is, for the list's type column ("PNG image").
fn type_name(e: &crate::vfs::Entry, path: Option<&std::path::Path>) -> String {
    if e.is_dir {
        return tr("Folder").to_string();
    }
    let lower = e.name.to_lowercase();
    // .tar.gz and the like: the whole compound extension.
    let (_, ext) = crate::ops::split_ext(&e.name);
    let ext = ext.trim_start_matches('.').to_uppercase();
    let mime = mime_guess::from_path(&e.name).first_raw().unwrap_or("");
    let package = path.zip(e.mtime).and_then(|(p, t)| crate::packages::package(p, &e.name, t));
    let kind = if package.is_some() {
        tr("Package")
    } else if lower.ends_with(".exe") || lower.ends_with(".msi") {
        tr("Windows program")
    } else if lower.ends_with(".desktop") {
        tr("Launcher")
    } else if lower.ends_with(".appimage") {
        tr("Program")
    } else if lower.ends_with(".sh") {
        tr("Script")
    } else if crate::vfs::is_archive_name(&e.name) {
        tr("Archive")
    } else if mime.starts_with("image/") {
        tr("Image")
    } else if mime.starts_with("video/") {
        tr("Video")
    } else if mime.starts_with("audio/") {
        tr("Audio")
    } else if mime == "application/pdf" || [".doc", ".docx", ".odt", ".rtf", ".epub", ".xls", ".xlsx", ".ods", ".ppt", ".pptx", ".odp"].iter().any(|x| lower.ends_with(x)) {
        tr("Document")
    } else if mime.starts_with("text/") || mime.contains("json") || mime.contains("xml") || mime.contains("javascript") || mime.contains("toml") {
        tr("Text")
    } else if e.executable() {
        tr("Program")
    } else {
        tr("File")
    };
    if ext.is_empty() { kind.to_string() } else { format!("{kind} {ext}") }
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

pub(crate) fn elided(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32, rows: usize) -> Arc<Galley> {
    layout(ui, text, font, color, width, rows, false)
}

pub(crate) fn elided_centered(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32, rows: usize) -> Arc<Galley> {
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
    // Long lists (shell history) scroll, following the cursor.
    let max_h = (ui.ctx().content_rect().height() - 140.0).max(200.0);
    let follow = m.shown != Some(m.cursor);
    m.shown = Some(m.cursor);
    // As tall as its items (up to max_h) from the first frame: a new area
    // starts at the centre, with only half the screen below it.
    let natural = m.items.len() as f32 * 36.0;
    egui::ScrollArea::vertical().id_salt(("menu-scroll", m.title.clone())).max_height(max_h).min_scrolled_height(natural.min(max_h)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        // Line labels up when some items have checkboxes and others don't.
        let has_checks = m.items.iter().any(|i| i.checked.is_some());
        for (i, it) in m.items.iter().enumerate() {
            let (rect, r) = ui.allocate_exact_size(vec2(300.0, 34.0), if it.enabled { Sense::click() } else { Sense::hover() });
            if r.hovered() && it.enabled && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                m.cursor = i;
                m.shown = Some(i);
            }
            let cur = i == m.cursor;
            if cur && follow {
                ui.scroll_to_rect(rect, None);
            }
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
            let sub = matches!(it.cmd, Cmd::Submenu(_));
            // Long labels (commands) are cut short.
            let g = elided_text(ui, &it.label, FontId::proportional(15.0), color, rect.max.x - x - if sub { 28.0 } else { 10.0 });
            ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
            if sub {
                ui.painter().text(pos2(rect.max.x - 12.0, rect.center().y), Align2::RIGHT_CENTER, "›", FontId::proportional(18.0), color);
            }
            if r.clicked() && it.enabled {
                click = Some(i);
            }
        }
    });
    click
}

/// How much of the keys' full size fits `screen`: typing into a terminal
/// leaves it most of the height (the keys take at most 40%), other typing
/// 60%; never wider than the screen.
pub(crate) fn osk_scale(screen: Rect, live: bool) -> f32 {
    let keys_h = keyboard_keys_height(1.0);
    let chrome = if live { 40.0 + 16.0 } else { 40.0 + 46.0 + 30.0 + 56.0 };
    let by_h = (screen.height() * if live { 0.4 } else { 0.6 } - chrome + 40.0) / keys_h;
    let by_w = (screen.width() - 60.0) / (15.0 * 46.0);
    by_h.min(by_w).clamp(0.55, 1.0)
}

fn keyboard_keys_height(s: f32) -> f32 {
    crate::keyboard::ROWS as f32 * (40.0 + 4.0) * s - 4.0 * s
}

fn draw_osk(ui: &mut Ui, k: &mut Osk, hidden: bool, scale: f32) -> Option<OskKey> {
    use crate::keyboard::{self, Slot};
    let mut pressed = None;
    let unit = 46.0 * scale;
    let gap = (4.0 * scale).max(2.0);
    let key_h = 40.0 * scale;
    let width = 15.0 * unit;
    ui.set_width(width);
    // Typing into a terminal: no title, the terminal shows where it goes.
    if !k.live() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&k.title).strong().size(16.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(keyboard::LAYOUTS[k.layout].name).strong().color(ui.visuals().selection.bg_fill));
            });
        });
        ui.add_space(6.0);
    }

    // Text field with caret (none when typing straight into the terminal).
    if !k.live() {
        let (rect, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
        ui.painter().rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
        ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, ui.visuals().selection.bg_fill), StrokeKind::Inside);
        let font = FontId::proportional(17.0);
        // Passwords: an asterisk a character, or nothing at all.
    let shown = |cs: &[char]| -> String {
        match (k.secret, hidden) {
            (true, true) => String::new(),
            (true, false) => "*".repeat(cs.len()),
            _ => cs.iter().collect(),
        }
    };
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
    }

    let (area, _) = ui.allocate_exact_size(vec2(width, keyboard::ROWS as f32 * (key_h + gap) - gap), Sense::hover());
    for r in 0..keyboard::ROWS {
        let mut x = area.min.x;
        let y = area.min.y + r as f32 * (key_h + gap);
        for (col, (slot, w)) in keyboard::row(r).into_iter().enumerate() {
            let rect = Rect::from_min_size(pos2(x + gap / 2.0, y), vec2(w * unit - gap, key_h));
            x += w * unit;
            let key = k.key_at(r, col);
            let resp = ui.interact(rect, ui.id().with(("osk", r, col)), Sense::click());
            let cur = r == k.row && col == k.col;
            let v = ui.visuals();
            // Modifiers that are on stay lit.
            let on = match key {
                OskKey::Shift => k.shift,
                OskKey::Caps => k.caps,
                OskKey::Ctrl => k.ctrl,
                OskKey::Alt => k.alt,
                _ => false,
            };
            let usable = k.live() || !matches!(key, OskKey::Ctrl | OskKey::Alt | OskKey::Tab);
            let special = matches!(slot, Slot::Key(_));
            let bg = if cur {
                v.selection.bg_fill
            } else if on {
                v.selection.bg_fill.gamma_multiply(0.45)
            } else if resp.hovered() {
                v.widgets.hovered.bg_fill
            } else if special {
                v.widgets.inactive.bg_fill.gamma_multiply(0.7)
            } else {
                v.widgets.inactive.bg_fill
            };
            ui.painter().rect_filled(rect, 6.0, bg);
            if cur {
                ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, v.strong_text_color()), StrokeKind::Inside);
            }
            let color = if usable { v.strong_text_color() } else { v.weak_text_color().gamma_multiply(0.6) };
            let label: String = match key {
                OskKey::Char(c) => c.to_string(),
                OskKey::Backspace => "⌫".into(),
                OskKey::Tab => "Tab ⇥".into(),
                // Caps writes katakana on the kana keyboard.
                OskKey::Caps if keyboard::LAYOUTS[k.layout].script == keyboard::Script::Kana => "カナ".into(),
                OskKey::Caps => "Caps".into(),
                OskKey::Enter => "Enter ⏎".into(),
                OskKey::Shift => "⇧ Shift".into(),
                OskKey::Ctrl => "Ctrl".into(),
                OskKey::Alt => "Alt".into(),
                OskKey::Lang => format!("⇄ {}", keyboard::LAYOUTS[keyboard::next_layout(k.layout)].name),
                OskKey::Space => tr("Space").into(),
                OskKey::Left => "←".into(),
                OskKey::Up => "↑".into(),
                OskKey::Down => "↓".into(),
                OskKey::Right => "→".into(),
                OskKey::Paste => tr("Paste").into(),
                OskKey::Ok => tr("OK").into(),
            };
            let mut size = if special { (14.0 * scale).max(10.0) } else { (18.0 * scale).max(12.0) };
            // Some room at the sides: a long word ("Вставить") gets smaller
            // rather than touching the edges.
            let room = rect.width() - 16.0 * scale.max(0.75);
            let w = ui.painter().layout_no_wrap(label.clone(), FontId::proportional(size), color).size().x;
            if w > room {
                size = (size * room / w).max(8.0);
            }
            ui.painter().text(rect.center() + vec2(0.0, 1.0), Align2::CENTER_CENTER, label, FontId::proportional(size), color);
            // The other symbol on the key, small in its corner.
            if let Slot::Ch(i) = slot {
                if let Some(alt) = keyboard::alt_at(k.layout, r, i, k.shift) {
                    ui.painter().text(rect.min + vec2(5.0, 3.0) * scale, Align2::LEFT_TOP, alt.to_string(), FontId::proportional((11.0 * scale).max(8.0)), v.weak_text_color());
                }
            }
            if resp.clicked() {
                k.row = r;
                k.col = col;
                pressed = Some(key);
            }
        }
    }
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

/// Room for three rows of folder colours (Papirus has 28) in the theme picker.
const THEME_CHIPS_H: f32 = 3.0 * 33.0 + 6.0;

/// Nothing changes until Apply (or a double click): the preview shows the
/// pick first.
fn draw_themes(ui: &mut Ui, t: &mut crate::app::ThemePicker, icons: &mut Icons) -> Option<Cmd> {
    let mut run = None;
    ui.label(egui::RichText::new(tr("Icon theme")).strong().size(18.0));
    ui.add_space(8.0);
    ui.horizontal_top(|ui| {
        // Theme list.
        ui.vertical(|ui| {
            ui.set_width(250.0);
            let scroll_to = t.take_scroll();
            egui::ScrollArea::vertical().id_salt("themes").max_height(380.0 + THEME_CHIPS_H).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (i, (id, label)) in t.items.iter().enumerate() {
                    let (rect, r) = ui.allocate_exact_size(vec2(240.0, 34.0), Sense::click());
                    let cur = i == t.cursor;
                    let v = ui.visuals();
                    if cur && t.in_colors {
                        // The pad is over in the colours: the theme stays marked.
                        ui.painter().rect_filled(rect, 6.0, tint(&v, 0.3));
                    } else if cur {
                        ui.painter().rect_filled(rect, 6.0, tint(&v, 0.55));
                        ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, v.selection.bg_fill), StrokeKind::Inside);
                    } else if r.hovered() {
                        ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
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
                    if r.double_clicked() {
                        run = Some(Cmd::SetIconLook(id.clone(), t.color.clone()));
                    } else if r.clicked() {
                        t.cursor = i;
                    }
                }
            });
        });
        // A plain ui.separator() here would stretch to the window's height
        // from the last frame, and the window would grow every frame.
        let (line, _) = ui.allocate_exact_size(vec2(9.0, 380.0 + THEME_CHIPS_H), Sense::hover());
        ui.painter().vline(line.center().x, line.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
        // Preview of the highlighted theme.
        ui.vertical(|ui| {
            let theme = t.preview[t.cursor].clone();
            ui.label(egui::RichText::new(&t.items[t.cursor].1).weak());
            ui.add_space(4.0);
            // Folder colours, where the theme comes in several. The column
            // keeps its height either way, so the window keeps its size.
            let colors = icons.folder_colors(&theme);
            t.colors = colors.clone();
            if !colors.is_empty() {
                let (room, _) = ui.allocate_exact_size(vec2(crate::app::ThemePicker::COLOR_COLS as f32 * 33.0 + 6.0, THEME_CHIPS_H), Sense::hover());
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(room).layout(egui::Layout::left_to_right(Align::Min).with_main_wrap(true)));
                let ui = &mut ui;
                {
                    ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
                    for c in std::iter::once(None).chain(colors.iter().map(Some)) {
                        let (rect, r) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
                        let on = t.color.as_ref() == c;
                        let v = ui.visuals();
                        if on && t.in_colors {
                            ui.painter().rect(rect, 6.0, tint(v, 0.55), Stroke::new(2.5, v.strong_text_color()), StrokeKind::Inside);
                        } else if on {
                            ui.painter().rect(rect, 6.0, tint(v, 0.4), Stroke::new(2.0, v.selection.bg_fill), StrokeKind::Inside);
                        } else if r.hovered() {
                            ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
                        }
                        let names = c.map(|c| format!("folder-{c}")).unwrap_or_else(|| "folder".into());
                        if let Some(u) = icons.named_in(&theme, &[names.as_str()], SMALL_ICON_LOOKUP) {
                            egui::Image::new(u).paint_at(ui, rect.shrink(5.0));
                        }
                        let r = r.on_hover_text(c.cloned().unwrap_or_else(|| tr("Standard").to_string()));
                        if r.clicked() {
                            t.color = c.cloned();
                        }
                    }
                }
            }
            egui::Grid::new("theme-preview").spacing([6.0, 6.0]).show(ui, |ui| {
                for (i, (caption, names)) in THEME_SAMPLES.iter().enumerate() {
                    let (rect, _) = ui.allocate_exact_size(vec2(78.0, 78.0), Sense::hover());
                    let ir = Rect::from_center_size(pos2(rect.center().x, rect.min.y + 28.0), Vec2::splat(48.0));
                    if let Some(u) = icons.named_colored(&theme, names, 64, t.color.as_deref()) {
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
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            // For the mouse; the pad has its keys in the hints below.
            if ui.button(tr("Cancel")).clicked() {
                run = Some(Cmd::Nothing);
            }
            if ui.button(egui::RichText::new(tr("OK")).strong()).clicked() {
                run = Some(Cmd::SetIconLook(t.items[t.cursor].0.clone(), t.color.clone()));
            }
        });
    });
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

fn draw_help(ui: &mut Ui, pad: PadKind, scroll: Option<f32>) -> bool {
    ui.label(egui::RichText::new(tr("Controls")).strong().size(18.0));
    ui.add_space(8.0);
    // (the pad's buttons, what it does, the keys); no buttons: keyboard only.
    let rows: [(&[Action], &str, &str); 27] = [
        (&[Action::Up], tr("Move"), "←↑→↓"),
        (&[Action::Accept], tr("Open"), "Enter"),
        (&[Action::Back], tr("Back"), "Backspace / Esc"),
        (&[Action::Select], tr("Select"), "Space / Insert"),
        (&[], tr("Select all"), "Ctrl+A"),
        (&[Action::Actions], tr("Actions"), "Shift+F10"),
        (&[Action::Menu], tr("Menu"), "F10"),
        (&[], tr("Copy / cut / paste"), "Ctrl+C / X / V"),
        (&[Action::Actions], tr("Rename"), "F2"),
        (&[Action::Actions], tr("Move to trash"), "Delete"),
        (&[Action::Actions], tr("Delete permanently"), "Shift+Delete"),
        (&[Action::Menu], tr("New folder"), "Ctrl+Shift+N"),
        (&[Action::View], tr("Dual pane"), "F3"),
        (&[Action::PrevTab, Action::NextTab], tr("Tabs"), "Ctrl+← → / Ctrl+Tab"),
        (&[Action::Menu], tr("New tab"), "Ctrl+T"),
        (&[Action::Menu], tr("Close tab"), "Ctrl+W"),
        (&[], tr("Reopen closed tab"), "Ctrl+Shift+T"),
        (&[Action::LeftTrigger, Action::RightTrigger], tr("Panes / page"), "Tab / PgUp PgDn"),
        (&[Action::LeftStick], tr("Sidebar"), "Ctrl+B / F9"),
        (&[], tr("History"), "Alt+← →"),
        (&[], tr("Home folder"), "Alt+Home"),
        (&[Action::Menu], tr("Filter / search"), "Ctrl+F / Ctrl+Shift+F"),
        (&[Action::Menu], tr("Go to path…"), "Ctrl+L"),
        (&[], tr("Refresh"), "F5 / Ctrl+R"),
        (&[Action::Actions], tr("Properties"), "Alt+Enter"),
        (&[Action::Menu], tr("View"), "Ctrl+1 / Ctrl+2"),
        (&[Action::Back], tr("Leave the terminal"), "Esc held"),
    ];
    let mut sa = egui::ScrollArea::vertical().id_salt("help").max_height((ui.ctx().content_rect().height() - 160.0).max(200.0));
    if let Some(d) = scroll.filter(|d| *d != 0.0) {
        let now = ui.ctx().data(|m| m.get_temp::<f32>(egui::Id::new("help-scroll"))).unwrap_or(0.0);
        sa = sa.vertical_scroll_offset((now + d).max(0.0));
    }
    let out = sa.show(ui, |ui| {
        egui::Grid::new("help").num_columns(3).spacing([18.0, 6.0]).show(ui, |ui| {
            ui.label(egui::RichText::new(tr("Gamepad")).weak());
            ui.label("");
            ui.label(egui::RichText::new(tr("Keyboard")).weak());
            ui.end_row();
            for (buttons, label, keys) in rows {
                ui.horizontal(|ui| {
                    if buttons.is_empty() {
                        ui.label(egui::RichText::new("—").weak()); // keyboard / mouse only
                    }
                    for a in buttons {
                        glyph(ui, *a, pad);
                    }
                });
                ui.label(label);
                ui.label(egui::RichText::new(keys).monospace());
                ui.end_row();
            }
        });
    });
    ui.ctx().data_mut(|m| m.insert_temp(egui::Id::new("help-scroll"), out.state.offset.y));
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
    /// Polka's own title bar: dragged, double-clicked, its buttons.
    MoveWindow,
    Maximize,
    Window(crate::titlebar::WinButton),
}

/// Theme size to ask for an icon drawn `pt` points wide, rounded up to a
/// size themes ship natively. Odd sizes (48, 56…) make lookups fall back to
/// scaled-up small icons (Breeze's `16@3x`), which are monochrome glyphs.
pub(crate) fn lookup_px(ui: &Ui, pt: f32) -> u16 {
    let px = (pt * ui.pixels_per_point()).ceil() as u16;
    [SMALL_ICON_LOOKUP, 64, 128, 256].into_iter().find(|s| *s >= px).unwrap_or(256)
}

/// Square toolbar button with a themed icon (text fallback).
pub(crate) fn tool_button(ui: &mut Ui, rect: Rect, icons: &mut Icons, names: &[&str], fallback: &str, tip: &str, on: bool) -> egui::Response {
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
/// One side's address: crumbs to click. While something is dragged, the
/// crumb it rests on is returned (it opens after a moment).
#[allow(clippy::too_many_arguments)]
fn address_field(
    ui: &mut Ui,
    rect: Rect,
    pane: &Pane,
    highlight: bool,
    idx: usize,
    file: Option<&str>,
    dragging: bool,
    spring: &Option<(crate::app::SpringAt, std::time::Instant, bool)>,
    drawn: &mut Vec<(usize, Rect, crate::vfs::Location)>,
    title_bar: bool,
) -> (Option<TopAction>, Option<(usize, crate::vfs::Location)>) {
    let mut rest = None;
    let mut act = None;
    // Being the title bar, the address moves the window when dragged.
    let sense = if title_bar { Sense::click_and_drag() } else { Sense::click() };
    let moved = |r: &egui::Response| r.drag_started_by(egui::PointerButton::Primary);
    let bg = ui.interact(rect, ui.id().with(("addr", idx)), sense);
    let v = ui.visuals().clone();
    let stroke = if highlight { Stroke::new(1.5, v.selection.bg_fill) } else { v.widgets.noninteractive.bg_stroke };
    ui.painter().rect(rect, 8.0, v.extreme_bg_color, stroke, StrokeKind::Inside);
    if title_bar && bg.double_clicked() {
        act = Some(TopAction::Maximize);
    } else if bg.clicked() {
        act = Some(TopAction::Focus(idx));
    } else if moved(&bg) {
        act = Some(TopAction::MoveWindow);
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
    let folders = crumbs.len();
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
        let resp = ui.interact(r, ui.id().with(("crumb", idx, i)), sense);
        if moved(&resp) {
            act = Some(TopAction::MoveWindow);
        }
        if resp.hovered() {
            painter.rect_filled(r, 6.0, v.widgets.hovered.weak_bg_fill);
        }
        if i < folders {
            drawn.push((idx, r, crumbs[i].1.clone()));
        }
        // A drag resting on a folder of the path (not the one shown).
        let under = dragging && i < folders && crumbs[i].1 != pane.loc && ui.input(|inp| inp.pointer.hover_pos()).is_some_and(|p| r.contains(p));
        if under {
            painter.rect(r, 6.0, tint(&v, 0.35), Stroke::new(1.5, v.selection.bg_fill), StrokeKind::Inside);
            let at = crate::app::SpringAt::Crumb(idx, crumbs[i].1.clone());
            if let Some(t) = spring_progress(spring, &at) {
                spring_bar(ui, r, t);
            }
            rest = Some((idx, crumbs[i].1.clone()));
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
    (act, rest)
}

/// How far a drag resting on `at` is from opening it (None: it isn't).
fn spring_progress(spring: &Option<(crate::app::SpringAt, std::time::Instant, bool)>, at: &crate::app::SpringAt) -> Option<f32> {
    match spring {
        Some((a, since, false)) if a == at => Some((since.elapsed().as_secs_f32() / crate::app::SPRING.as_secs_f32()).min(1.0)),
        _ => None,
    }
}

/// The wait before a place opens under a drag: a line filling along the
/// bottom of `rect`.
fn spring_bar(ui: &Ui, rect: Rect, t: f32) {
    let y = rect.max.y - 3.0;
    let x0 = rect.min.x + 6.0;
    let x1 = x0 + (rect.width() - 12.0) * t;
    ui.painter().line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(3.0, ui.visuals().selection.bg_fill));
}

// ---------------------------------------------------------------- settings

pub enum SettingsEvent {
    LayoutChip(usize),
    /// A window buttons' look picked.
    WinStyle(usize),
    Row(usize),
    Step(usize, f32),
    Pick(usize),
    Theme,
    Close,
    Chip(usize),
    Toggle(usize),
    /// Light/dark choice clicked.
    Appearance(u8),
    SchemesFolder,
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
            SettingsEvent::SchemesFolder => self.run(Cmd::SchemesFolder),
            SettingsEvent::Chip(i) => self.settings_toggle_open(i),
            SettingsEvent::LayoutChip(i) => self.settings_toggle_layout(i),
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
            SettingsEvent::WinStyle(i) => {
                st.row = row::WIN_STYLE;
                self.cfg.win_style = crate::titlebar::STYLES[i];
                self.cfg.save();
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

pub(crate) const SETTINGS_W: f32 = 560.0;

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Row background; the focused row gets the highlight.
pub(crate) fn settings_row(ui: &mut Ui, h: f32, focused: bool, id: impl std::hash::Hash) -> (Rect, egui::Response) {
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

fn draw_settings(ui: &mut Ui, st: &mut crate::app::Settings, cfg: &Config, icons: &mut Icons, schemes: &[crate::schemes::Scheme]) -> Option<SettingsEvent> {
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
            let fill = rgb(sc.unwrap_or_else(crate::schemes::default_accent));
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
        // A colour scheme is light or dark of itself.
        let off = cfg.scheme.is_some();
        if on {
            ui.painter().rect_filled(seg, 6.0, if off { v.selection.bg_fill.gamma_multiply(0.35) } else { v.selection.bg_fill });
        } else if resp.hovered() {
            ui.painter().rect_filled(seg, 6.0, v.widgets.hovered.weak_bg_fill);
        }
        let c = if off { v.weak_text_color() } else if on { v.selection.stroke.color } else { v.text_color() };
        ui.painter().text(seg.center(), Align2::CENTER_CENTER, *name, FontId::proportional(14.0), c);
        if resp.clicked() {
            ev = Some(SettingsEvent::Appearance(i as u8));
        }
    }

    // Colour scheme: ‹ name ›, its colours beside it.
    let (rect, r) = settings_row(ui, 44.0, st.row == row::SCHEME, "scheme");
    if r.clicked() {
        ev = Some(SettingsEvent::Row(row::SCHEME));
    }
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Color scheme"), label_font.clone(), v.text_color());
    // One's own schemes: their folder (△ on the pad).
    let fb = Rect::from_center_size(pos2(rect.max.x - 340.0, rect.center().y), Vec2::splat(28.0));
    let fresp = ui.interact(fb, ui.id().with("scheme-folder"), Sense::click());
    if fresp.hovered() {
        ui.painter().rect_filled(fb, 6.0, v.widgets.hovered.weak_bg_fill);
    }
    if let Some(u) = icons.named(&["folder"], lookup_px(ui, 20.0)) {
        egui::Image::new(u).paint_at(ui, fb.shrink(4.0));
    }
    if fresp.on_hover_text(tr("Your own schemes (JSON)")).clicked() {
        ev = Some(SettingsEvent::SchemesFolder);
    }
    let scheme = cfg.scheme.as_ref().and_then(|id| schemes.iter().find(|s| s.id == *id));
    let name = scheme.map_or_else(|| tr("Standard").to_string(), |s| s.label());
    let (minus, plus) = (
        Rect::from_center_size(pos2(rect.max.x - 300.0, rect.center().y), Vec2::splat(28.0)),
        Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(28.0)),
    );
    for (b, label, d) in [(minus, "‹", -1.0), (plus, "›", 1.0)] {
        let resp = ui.interact(b, ui.id().with(("scheme", label)), Sense::click());
        ui.painter().rect_filled(b, 6.0, if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { v.widgets.inactive.weak_bg_fill });
        ui.painter().text(b.center(), Align2::CENTER_CENTER, label, FontId::proportional(18.0), v.text_color());
        if resp.clicked() {
            ev = Some(SettingsEvent::Step(row::SCHEME, d));
        }
    }
    let mid = (minus.max.x + plus.min.x) / 2.0;
    ui.painter().text(pos2(mid, rect.center().y), Align2::CENTER_CENTER, name, label_font.clone(), v.strong_text_color());
    if let Some(s) = scheme {
        // Background, controls, text, highlight: a strip under the name.
        for (i, c) in [s.background, s.surface, s.text, s.accent].into_iter().enumerate() {
            let at = Rect::from_center_size(pos2(mid - 27.0 + i as f32 * 18.0, rect.max.y - 6.0), vec2(16.0, 5.0));
            ui.painter().rect(at, 2.0, c, Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color), StrokeKind::Outside);
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
        (row::ZOOM, tr("Interface scale"), format!("{:.0}%", if crate::steam_add::game_mode() { cfg.zoom_game } else { cfg.zoom } * 100.0)),
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

    // Keyboard layouts for ⇄.
    // Up to eight a line.
    const PER_LINE: usize = 8;
    let lines = crate::keyboard::CODES.len().div_ceil(PER_LINE);
    let (rect, r) = settings_row(ui, 44.0 + 42.0 * lines as f32, st.row == row::LAYOUTS, "layouts");
    if r.clicked() {
        ev = Some(SettingsEvent::Row(row::LAYOUTS));
    }
    ui.painter().text(rect.min + vec2(14.0, 18.0), Align2::LEFT_CENTER, tr("Keyboard layouts"), label_font.clone(), v.text_color());
    let on_layouts = cfg.layouts();
    let n = crate::keyboard::CODES.len().min(PER_LINE);
    let chip_w = (SETTINGS_W - 28.0 - (n as f32 - 1.0) * 6.0) / n as f32;
    for (i, code) in crate::keyboard::CODES.iter().enumerate() {
        let (line, col) = (i / PER_LINE, i % PER_LINE);
        let chip = Rect::from_min_size(pos2(rect.min.x + 14.0 + col as f32 * (chip_w + 6.0), rect.min.y + 38.0 + line as f32 * 42.0), vec2(chip_w, 36.0));
        let resp = ui.interact(chip, ui.id().with(("layout-chip", i)), Sense::click());
        let on = on_layouts.iter().any(|c| c == code);
        let cursor = st.row == row::LAYOUTS && st.layout_chip == i;
        let fill = if on { tint(&v, 0.45) } else if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { v.widgets.inactive.weak_bg_fill };
        let stroke = if cursor { Stroke::new(2.0, v.strong_text_color()) } else { Stroke::NONE };
        ui.painter().rect(chip, 8.0, fill, stroke, StrokeKind::Inside);
        let label = crate::keyboard::LAYOUTS[i].name;
        ui.painter().text(chip.center(), Align2::CENTER_CENTER, label, FontId::proportional(15.0), v.text_color());
        if resp.clicked() {
            ev = Some(SettingsEvent::LayoutChip(i));
        }
    }

    // Packages install on ✕.
    let (rect, r) = settings_row(ui, 44.0, st.row == row::INSTALL, "install");
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Install packages on open"), label_font.clone(), v.text_color());
    let b = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(20.0));
    ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
    if cfg.install_on_open {
        ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), v.strong_text_color());
    }
    if r.clicked() {
        ev = Some(SettingsEvent::Toggle(row::INSTALL));
    }

    // How typed passwords show.
    let (rect, r) = settings_row(ui, 44.0, st.row == row::PASSWORDS, "passwords");
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Passwords as you type"), label_font.clone(), v.text_color());
    let shown = if cfg.password_hidden { tr("Hidden") } else { "* * * *" };
    ui.painter().text(pos2(rect.max.x - 16.0, rect.center().y), Align2::RIGHT_CENTER, format!("◀ {shown} ▶"), FontId::proportional(15.0), v.strong_text_color());
    if r.clicked() {
        ev = Some(SettingsEvent::Toggle(row::PASSWORDS));
    }

    // Dual mode: the right pane where it was.
    let (rect, r) = settings_row(ui, 44.0, st.row == row::REMEMBER_RIGHT, "remember-right");
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Right pane opens where it was"), label_font.clone(), v.text_color());
    let b = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(20.0));
    ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
    if cfg.remember_right {
        ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), v.strong_text_color());
    }
    if r.clicked() {
        ev = Some(SettingsEvent::Toggle(row::REMEMBER_RIGHT));
    }

    // A finger opens on one tap or two.
    let (rect, r) = settings_row(ui, 44.0, st.row == row::TOUCH_OPEN, "touch-open");
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Open by touch"), label_font.clone(), v.text_color());
    let shown = if cfg.touch_double { tr("Double tap") } else { tr("One tap") };
    ui.painter().text(pos2(rect.max.x - 16.0, rect.center().y), Align2::RIGHT_CENTER, format!("◀ {shown} ▶"), FontId::proportional(15.0), v.strong_text_color());
    if r.clicked() {
        ev = Some(SettingsEvent::Toggle(row::TOUCH_OPEN));
    }

    // The desktop's title bar, or Polka's top bar as one.
    let (rect, r) = settings_row(ui, 44.0, st.row == row::TITLEBAR, "titlebar");
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("System title bar"), label_font.clone(), v.text_color());
    let b = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(20.0));
    ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
    if cfg.system_titlebar {
        ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), v.strong_text_color());
    }
    if r.clicked() {
        ev = Some(SettingsEvent::Toggle(row::TITLEBAR));
    }

    // Its buttons (shown without the system's title bar): the look, each
    // tile a preview…
    use crate::titlebar::{ORDERS, STYLES, preview};
    let order = ORDERS[cfg.win_order as usize % ORDERS.len()];
    const PER: usize = 4;
    let lines = STYLES.len().div_ceil(PER);
    let (rect, r) = settings_row(ui, 44.0 + 66.0 * lines as f32, st.row == row::WIN_STYLE, "win-style");
    if r.clicked() {
        ev = Some(SettingsEvent::Row(row::WIN_STYLE));
    }
    ui.painter().text(rect.min + vec2(14.0, 18.0), Align2::LEFT_CENTER, tr("Window buttons"), label_font.clone(), v.text_color());
    let tile_w = (SETTINGS_W - 28.0 - (PER as f32 - 1.0) * 8.0) / PER as f32;
    for (i, style) in STYLES.iter().enumerate() {
        let (line, col) = (i / PER, i % PER);
        let tile = Rect::from_min_size(pos2(rect.min.x + 14.0 + col as f32 * (tile_w + 8.0), rect.min.y + 38.0 + line as f32 * 66.0), vec2(tile_w, 60.0));
        let resp = ui.interact(tile, ui.id().with(("win-style", i)), Sense::click());
        let on = cfg.win_style == *style;
        let fill = if on { tint(&v, 0.45) } else if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { v.widgets.inactive.weak_bg_fill };
        let stroke = if on && st.row == row::WIN_STYLE { Stroke::new(2.0, v.strong_text_color()) } else { Stroke::NONE };
        ui.painter().rect(tile, 8.0, fill, stroke, StrokeKind::Inside);
        preview(ui, icons, *style, &order, Rect::from_min_size(tile.min, vec2(tile.width(), 36.0)), 0.75);
        ui.painter().text(pos2(tile.center().x, tile.max.y - 13.0), Align2::CENTER_CENTER, style.name(), FontId::proportional(13.0), v.text_color());
        if resp.clicked() {
            ev = Some(SettingsEvent::WinStyle(i));
        }
    }

    // …their side…
    let (rect, r) = settings_row(ui, 44.0, st.row == row::WIN_SIDE, "win-side");
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Window buttons side"), label_font.clone(), v.text_color());
    let shown = if cfg.win_left { tr("Left") } else { tr("Right") };
    ui.painter().text(pos2(rect.max.x - 16.0, rect.center().y), Align2::RIGHT_CENTER, format!("◀ {shown} ▶"), FontId::proportional(15.0), v.strong_text_color());
    if r.clicked() {
        ev = Some(SettingsEvent::Toggle(row::WIN_SIDE));
    }

    // …and order.
    let (rect, r) = settings_row(ui, 48.0, st.row == row::WIN_ORDER, "win-order");
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Button order"), label_font.clone(), v.text_color());
    let f = FontId::proportional(15.0);
    ui.painter().text(pos2(rect.max.x - 16.0, rect.center().y), Align2::RIGHT_CENTER, "▶", f.clone(), v.strong_text_color());
    let shown = Rect::from_min_max(pos2(rect.max.x - 210.0, rect.min.y), pos2(rect.max.x - 36.0, rect.max.y));
    preview(ui, icons, cfg.win_style, &order, shown, 1.0);
    ui.painter().text(pos2(shown.min.x - 4.0, rect.center().y), Align2::RIGHT_CENTER, "◀", f, v.strong_text_color());
    if r.clicked() {
        ev = Some(SettingsEvent::Toggle(row::WIN_ORDER));
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
                if let Some(u) = g.names.first().and_then(|n| icons.named_plain(&[n.as_str()], SMALL_ICON_LOOKUP)) {
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
                if let Some(u) = icons.named_plain(&[name.as_str()], 64) {
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
