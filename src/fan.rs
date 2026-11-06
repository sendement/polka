//! The fan menu: the pad's stick (the left one, as set) pushed out shows
//! two circles at the window's left edge, the outer one still and the
//! inner one following the stick; out near the outer one's edge the
//! context menu spreads out to the right as a fan, the item the stick
//! points at lit in the accent colour, ✕ / A runs it. Back to the middle
//! (or let go) it's gone. The inner circle touching the outer one's left
//! edge switches to Polka's own menu (and back); a submenu opens as a
//! further fan, ○ / B back out of it.

use std::time::Instant;

use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2, pos2, vec2};

use crate::app::{App, Cmd, Focus, Menu, Overlay};
use crate::i18n::tr;

/// Pushed this far (of the way out), the circles show.
const SHOW: f32 = 0.22;
/// Half the angle, about straight left, where the inner circle touches
/// the switch to the other menu.
const FLIP_ZONE: f32 = 35.0;
/// The fan's items from this angle above straight right to as far below.
const SPREAD: f32 = 70.0;
/// Labels no closer together than this (unscaled), up and down; what
/// doesn't fit goes under "More…".
const ROW: f32 = 30.0;
/// How long levels and the switch slide in (seconds).
const SLIDE: f32 = 0.22;

/// One fan: a menu's items; the last one "More…" when they didn't fit.
pub struct Level {
    pub menu: Menu,
    more: Option<Menu>,
    /// The item gone into from here (a submenu, "More…").
    chosen: Option<usize>,
}

/// The fan's height either way from the middle, in a window this high.
fn radius_y(height: f32, s: f32) -> f32 {
    (height * 0.44).min(320.0 * s).max(56.0 * s + 60.0)
}

/// How many items fit in one fan.
fn most(theme: Theme, height: f32, s: f32) -> usize {
    let n = if theme.round() {
        // (Along the ring's middle, a row's height apart.)
        let mid = (56.0 * s + 10.0 * s + radius_y(height, s)) / 2.0;
        2.0 * ROUND_SPREAD.to_radians() * mid / (24.0 * s)
    } else {
        2.0 * radius_y(height, s) * SPREAD.to_radians().sin() / (ROW * s)
    };
    n.floor().clamp(5.0, 24.0) as usize
}

impl Level {
    fn new(mut menu: Menu, most: usize) -> Self {
        let more = (menu.items.len() > most).then(|| {
            let rest = menu.items.split_off(most - 1);
            Menu { kind: None, ..Menu::new(tr("More…"), rest) }
        });
        if more.is_some() {
            menu.items.push(crate::app::item(tr("More…"), Cmd::Nothing));
            // (Its toggles can't be rebuilt in place once split.)
            menu.kind = None;
        }
        Level { menu, more, chosen: None }
    }

    fn is_more(&self, i: usize) -> bool {
        self.more.is_some() && i + 1 == self.menu.items.len()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Slide {
    /// A level further in, or back out.
    In,
    Out,
    /// Over to the other menu (Polka's, or what's in hand).
    Flip,
}

#[derive(Default)]
pub struct Fan {
    /// The stick (x right, y up).
    pub pos: Vec2,
    pub shown: bool,
    /// Polka's own menu (else what's in hand's).
    pub global: bool,
    /// The fans open, the first one outermost-in.
    pub levels: Vec<Level>,
    /// The item pointed at, in the last level.
    pub hot: Option<usize>,
    /// Something ran: nothing more until the stick is back in the middle.
    pub spent: bool,
    /// In the switch's zone already (it switches once on the way in).
    flip_armed: bool,
    pub slide: Option<(Instant, Slide)>,
    /// Back in the middle since (it goes after a moment).
    centred: Option<Instant>,
}

/// How the items look.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    /// Pills on an arc, the one pointed at joined to the circle by a line.
    Lines,
    /// A ring cut in segments, the text along each; the one pointed at
    /// stands out a little, lit.
    Segments,
    /// Petals out from the circle, the text along each.
    Petals,
    /// Just the words on an arc, the one pointed at bigger, lit.
    Text,
}

impl Theme {
    pub fn of(n: u8) -> Theme {
        match n {
            1 => Theme::Segments,
            2 => Theme::Petals,
            3 => Theme::Text,
            _ => Theme::Lines,
        }
    }

