//! Polka's own title bar, when the desktop's is turned off: the top bar
//! moves the window (dragged by the address), and gets the window's
//! buttons, in a style as set (as the desktop's own — KDE's Breeze, GNOME's
//! Adwaita, else the icon theme's — or macOS's, Windows 11's…), on either
//! side, in any order; the window's edges resize it.

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WinButton {
    Minimize,
    Maximize,
    Close,
}

/// The orders the three buttons can come in (the settings' "Button order"
/// steps through them).
pub const ORDERS: [[WinButton; 3]; 6] = {
    use WinButton::*;
    [[Minimize, Maximize, Close], [Close, Minimize, Maximize], [Close, Maximize, Minimize], [Minimize, Close, Maximize], [Maximize, Minimize, Close], [Maximize, Close, Minimize]]
};

/// How the buttons look.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// As the desktop's: Breeze on KDE, Adwaita on GNOME, else the icon
    /// theme's.
    #[default]
    Auto,
    Breeze,
    Adwaita,
    /// Ubuntu's Yaru: an orange close button.
    Yaru,
    /// Traffic lights.
    Macos,
    Win11,
    /// The icon theme's window-* icons.
    Icons,
    /// Thin lines, nothing around them.
    Plain,
}

pub const STYLES: [Style; 8] = [Style::Auto, Style::Breeze, Style::Adwaita, Style::Yaru, Style::Macos, Style::Win11, Style::Icons, Style::Plain];

impl Style {
    pub fn name(self) -> &'static str {
        match self {
            Style::Auto => tr("As the desktop"),
            Style::Breeze => "Breeze",
            Style::Adwaita => "Adwaita",
            Style::Yaru => "Ubuntu",
            Style::Macos => "macOS",
            Style::Win11 => "Windows 11",
            Style::Icons => tr("Icon theme"),
            Style::Plain => tr("Simple"),
        }
    }

    /// What Auto is here.
    pub fn resolve(self) -> Style {
        match (self, desktop()) {
            (Style::Auto, Desktop::Kde) => Style::Breeze,
            (Style::Auto, Desktop::Gnome) => Style::Adwaita,
            (Style::Auto, Desktop::Other) => Style::Icons,
            (s, _) => s,
        }
    }

    /// A button's size and the gap between them (at scale 1).
    fn metrics(self) -> (Vec2, f32) {
        match self.resolve() {
            Style::Adwaita => (Vec2::splat(30.0), 10.0),
            Style::Yaru => (Vec2::splat(30.0), 6.0),
            Style::Macos => (Vec2::splat(20.0), 4.0),
            Style::Win11 => (vec2(46.0, 38.0), 0.0),
            _ => (Vec2::splat(34.0), 4.0),
        }
    }
}

/// The buttons' look and place, as set.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub style: Style,
    pub order: [WinButton; 3],
    /// At the left of the top bar (else the right).
    pub left: bool,
}

impl Look {
    pub fn of(cfg: &crate::config::Config) -> Self {
        Look { style: cfg.win_style, order: ORDERS[cfg.win_order as usize % ORDERS.len()], left: cfg.win_left }
    }
}

/// Room the three buttons take on the top bar (with a little margin).
pub fn buttons_width(style: Style) -> f32 {
    group_width(style, 1.0) + 10.0
}

fn group_width(style: Style, scale: f32) -> f32 {
    let (size, gap) = style.metrics();
    (3.0 * size.x + 2.0 * gap) * scale
}

/// The three buttons from `x` (their left edge) on the row's middle `cy`;
/// the one clicked.
pub fn buttons(ui: &mut Ui, icons: &mut Icons, look: Look, x: f32, cy: f32) -> Option<WinButton> {
    let maximized = ui.input(|i| i.viewport().maximized).unwrap_or(false);
    let rects = layout(look.style, &look.order, x, cy, 1.0);
    let resp: Vec<egui::Response> = rects.iter().map(|(b, r)| ui.interact(*r, ui.id().with(("win-button", *b as u8)), Sense::click())).collect();
    // (macOS shows its glyphs while the pointer is over any of them.)
    let over_group = resp.iter().any(|r| r.hovered());
    let mut hit = None;
    for ((b, rect), r) in rects.into_iter().zip(resp) {
        let hot = r.hovered() || r.is_pointer_button_down_on();
        paint(ui, icons, look.style, b, rect, maximized, hot, over_group, 1.0);
        let tip = match b {
            WinButton::Minimize => tr("Minimize"),
            WinButton::Maximize if maximized => tr("Restore"),
            WinButton::Maximize => tr("Maximize"),
            WinButton::Close => tr("Close"),
        };
        if r.on_hover_text(tip).clicked() {
            hit = Some(b);
        }
    }
    hit
}

