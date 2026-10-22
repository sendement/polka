//! Polka's own title bar, when the desktop's is turned off: the top bar
//! moves the window (dragged by the address), and gets the window's
//! buttons, drawn the way the desktop draws its own (KDE's Breeze, GNOME's
//! Adwaita, else the icon theme's); the window's edges resize it.

use egui::{Color32, CursorIcon, Rect, ResizeDirection, Sense, Stroke, Ui, Vec2, ViewportCommand, pos2, vec2};

use crate::i18n::tr;
use crate::icons::Icons;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Desktop {
    Kde,
    Gnome,
    Other,
}

/// Whose window buttons to look like (from XDG_CURRENT_DESKTOP).
pub fn desktop() -> Desktop {
    static D: std::sync::OnceLock<Desktop> = std::sync::OnceLock::new();
    *D.get_or_init(|| from_env(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default()))
}

fn from_env(d: &str) -> Desktop {
    let d = d.to_uppercase();
    if d.split(':').any(|p| p == "KDE") {
        Desktop::Kde
    } else if d.split(':').any(|p| ["GNOME", "UNITY", "PANTHEON", "BUDGIE", "CINNAMON", "X-CINNAMON"].contains(&p)) {
        Desktop::Gnome
    } else {
        Desktop::Other
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum WinButton {
    Minimize,
    Maximize,
    Close,
}

/// Room the three buttons take at the right of the top bar.
pub fn buttons_width(d: Desktop) -> f32 {
    3.0 * button_size(d) + 2.0 * gap(d) + 10.0
}

fn button_size(d: Desktop) -> f32 {
    match d {
        Desktop::Gnome => 30.0,
        _ => 34.0,
    }
}

fn gap(d: Desktop) -> f32 {
    match d {
        Desktop::Gnome => 10.0,
        _ => 4.0,
    }
}

/// The three buttons, right-aligned at `right` on the row's middle `cy`;
/// the one clicked.
pub fn buttons(ui: &mut Ui, icons: &mut Icons, d: Desktop, right: f32, cy: f32) -> Option<WinButton> {
    let maximized = ui.input(|i| i.viewport().maximized).unwrap_or(false);
    let size = button_size(d);
    let mut x = right - size;
    let mut hit = None;
    for b in [WinButton::Close, WinButton::Maximize, WinButton::Minimize] {
        let rect = Rect::from_center_size(pos2(x + size / 2.0, cy), Vec2::splat(size));
        if button(ui, icons, d, b, rect, maximized).clicked() {
            hit = Some(b);
        }
        x -= size + gap(d);
    }
    hit
}

fn button(ui: &mut Ui, icons: &mut Icons, d: Desktop, b: WinButton, rect: Rect, maximized: bool) -> egui::Response {
    let r = ui.interact(rect, ui.id().with(("win-button", b as u8)), Sense::click());
    let v = ui.visuals().clone();
    let dark = v.dark_mode;
    let hot = r.hovered() || r.is_pointer_button_down_on();
    // Breeze: a close button going red under the pointer, the others a
    // circle; Adwaita: small round buttons, always on a soft disc.
    let close_red = Color32::from_rgb(0xda, 0x44, 0x53);
    let (fill, fg) = match d {
        Desktop::Gnome => {
            let base = if dark { Color32::from_white_alpha(26) } else { Color32::from_black_alpha(20) };
            let over = if dark { Color32::from_white_alpha(46) } else { Color32::from_black_alpha(36) };
            (Some(if hot { over } else { base }), v.text_color())
        }
        _ if b == WinButton::Close && hot => (Some(close_red), Color32::WHITE),
        _ => (hot.then_some(v.widgets.hovered.weak_bg_fill), v.text_color()),
    };
    let disc = match d {
        Desktop::Gnome => Rect::from_center_size(rect.center(), Vec2::splat(24.0)),
        Desktop::Kde => Rect::from_center_size(rect.center(), Vec2::splat(22.0)),
        Desktop::Other => rect.shrink(2.0),
    };
    if let Some(f) = fill {
        let round = if d == Desktop::Other { 8.0 } else { disc.width() / 2.0 };
        ui.painter().rect_filled(disc, round, f);
    }
    let names: &[&str] = match b {
        WinButton::Minimize => &["window-minimize"],
        WinButton::Maximize if maximized => &["window-restore"],
        WinButton::Maximize => &["window-maximize"],
        WinButton::Close => &["window-close"],
    };
    let pt = if d == Desktop::Gnome { 16.0 } else { 18.0 };
    let px = ((pt * ui.pixels_per_point()).ceil() as u16).max(16);
    let theme = match d {
        Desktop::Kde if dark => Some("breeze-dark"),
        Desktop::Kde => Some("breeze"),
        Desktop::Gnome => Some("Adwaita"),
        Desktop::Other => None,
    };
    let found = match theme {
        Some(t) => icons.ui_icon_in(t, names, px, fg).or_else(|| icons.ui_icon(names, px, fg)),
        None => icons.ui_icon(names, px, fg),
    };
    match found {
        Some(u) => egui::Image::new(u).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(pt))),
        None => glyph(ui, b, maximized, rect.center(), fg),
    }
    let tip = match b {
        WinButton::Minimize => tr("Minimize"),
        WinButton::Maximize if maximized => tr("Restore"),
        WinButton::Maximize => tr("Maximize"),
        WinButton::Close => tr("Close"),
    };
    r.on_hover_text(tip)
}