    pub fn label(n: u8) -> &'static str {
        tr(["Lines", "Segments", "Petals", "Text"][n.min(3) as usize])
    }

    /// Round the circle (segments, petals): even angles, else even
    /// heights.
    fn round(self) -> bool {
        matches!(self, Theme::Segments | Theme::Petals)
    }
}

/// The round ones' items from this angle above straight right to as far
/// below.
const ROUND_SPREAD: f32 = 80.0;

/// Where the items of a fan of `n` sit: their angles (degrees, up
/// positive). On an arc: evenly apart up and down, so their labels don't
/// crowd; round: evenly apart.
pub fn angles(theme: Theme, n: usize) -> Vec<f32> {
    if theme.round() {
        let step = 2.0 * ROUND_SPREAD / n.max(1) as f32;
        return (0..n).map(|i| ROUND_SPREAD - (i as f32 + 0.5) * step).collect();
    }
    let top = SPREAD.to_radians().sin();
    (0..n)
        .map(|i| {
            let s = if n == 1 { 0.0 } else { top * (1.0 - 2.0 * (i as f32 + 0.5) / n as f32) };
            s.asin().to_degrees()
        })
        .collect()
}

/// The item a stick angle points at (the nearest).
pub fn pointed(theme: Theme, n: usize, angle: f32) -> Option<usize> {
    let a = angle.clamp(-90.0, 90.0);
    angles(theme, n).iter().enumerate().min_by(|x, y| (x.1 - a).abs().total_cmp(&(y.1 - a).abs())).map(|(i, _)| i)
}

impl App {
    fn fan_level(&self, menu: Menu) -> Level {
        Level::new(menu, most(Theme::of(self.cfg.fan.theme), self.ctx.content_rect().height(), self.cfg.fan.size))
    }

    /// Nothing else open the fan would cover or take keys from.
    fn fan_free(&self) -> bool {
        self.overlay.is_none() && self.term_window.is_none() && self.icon_pick.is_none() && self.drag.is_none()
    }

    /// The fan has the left stick now (else it moves the cursor as the
    /// d-pad does). Also after one of its items ran until the stick is
    /// back in the middle: what the item opened (settings, a dialog)
    /// isn't moved about by the stick still pushed.
    pub fn fan_has_left_stick(&self) -> bool {
        self.cfg.fan.on && self.cfg.fan.stick == 0 && (self.fan_free() || self.fan.spent)
    }