/// The buttons as they'd look, not to be pressed (the settings' preview),
/// centred in `rect`.
pub fn preview(ui: &Ui, icons: &mut Icons, style: Style, order: &[WinButton; 3], rect: Rect, scale: f32) {
    let x = rect.center().x - group_width(style, scale) / 2.0;
    for (b, r) in layout(style, order, x, rect.center().y, scale) {
        paint(ui, icons, style, b, r, false, false, false, scale);
    }
}

fn layout(style: Style, order: &[WinButton; 3], x: f32, cy: f32, scale: f32) -> Vec<(WinButton, Rect)> {
    let (size, gap) = style.metrics();
    let (size, gap) = (size * scale, gap * scale);
    order.iter().enumerate().map(|(i, b)| (*b, Rect::from_min_size(pos2(x + i as f32 * (size.x + gap), cy - size.y / 2.0), size))).collect()
}

#[allow(clippy::too_many_arguments)]
fn paint(ui: &Ui, icons: &mut Icons, style: Style, b: WinButton, rect: Rect, maximized: bool, hot: bool, over_group: bool, scale: f32) {
    let v = ui.visuals();
    let dark = v.dark_mode;
    let text = v.text_color();
    let p = ui.painter();
    let c = rect.center();
    let close_red = Color32::from_rgb(0xda, 0x44, 0x53);
    let soft = |strong: bool| match (dark, strong) {
        (true, false) => Color32::from_white_alpha(26),
        (true, true) => Color32::from_white_alpha(46),
        (false, false) => Color32::from_black_alpha(20),
        (false, true) => Color32::from_black_alpha(36),
    };
    // The glyph's colour, and a theme to take an icon from (else drawn).
    let (fg, theme, pt): (Color32, Option<String>, f32) = match style.resolve() {
        Style::Breeze => {
            let red = b == WinButton::Close && hot;
            if red {
                p.circle_filled(c, 11.0 * scale, close_red);
            } else if hot {
                p.circle_filled(c, 11.0 * scale, soft(false));
            }
            (if red { Color32::WHITE } else { text }, Some(if dark { "breeze-dark" } else { "breeze" }.into()), 18.0)
        }
        Style::Adwaita => {
            p.circle_filled(c, 12.0 * scale, soft(hot));
            (text, Some("Adwaita".into()), 16.0)
        }
        Style::Yaru => {
            if b == WinButton::Close {
                let orange = Color32::from_rgb(0xe9, 0x54, 0x20);
                p.circle_filled(c, 11.0 * scale, if hot { orange.gamma_multiply(1.15) } else { orange });
                (Color32::WHITE, None, 14.0)
            } else {
                if hot {
                    p.circle_filled(c, 11.0 * scale, soft(false));
                }
                (text, None, 14.0)
            }
        }
        Style::Macos => {
            let fill = match b {
                WinButton::Close => Color32::from_rgb(0xff, 0x5f, 0x57),
                WinButton::Minimize => Color32::from_rgb(0xfe, 0xbc, 0x2e),
                WinButton::Maximize => Color32::from_rgb(0x28, 0xc8, 0x40),
            };
            p.circle_filled(c, 6.5 * scale, fill);
            p.circle_stroke(c, 6.5 * scale, Stroke::new(0.6, Color32::from_black_alpha(40)));
            if over_group {
                mac_glyph(ui, b, c, scale);
            }
            return;
        }
        Style::Win11 => {
            let red = b == WinButton::Close && hot;
            if red {
                p.rect_filled(rect, 0.0, Color32::from_rgb(0xc4, 0x2b, 0x1c));
            } else if hot {
                p.rect_filled(rect, 0.0, soft(false));
            }
            glyph(ui, b, maximized, c, if red { Color32::WHITE } else { text }, 1.0, 10.0 * scale);
            return;
        }
        Style::Icons => {
            if b == WinButton::Close && hot {
                p.rect_filled(rect.shrink(2.0 * scale), 8.0 * scale, close_red);
            } else if hot {
                p.rect_filled(rect.shrink(2.0 * scale), 8.0 * scale, v.widgets.hovered.weak_bg_fill);
            }
            (if b == WinButton::Close && hot { Color32::WHITE } else { text }, Some(String::new()), 18.0)
        }
        Style::Plain | Style::Auto => {
            if hot {
                p.circle_filled(c, 13.0 * scale, soft(false));
            }
            glyph(ui, b, maximized, c, text, 1.3, 10.0 * scale);
            return;
        }
    };
    let names: &[&str] = match b {
        WinButton::Minimize => &["window-minimize"],
        WinButton::Maximize if maximized => &["window-restore"],
        WinButton::Maximize => &["window-maximize"],
        WinButton::Close => &["window-close"],
    };
    let pt = pt * scale;
    let px = ((pt * ui.pixels_per_point()).ceil() as u16).max(16);
    let found = match theme.as_deref() {
        Some("") => icons.ui_icon(names, px, fg),
        Some(t) => icons.ui_icon_in(t, names, px, fg),
        None => None,
    };
    match found {
        Some(u) => egui::Image::new(u).paint_at(ui, Rect::from_center_size(c, Vec2::splat(pt))),
        // Breeze's own are chevrons.
        None if style.resolve() == Style::Breeze => breeze_glyph(ui, b, maximized, c, fg, scale),
        None => glyph(ui, b, maximized, c, fg, 1.5, 10.0 * scale),
    }
}