/// No icon anywhere: the shapes drawn.
fn glyph(ui: &Ui, b: WinButton, maximized: bool, c: egui::Pos2, color: Color32) {
    let s = Stroke::new(1.5, color);
    let p = ui.painter();
    match b {
        WinButton::Minimize => {
            p.line_segment([c + vec2(-5.0, 4.0), c + vec2(5.0, 4.0)], s);
        }
        WinButton::Maximize if maximized => {
            p.rect_stroke(Rect::from_center_size(c + vec2(-1.5, 1.5), Vec2::splat(8.0)), 1.0, s, egui::StrokeKind::Middle);
            p.line_segment([c + vec2(-2.0, -5.5), c + vec2(5.5, -5.5)], s);
            p.line_segment([c + vec2(5.5, -5.5), c + vec2(5.5, 2.0)], s);
        }
        WinButton::Maximize => {
            p.rect_stroke(Rect::from_center_size(c, Vec2::splat(10.0)), 1.0, s, egui::StrokeKind::Middle);
        }
        WinButton::Close => {
            p.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], s);
            p.line_segment([c + vec2(5.0, -5.0), c + vec2(-5.0, 5.0)], s);
        }
    }
}

/// A button clicked: what the window does.
pub fn press(ctx: &egui::Context, b: WinButton) {
    match b {
        WinButton::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
        WinButton::Maximize => toggle_maximized(ctx),
        WinButton::Close => ctx.send_viewport_cmd(ViewportCommand::Close),
    }
}

pub fn toggle_maximized(ctx: &egui::Context) {
    let m = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!m));
}

/// The window's edges and corners resize it (no frame of the desktop's to
/// take hold of): a few points along the rim, the corners a little further.
pub fn resize_edges(ctx: &egui::Context) {
    let (maximized, pointer, pressed) = ctx.input(|i| {
        let v = i.viewport();
        (v.maximized.unwrap_or(false) || v.fullscreen.unwrap_or(false), i.pointer.hover_pos(), i.pointer.primary_pressed())
    });
    let Some(p) = pointer.filter(|_| !maximized) else { return };
    let s = ctx.content_rect();
    const W: f32 = 5.0;
    const C: f32 = 16.0;
    let (l, r, t, b) = (p.x - s.min.x, s.max.x - p.x, p.y - s.min.y, s.max.y - p.y);
    if l.min(r).min(t).min(b) > W {
        return;
    }
    use ResizeDirection as D;
    let (dir, cursor) = match (l < C, r < C, t < C, b < C) {
        (true, _, true, _) => (D::NorthWest, CursorIcon::ResizeNorthWest),
        (_, true, true, _) => (D::NorthEast, CursorIcon::ResizeNorthEast),
        (true, _, _, true) => (D::SouthWest, CursorIcon::ResizeSouthWest),
        (_, true, _, true) => (D::SouthEast, CursorIcon::ResizeSouthEast),
        _ if t <= W => (D::North, CursorIcon::ResizeVertical),
        _ if b <= W => (D::South, CursorIcon::ResizeVertical),
        _ if l <= W => (D::West, CursorIcon::ResizeHorizontal),
        _ => (D::East, CursorIcon::ResizeHorizontal),
    };
    ctx.set_cursor_icon(cursor);
    if pressed {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktops() {
        assert_eq!(from_env("KDE"), Desktop::Kde);
        assert_eq!(from_env("ubuntu:GNOME"), Desktop::Gnome);
        assert_eq!(from_env("Hyprland"), Desktop::Other);
        assert_eq!(from_env(""), Desktop::Other);
    }
}