    /// Tells the pad's thread which stick to send.
    pub fn fan_sync_stick(&self) {
        let s = if self.cfg.fan.on { self.cfg.fan.stick + 1 } else { 0 };
        crate::input::FAN_STICK.store(s, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn fan_close(&mut self) {
        let f = &mut self.fan;
        f.shown = false;
        f.levels.clear();
        f.hot = None;
        f.global = false;
        f.flip_armed = false;
        f.slide = None;
        f.centred = None;
    }

    fn slide(&mut self, s: Slide) {
        self.fan.slide = self.cfg.fan.anim.then(|| (Instant::now(), s));
    }

    /// What the stick's menu holds: what's in hand's (the item, the page,
    /// the place), or Polka's own; nothing if there's no menu.
    fn fan_menu(&mut self, global: bool) -> Option<Menu> {
        let before = self.overlay.take();
        if global {
            self.open_app_menu();
        } else if self.focus == Focus::Sidebar {
            match self.side_items.get(self.side_cursor).cloned().filter(|it| !it.usage && !it.torrents) {
                Some(it) => self.side_menu(it),
                None => self.open_app_menu(),
            }
        } else if self.term_shown() {
            self.terminal_menu();
        } else if self.torrents_shown() {
            self.torrents_menu();
        } else {
            self.open_actions_menu();
        }
        // (Only a menu: anything else it opened is let go.)
        let got = match self.overlay.take() {
            Some(Overlay::Menu(m)) => Some(m),
            _ => None,
        };
        self.overlay = before;
        got.filter(|m| !m.items.is_empty())
    }

    fn fan_root(&mut self) {
        let global = self.fan.global;
        let menu = self.fan_menu(global).or_else(|| if global { None } else { self.fan_menu(true) });
        self.fan.levels = menu.map(|m| self.fan_level(m)).into_iter().collect();
        self.fan.hot = None;
    }

    /// The stick moved.
    pub fn fan_stick(&mut self, x: f32, y: f32) {
        // (Back in the middle after an item ran: the stick's free again,
        // whatever that item opened.)
        if x.hypot(y) < SHOW {
            self.fan.spent = false;
        }
        if !self.cfg.fan.on || !self.fan_free() {
            self.fan_close();
            return;
        }
        self.fan.pos = vec2(x, y);
        let mag = x.hypot(y).min(1.0);
        if mag < SHOW {
            self.fan.spent = false;
            // A moment before it goes: the stick may be on its way over.
            if self.fan.shown && self.cfg.fan.linger > 0.0 {
                self.fan.hot = None;
                self.fan.flip_armed = false;
                self.fan.centred.get_or_insert_with(Instant::now);
            } else {
                self.fan_close();
            }
            return;
        }
        self.fan.centred = None;
        if self.fan.spent {
            return;
        }
        if !self.fan.shown {
            self.fan.shown = true;
            self.fan.global = false;
            self.fan_root();
        }
        if mag < self.cfg.fan.reach {
            self.fan.hot = None;
            self.fan.flip_armed = false;
            return;
        }
        let angle = y.atan2(x).to_degrees();
        // At the left edge: the other menu (once on the way in).
        if angle.abs() > 180.0 - FLIP_ZONE {
            if !self.fan.flip_armed {
                self.fan.flip_armed = true;
                self.fan.global = !self.fan.global;
                self.fan_root();
                self.slide(Slide::Flip);
            }
            self.fan.hot = None;
            return;
        }
        self.fan.flip_armed = false;
        let theme = Theme::of(self.cfg.fan.theme);
        self.fan.hot = self.fan.levels.last().and_then(|l| pointed(theme, l.menu.items.len(), angle));
    }

    /// ✕ / A while the fan shows: the item pointed at (true: taken).
    pub fn fan_accept(&mut self) -> bool {
        if !self.fan.shown {
            return false;
        }
        let Some(i) = self.fan.hot else { return true };
        let Some(level) = self.fan.levels.last() else { return true };
        if level.is_more(i) {
            let more = level.more.as_ref().map(|m| Menu { kind: None, ..Menu::new(&m.title, m.items.clone()) });
            if let Some(m) = more {
                if let Some(cur) = self.fan.levels.last_mut() {
                    cur.chosen = Some(i);
                }
                let l = self.fan_level(m);
                self.fan.levels.push(l);
                self.fan.hot = None;
                self.slide(Slide::In);
            }
            return true;
        }
        let Some(it) = level.menu.items.get(i).filter(|it| it.enabled).cloned() else { return true };
        let kind = level.menu.kind;
        match it.cmd {
            Cmd::Submenu(k) => {
                let sub = self.build_submenu(k);
                if let Some(cur) = self.fan.levels.last_mut() {
                    cur.chosen = Some(i);
                }
                let l = self.fan_level(sub);
                self.fan.levels.push(l);
                self.fan.hot = None;
                self.slide(Slide::In);
            }
            cmd if it.keep_open => {
                self.run(cmd);
                // Rebuilt in place, so the ticks follow.
                let fresh = match kind {
                    Some(k) => Some(self.build_submenu(k)),
                    None if self.fan.levels.len() == 1 => self.fan_menu(self.fan.global),
                    None => None,
                };
                if let (Some(m), Some(l)) = (fresh, self.fan.levels.last_mut()) {
                    if m.items.len() == l.menu.items.len() {
                        l.menu.items = m.items;
                    }
                }
            }
            cmd => {
                self.fan_close();
                self.fan.spent = true;
                self.run(cmd);
            }
        }
        true
    }

    /// ○ / B while the fan shows: out of a submenu, else it's put away
    /// (till the stick is back in the middle). True: taken.
    pub fn fan_back(&mut self) -> bool {
        if !self.fan.shown {
            return false;
        }
        if self.fan.levels.len() > 1 {
            self.fan.levels.pop();
            self.fan.hot = None;
            if let Some(l) = self.fan.levels.last_mut() {
                l.chosen = None;
            }
            self.slide(Slide::Out);
        } else {
            self.fan_close();
            self.fan.spent = true;
        }
        true
    }

    /// Drawn over everything, at the window's left edge, its middle.
    pub fn draw_fan(&mut self, ctx: &egui::Context) {
        if !self.fan.shown {
            return;
        }
        // Back in the middle a while: gone.
        if let Some(t) = self.fan.centred {
            let left = self.cfg.fan.linger - t.elapsed().as_secs_f32();
            if left <= 0.0 {
                self.fan_close();
                return;
            }
            ctx.request_repaint_after(std::time::Duration::from_secs_f32(left));
        }
        let s = self.cfg.fan.size;
        let reach = self.cfg.fan.reach;
        let theme = Theme::of(self.cfg.fan.theme);
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("fan")));
        let area = ctx.content_rect();
        let v = ctx.style_of(ctx.theme()).visuals.clone();
        let accent = v.selection.bg_fill;
        let on_accent = v.selection.stroke.color;
        let outer = 56.0 * s;
        let knob = 20.0 * s;
        let anchor = pos2(area.min.x + 22.0 + outer, area.center().y);
        let (x, y) = (self.fan.pos.x, self.fan.pos.y);
        let mag = if self.fan.centred.is_some() { 0.0 } else { x.hypot(y).min(1.0) };
        let dir = if mag > 0.0 { vec2(x, -y) / x.hypot(y) } else { Vec2::ZERO };
        let at = anchor + dir * mag * (outer - knob);
        // How far into the slide (eased), 1 when none.
        let t = self.fan.slide.map_or(1.0, |(since, _)| (since.elapsed().as_secs_f32() / SLIDE).min(1.0));
        if t < 1.0 {
            ctx.request_repaint();
        } else {
            self.fan.slide = None;
        }
        let ease = 1.0 - (1.0 - t).powi(3);
        let slide = self.fan.slide.map(|s| s.1);
        // The items: shown as the stick nears the edge.
        let show = ((mag - reach * 0.6) / (reach * 0.4)).clamp(0.0, 1.0);
        // The window behind dimmed (more with the items out).
        painter.rect_filled(area, 0.0, Color32::from_black_alpha((70.0 + 80.0 * show) as u8));
        // The circles.
        let bg = v.extreme_bg_color.gamma_multiply(0.9);
        painter.circle(anchor, outer, bg, Stroke::new(2.0 * s, accent));
        // The switch: the outer circle's left edge, lit while touched.
        let in_zone = mag >= reach && y.atan2(x).to_degrees().abs() > 180.0 - FLIP_ZONE;
        let zone: Vec<Pos2> = (0..=12)
            .map(|k| {
                let a = (180.0 - FLIP_ZONE + 2.0 * FLIP_ZONE * k as f32 / 12.0).to_radians();
                anchor + vec2(a.cos(), -a.sin()) * outer
            })
            .collect();
        painter.add(egui::Shape::line(zone, Stroke::new(if in_zone { 6.0 } else { 4.0 } * s, if in_zone { accent } else { accent.gamma_multiply(0.45) })));
        painter.circle(at, knob, accent, Stroke::new(2.0, on_accent.gamma_multiply(0.6)));
        // Which menu, above the circles; the other one's name by the switch.
        let title = self.fan.levels.iter().map(|l| l.menu.title.as_str()).collect::<Vec<_>>().join(" › ");
        let font = FontId::proportional(14.0 * s.max(0.85));
        painter.text(anchor - vec2(outer, outer + 10.0 * s), Align2::LEFT_BOTTOM, &title, font, v.strong_text_color());
        let other = if self.fan.global { tr("Item menu") } else { tr("Polka menu") };
        painter.text(anchor + vec2(-outer, outer + 10.0 * s), Align2::LEFT_TOP, format!("◀ {other}"), FontId::proportional(12.0 * s.max(0.85)), v.weak_text_color());
        if show <= 0.0 || self.fan.levels.is_empty() {
            return;
        }
        let geo = Geo { anchor, outer, s, ry: radius_y(area.height(), s), font: 15.0 * s.max(0.85) };
        let gap = 190.0 * s;
        let depth = self.fan.levels.len() - 1;
        if !theme.round() {
            // A faint arc: the fan's span.
            let span: Vec<Pos2> = (0..=24)
                .map(|k| {
                    let a = (SPREAD - 2.0 * SPREAD * k as f32 / 24.0).to_radians();
                    anchor + vec2(a.cos(), -a.sin()) * (outer + 12.0 * s)
                })
                .collect();
            painter.add(egui::Shape::line(span, Stroke::new(2.0 * s, accent.gamma_multiply(0.3 * show))));
        }
        for (d, level) in self.fan.levels.iter().enumerate() {
            let last = d == depth;
            // Round ones: only the level in hand (the way there above).
            if theme.round() && !last {
                continue;
            }
            // On an arc, the level just opened slides out from the one
            // before it, which dims (and lights again coming back); round,
            // it grows out of the circle.
            let (rx, alpha, grow) = match (slide, last) {
                (Some(Slide::In), true) if theme.round() => (geo.ry, ease, 0.55 + 0.45 * ease),
                (Some(Slide::In), true) => (geo.ry + (d as f32 - 1.0 + ease) * gap, ease, 1.0),
                (Some(Slide::In), false) if d + 1 == depth => (geo.ry + d as f32 * gap, 1.0 - 0.65 * ease, 1.0),
                (Some(Slide::Out), true) => (geo.ry + d as f32 * gap, 0.35 + 0.65 * ease, if theme.round() { 0.8 + 0.2 * ease } else { 1.0 }),
                (_, true) => (geo.ry + d as f32 * gap, 1.0, 1.0),
                (_, false) => (geo.ry + d as f32 * gap, 0.35, 1.0),
            };
            let flip = slide == Some(Slide::Flip);
            let look = Look {
                rx,
                alpha: alpha * show * if flip { ease } else { 1.0 },
                // Switching menus: the items swing in from above.
                swing: if flip { (1.0 - ease) * 40.0 } else { 0.0 },
                hot: if last { self.fan.hot } else { None },
                chosen: level.chosen,
                // Levels before: only the item gone into (the rest fade
                // out as the next level comes).
                others: match (slide, d + 1 == depth) {
                    _ if last => 1.0,
                    (Some(Slide::In), true) => 1.0 - ease,
                    _ => 0.0,
                },
                last,
                grow,
            };
            let items: Vec<Shown> = (0..level.menu.items.len())
                .map(|i| {
                    let it = &level.menu.items[i];
                    let mut label = it.label.clone();
                    if matches!(it.cmd, Cmd::Submenu(_)) || level.is_more(i) {
                        label += "  ›";
                    }
                    if it.checked == Some(true) {
                        label = format!("✔ {label}");
                    }
                    Shown { label, enabled: it.enabled }
                })
                .collect();
            paint_level(&painter, theme, &geo, &look, &items, &v);
        }
    }
}