/// Plain shapes: a line, a square (two when maximized), a cross, `s` wide.
fn glyph(ui: &Ui, b: WinButton, maximized: bool, c: egui::Pos2, color: Color32, width: f32, s: f32) {
    let st = Stroke::new(width, color);
    let p = ui.painter();
    let h = s / 2.0;
    match b {
        WinButton::Minimize => {
            p.line_segment([c + vec2(-h, 0.0), c + vec2(h, 0.0)], st);
        }
        WinButton::Maximize if maximized => {
            let k = s * 0.2;
            p.rect_stroke(Rect::from_center_size(c + vec2(-k / 2.0, k / 2.0), Vec2::splat(s - k)), 1.0, st, egui::StrokeKind::Middle);
            p.line_segment([c + vec2(-h + k, -h), c + vec2(h, -h)], st);
            p.line_segment([c + vec2(h, -h), c + vec2(h, h - k)], st);
        }
        WinButton::Maximize => {
            p.rect_stroke(Rect::from_center_size(c, Vec2::splat(s)), 1.0, st, egui::StrokeKind::Middle);
        }
        WinButton::Close => {
            p.line_segment([c + vec2(-h, -h), c + vec2(h, h)], st);
            p.line_segment([c + vec2(h, -h), c + vec2(-h, h)], st);
        }
    }
}

/// KDE's: chevrons down and up, a diamond when maximized, a cross.
fn breeze_glyph(ui: &Ui, b: WinButton, maximized: bool, c: egui::Pos2, color: Color32, scale: f32) {
    let st = Stroke::new(1.5, color);
    let p = ui.painter();
    let s = 5.0 * scale;
    match b {
        WinButton::Minimize => {
            p.add(egui::Shape::line(vec![c + vec2(-s, -s / 2.0), c + vec2(0.0, s / 2.0), c + vec2(s, -s / 2.0)], st));
        }
        WinButton::Maximize if maximized => {
            p.add(egui::Shape::closed_line(vec![c + vec2(0.0, -s), c + vec2(s, 0.0), c + vec2(0.0, s), c + vec2(-s, 0.0)], st));
        }
        WinButton::Maximize => {
            p.add(egui::Shape::line(vec![c + vec2(-s, s / 2.0), c + vec2(0.0, -s / 2.0), c + vec2(s, s / 2.0)], st));
        }
        WinButton::Close => {
            p.line_segment([c + vec2(-s, -s), c + vec2(s, s)], st);
            p.line_segment([c + vec2(s, -s), c + vec2(-s, s)], st);
        }
    }
}

/// The traffic lights' marks: ×, −, and two corners for the zoom.
fn mac_glyph(ui: &Ui, b: WinButton, c: egui::Pos2, scale: f32) {
    let st = Stroke::new(1.2 * scale, Color32::from_black_alpha(150));
    let p = ui.painter();
    let s = 3.2 * scale;
    match b {
        WinButton::Close => {
            p.line_segment([c + vec2(-s, -s), c + vec2(s, s)], st);
            p.line_segment([c + vec2(s, -s), c + vec2(-s, s)], st);
        }
        WinButton::Minimize => {
            p.line_segment([c + vec2(-s - 0.5, 0.0), c + vec2(s + 0.5, 0.0)], st);
        }
        WinButton::Maximize => {
            let f = Color32::from_black_alpha(150);
            p.add(egui::Shape::convex_polygon(vec![c + vec2(-s, -s), c + vec2(s * 0.6, -s), c + vec2(-s, s * 0.6)], f, Stroke::NONE));
            p.add(egui::Shape::convex_polygon(vec![c + vec2(s, s), c + vec2(-s * 0.6, s), c + vec2(s, -s * 0.6)], f, Stroke::NONE));
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