/// Where a fan is: its circles' middle, the outer one's radius, the size,
/// how far up and down (and out) the items go.
pub struct Geo {
    pub anchor: Pos2,
    pub outer: f32,
    pub s: f32,
    pub ry: f32,
    /// The items' text size.
    pub font: f32,
}

/// How one level shows just now.
pub struct Look {
    /// How far out (on an arc).
    pub rx: f32,
    pub alpha: f32,
    /// Turned by (degrees, switching menus).
    pub swing: f32,
    pub hot: Option<usize>,
    pub chosen: Option<usize>,
    /// The items other than the one gone into, before the last level.
    pub others: f32,
    pub last: bool,
    /// Round ones: how far grown out (0..1).
    pub grow: f32,
}

pub struct Shown {
    pub label: String,
    pub enabled: bool,
}

/// `text` as wide as `max` at most (cut short with "…").
fn fit(p: &egui::Painter, text: &str, font: &FontId, color: Color32, max: f32) -> std::sync::Arc<egui::Galley> {
    let g = p.layout_no_wrap(text.to_string(), font.clone(), color);
    if g.size().x <= max {
        return g;
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.len() > 1 {
        chars.pop();
        let s: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
        let g = p.layout_no_wrap(s, font.clone(), color);
        if g.size().x <= max {
            return g;
        }
    }
    g
}

/// Text along a ray at `angle` (degrees, up positive), from `from` out
/// (or centred in `len` from there).
#[allow(clippy::too_many_arguments)]
fn along(p: &egui::Painter, anchor: Pos2, angle: f32, from: f32, len: f32, text: &str, font: &FontId, color: Color32, centre: bool) {
    let g = fit(p, text, font, color, len.max(10.0));
    let a = angle.to_radians();
    let u = vec2(a.cos(), -a.sin());
    // (The text's down: a quarter turn on from along.)
    let n = vec2(-u.y, u.x);
    let start = if centre { from + (len - g.size().x).max(0.0) / 2.0 } else { from };
    let pos = anchor + u * start - n * (g.size().y / 2.0);
    p.add(egui::epaint::TextShape::new(pos, g, color).with_angle(-a));
}

/// A ring's piece: from `r0` to `r1`, between two angles (degrees).
#[allow(clippy::too_many_arguments)]
fn sector(p: &egui::Painter, c: Pos2, r0: f32, r1: f32, a0: f32, a1: f32, fill: Color32, stroke: Stroke) {
    let steps = ((a1 - a0).abs() / 3.0).ceil().max(2.0) as usize;
    let at = |r: f32, k: usize| {
        let a = (a0 + (a1 - a0) * k as f32 / steps as f32).to_radians();
        c + vec2(a.cos(), -a.sin()) * r
    };
    let mut mesh = egui::Mesh::default();
    for k in 0..=steps {
        mesh.colored_vertex(at(r0, k), fill);
        mesh.colored_vertex(at(r1, k), fill);
    }
    for k in 0..steps as u32 {
        let i = 2 * k;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
    p.add(mesh);
    let mut edge: Vec<Pos2> = (0..=steps).map(|k| at(r0, k)).collect();
    edge.extend((0..=steps).rev().map(|k| at(r1, k)));
    p.add(egui::Shape::closed_line(edge, stroke));
}

/// One level's items, the way the theme draws them.
pub fn paint_level(p: &egui::Painter, theme: Theme, g: &Geo, l: &Look, items: &[Shown], v: &egui::Visuals) {
    let s = g.s;
    let accent = v.selection.bg_fill;
    let on_accent = v.selection.stroke.color;
    let border = v.widgets.noninteractive.bg_stroke.color;
    let n = items.len();
    let step = 2.0 * ROUND_SPREAD / n.max(1) as f32;
    let font = FontId::proportional(g.font);
    for (i, a) in angles(theme, n).into_iter().enumerate() {
        let it = &items[i];
        let alpha = if l.last || l.chosen == Some(i) { l.alpha } else { l.alpha * l.others };
        if alpha <= 0.01 {
            continue;
        }
        let a = a + l.swing;
        let lit = l.hot == Some(i) || (!l.last && l.chosen == Some(i));
        let color = if lit {
            on_accent
        } else if it.enabled {
            v.text_color()
        } else {
            v.weak_text_color()
        }
        .gamma_multiply(alpha);
        let rad = a.to_radians();
        let u = vec2(rad.cos(), -rad.sin());
        let spoke = |p: &egui::Painter, to: Pos2| {
            let from = g.anchor + u * g.outer;
            p.line_segment([from, to], Stroke::new(2.5 * s, accent.gamma_multiply(alpha)));
            p.circle_filled(from, 4.0 * s, accent.gamma_multiply(alpha));
        };
        match theme {
            Theme::Lines => {
                let pos = g.anchor + vec2(rad.cos() * l.rx, -rad.sin() * g.ry);
                let galley = p.layout_no_wrap(it.label.clone(), font.clone(), color);
                let pill = Rect::from_min_size(pos2(pos.x, pos.y - galley.size().y / 2.0), galley.size()).expand2(vec2(10.0, 5.0) * s);
                let fill = if lit { accent } else { v.extreme_bg_color.gamma_multiply(0.92) };
                let stroke = if lit { Stroke::NONE } else { Stroke::new(1.0, border.gamma_multiply(alpha)) };
                p.rect(pill, pill.height() / 2.0, fill.gamma_multiply(alpha), stroke, egui::StrokeKind::Inside);
                p.galley(pos2(pill.min.x + 10.0 * s, pill.center().y - galley.size().y / 2.0), galley, color);
                if lit && l.last {
                    spoke(p, pos2(pill.min.x, pill.center().y));
                }
            }
            Theme::Text => {
                let pos = g.anchor + vec2(rad.cos() * l.rx, -rad.sin() * g.ry);
                let (f, c) = if lit { (FontId::proportional(g.font * 1.27), accent.gamma_multiply(alpha)) } else { (font.clone(), color) };
                let galley = p.layout_no_wrap(it.label.clone(), f, c);
                p.galley(pos2(pos.x, pos.y - galley.size().y / 2.0), galley, c);
                if lit && l.last {
                    spoke(p, pos2(pos.x - 8.0 * s, pos.y));
                }
            }
            Theme::Segments => {
                let r0 = g.outer + 10.0 * s;
                let r1 = r0 + (g.ry - r0).max(60.0 * s) * l.grow;
                let (r0, r1) = if lit { (r0 - 4.0 * s, r1 + 14.0 * s) } else { (r0, r1) };
                let fill = if lit { accent } else { v.extreme_bg_color.gamma_multiply(0.92) };
                let line = if lit { on_accent.gamma_multiply(0.5) } else { border };
                sector(p, g.anchor, r0, r1, a - step / 2.0, a + step / 2.0, fill.gamma_multiply(alpha), Stroke::new(1.2, line.gamma_multiply(alpha)));
                along(p, g.anchor, a, r0 + 12.0 * s, r1 - r0 - 22.0 * s, &it.label, &font, color, false);
            }
            Theme::Petals => {
                let r0 = g.outer + 6.0 * s;
                let r1 = r0 + (g.ry * 0.92 - r0).max(60.0 * s) * l.grow + if lit { 16.0 * s } else { 0.0 };
                let mid = r0 + 0.55 * (r1 - r0);
                let w = (mid * (step.to_radians() / 2.0).sin() * 0.92).clamp(6.0 * s, 30.0 * s);
                let nrm = vec2(-u.y, u.x);
                let mut pts: Vec<Pos2> = (0..=16)
                    .map(|k| {
                        let t = k as f32 / 16.0;
                        g.anchor + u * (r0 + t * (r1 - r0)) + nrm * (w * (std::f32::consts::PI * t).sin())
                    })
                    .collect();
                pts.extend((1..16).rev().map(|k| {
                    let t = k as f32 / 16.0;
                    g.anchor + u * (r0 + t * (r1 - r0)) - nrm * (w * (std::f32::consts::PI * t).sin())
                }));
                let fill = if lit { accent } else { v.extreme_bg_color.gamma_multiply(0.92) };
                let line = if lit { on_accent.gamma_multiply(0.5) } else { border };
                p.add(egui::Shape::convex_polygon(pts, fill.gamma_multiply(alpha), Stroke::new(1.2, line.gamma_multiply(alpha))));
                let f = FontId::proportional(g.font.min(w * 1.25).max(9.0));
                along(p, g.anchor, a, r0 + (r1 - r0) * 0.12, (r1 - r0) * 0.76, &it.label, &f, color, true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fan_points_at_the_nearest() {
        let a = angles(Theme::Lines, 5);
        assert!(a[0] > 40.0 && a[4] < -40.0 && a[2].abs() < 0.01, "{a:?}");
        assert!(most(Theme::Lines, 680.0, 1.0) >= 17 && most(Theme::Lines, 400.0, 1.0) < most(Theme::Lines, 680.0, 1.0));
        assert!(a.windows(2).all(|w| w[0] > w[1]));
        assert_eq!(pointed(Theme::Lines, 5, 0.0), Some(2));
        assert_eq!(pointed(Theme::Lines, 5, 89.0), Some(0));
        assert_eq!(pointed(Theme::Lines, 5, -170.0), Some(4));
        assert_eq!(pointed(Theme::Lines, 1, 40.0), Some(0));
        assert_eq!(pointed(Theme::Lines, 0, 0.0), None);
        // Round: even slices of ±80°, each pointed at within its own.
        let r = angles(Theme::Segments, 4);
        assert_eq!(r, vec![60.0, 20.0, -20.0, -60.0]);
        assert_eq!(pointed(Theme::Segments, 4, 39.0), Some(1));
        assert_eq!(pointed(Theme::Segments, 4, 41.0), Some(0));
    }
}

/// How the fan looks, as set (in the settings): its circles, the stick
/// pushed up and right, a few items, one pointed at.
pub fn draw_preview(painter: &egui::Painter, rect: Rect, prefs: &crate::config::FanPrefs, v: &egui::Visuals) {
    let s = prefs.size.min(1.1);
    let accent = v.selection.bg_fill;
    let fade = if prefs.on { 1.0 } else { 0.4 };
    painter.rect_filled(rect, 10.0, v.faint_bg_color);
    let painter = painter.with_clip_rect(rect);
    let outer = 40.0 * s;
    let knob = 15.0 * s;
    let anchor = pos2(rect.min.x + 18.0 + outer, rect.center().y);
    painter.circle(anchor, outer, v.extreme_bg_color, Stroke::new(2.0 * s, accent.gamma_multiply(fade)));
    let theme = Theme::of(prefs.theme);
    let labels = [tr("Open"), tr("Copy"), tr("Rename"), tr("Move to trash"), tr("Properties")];
    let hot = 1;
    let a = angles(theme, labels.len())[hot].to_radians();
    painter.circle_filled(anchor + vec2(a.cos(), -a.sin()) * (outer - knob), knob, accent.gamma_multiply(fade));
    let geo = Geo { anchor, outer, s, ry: (rect.height() / 2.0 - 10.0).min(170.0 * s), font: 12.0 };
    let look = Look { rx: geo.ry * 1.25, alpha: fade, swing: 0.0, hot: Some(hot), chosen: None, others: 1.0, last: true, grow: 1.0 };
    let items: Vec<Shown> = labels.iter().map(|l| Shown { label: l.to_string(), enabled: true }).collect();
    paint_level(&painter, theme, &geo, &look, &items, v);
}
