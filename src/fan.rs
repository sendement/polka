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
    /// The submenu it's (part of), to rebuild after a toggle.
    src: Option<crate::app::SubMenu>,
}

/// The fan's height either way from the middle, in a window this high.
fn radius_y(height: f32, s: f32) -> f32 {
    (height * 0.44).min(320.0 * s).max(56.0 * s + 60.0)
}

/// How many items fit in one fan.
fn most(theme: Theme, height: f32, s: f32, stretch: f32) -> usize {
    let n = if theme.round() {
        // (Along the ring's middle, a row's height apart; petals need
        // room either side of their text, and their gaps.)
        let mid = (56.0 * s + 10.0 * s + radius_y(height, s)) / 2.0 * (1.0 + stretch.max(1.0)) / 2.0;
        let row = if matches!(theme, Theme::Petals | Theme::Daisy) { PETAL_ROW } else { ROUND_ROW };
        2.0 * ROUND_SPREAD.to_radians() * mid / (row * s)
    } else {
        2.0 * radius_y(height, s) * SPREAD.to_radians().sin() / (ROW * s)
    };
    n.floor().clamp(5.0, 24.0) as usize
}

impl Level {
    fn new(mut menu: Menu, most: usize) -> Self {
        let src = menu.kind;
        let more = (menu.items.len() > most).then(|| {
            let rest = menu.items.split_off(most - 1);
            Menu { kind: None, ..Menu::new(tr("More…"), rest) }
        });
        if more.is_some() {
            menu.items.push(crate::app::item(tr("More…"), Cmd::Nothing));
            // (Its toggles can't be rebuilt in place once split.)
            menu.kind = None;
        }
        Level { menu, more, chosen: None, src }
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
    /// A daisy's: narrow at the circle, round at their ends.
    Daisy,
    /// Segments with short soft edges, shaded, the one pointed at
    /// glowing in the accent colour.
    Glow,
    /// Just the words on an arc, the one pointed at bigger, lit.
    Text,
}

impl Theme {
    pub fn of(n: u8) -> Theme {
        match n {
            1 => Theme::Segments,
            2 => Theme::Petals,
            4 => Theme::Daisy,
            5 => Theme::Glow,
            3 => Theme::Text,
            _ => Theme::Lines,
        }
    }

    pub fn label(n: u8) -> &'static str {
        tr(["Lines", "Segments", "Petals", "Text", "Daisy", "Glow"][n.min(5) as usize])
    }

    /// Round the circle (segments, petals): even angles, else even
    /// heights.
    fn round(self) -> bool {
        matches!(self, Theme::Segments | Theme::Petals | Theme::Daisy | Theme::Glow)
    }
}

/// The round ones' items from this angle above straight right to as far
/// below.
const ROUND_SPREAD: f32 = 80.0;
/// Round ones: items no closer than this along the ring's middle; their
/// text this much bigger than on an arc.
const ROUND_ROW: f32 = 34.0;
const PETAL_ROW: f32 = 44.0;
const ROUND_TEXT: f32 = 1.12;

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
        Level::new(menu, most(Theme::of(self.cfg.fan.theme), self.ctx.content_rect().height(), self.cfg.fan.size, self.cfg.fan.length))
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
                let src = level.src;
                if let Some(cur) = self.fan.levels.last_mut() {
                    cur.chosen = Some(i);
                }
                let mut l = self.fan_level(m);
                l.src = src;
                self.fan.levels.push(l);
                self.fan.hot = None;
                self.slide(Slide::In);
            }
            return true;
        }
        let Some(it) = level.menu.items.get(i).filter(|it| it.enabled).cloned() else { return true };
        let src = level.src;
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
                // Its menu again, so the ticks follow (its items matched
                // by what they do: it may be split under "More…").
                let fresh = match src {
                    Some(k) => Some(self.build_submenu(k)),
                    None => self.fan_menu(self.fan.global),
                };
                if let (Some(m), Some(l)) = (fresh, self.fan.levels.last_mut()) {
                    for it in &mut l.menu.items {
                        let what = format!("{:?}", it.cmd);
                        if let Some(f) = m.items.iter().find(|f| format!("{:?}", f.cmd) == what) {
                            it.checked = f.checked;
                            it.enabled = f.enabled;
                            it.label = f.label.clone();
                        }
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
        let geo = Geo { anchor, outer, s, ry: radius_y(area.height(), s), font: 15.0 * s.max(0.85), stretch: self.cfg.fan.length };
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
    /// Round ones: how much further out they reach straight right than
    /// up and down (an ellipse; 1 a circle).
    pub stretch: f32,
}

impl Geo {
    /// Round ones: how far out (from the middle) at `angle` (degrees),
    /// for items `len` long up and down from `r0` out.
    fn reach(&self, r0: f32, len: f32, angle: f32) -> f32 {
        let (k, a) = (self.stretch.max(1.0), angle.to_radians());
        (r0 + len) * k / (a.cos().powi(2) + (k * a.sin()).powi(2)).sqrt()
    }
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
    p.layout_no_wrap("…".into(), font.clone(), color)
}

/// Where text goes along a petal half as wide as `hw(radius)` from `r0`
/// to `r1`: its size (at most `most`, smaller in a narrow one) and the
/// stretch where the petal's wide enough for it with `pad` above and
/// below.
fn text_run(hw: impl Fn(f32) -> f32, r0: f32, r1: f32, most: f32, pad: f32) -> (f32, f32, f32) {
    const N: usize = 48;
    let at = |k: usize| r0 + (r1 - r0) * k as f32 / N as f32;
    let widest = (0..=N).map(|k| hw(at(k))).fold(0.0, f32::max);
    let size = most.min((widest * 0.9 - pad) / 0.65).max(9.0);
    let need = size * 0.65 + pad;
    // The longest stretch that wide.
    let (mut best, mut start) = ((r0, r0), None);
    for k in 0..=N {
        match (hw(at(k)) >= need, start) {
            (true, None) => start = Some(at(k)),
            (false, Some(a)) => {
                if at(k) - a > best.1 - best.0 {
                    best = (a, at(k));
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(a) = start {
        if r1 - a > best.1 - best.0 {
            best = (a, r1);
        }
    }
    (size, best.0, best.1)
}

/// A shape along `u` from `r0` to `r1` out of `c`, half as wide as
/// `hw(radius)` either side (filled strip by strip: it needn't be convex).
#[allow(clippy::too_many_arguments)]
fn petal(p: &egui::Painter, c: Pos2, u: Vec2, r0: f32, r1: f32, hw: impl Fn(f32) -> f32, fill: Color32, stroke: Stroke) {
    const N: usize = 24;
    let n = vec2(-u.y, u.x);
    let pts: Vec<(Pos2, Pos2)> = (0..=N)
        .map(|k| {
            let r = r0 + (r1 - r0) * k as f32 / N as f32;
            let w = hw(r).max(0.0);
            (c + u * r + n * w, c + u * r - n * w)
        })
        .collect();
    let mut mesh = egui::Mesh::default();
    for (a, b) in &pts {
        mesh.colored_vertex(*a, fill);
        mesh.colored_vertex(*b, fill);
    }
    for k in 0..N as u32 {
        let i = 2 * k;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
    p.add(mesh);
    let mut edge: Vec<Pos2> = pts.iter().map(|q| q.0).collect();
    edge.extend(pts.iter().rev().map(|q| q.1));
    p.add(egui::Shape::closed_line(edge, stroke));
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
fn sector(p: &egui::Painter, c: Pos2, r0: f32, r1: &dyn Fn(f32) -> f32, a0: f32, a1: f32, fill: Color32, stroke: Stroke) {
    let steps = ((a1 - a0).abs() / 3.0).ceil().max(2.0) as usize;
    let at = |outer: bool, k: usize| {
        let deg = a0 + (a1 - a0) * k as f32 / steps as f32;
        let a = deg.to_radians();
        c + vec2(a.cos(), -a.sin()) * if outer { r1(deg) } else { r0 }
    };
    let mut mesh = egui::Mesh::default();
    for k in 0..=steps {
        mesh.colored_vertex(at(false, k), fill);
        mesh.colored_vertex(at(true, k), fill);
    }
    for k in 0..steps as u32 {
        let i = 2 * k;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
    p.add(mesh);
    let mut edge: Vec<Pos2> = (0..=steps).map(|k| at(false, k)).collect();
    edge.extend((0..=steps).rev().map(|k| at(true, k)));
    p.add(egui::Shape::closed_line(edge, stroke));
}

/// `top` over `under` (premultiplied colours).
fn blend(under: Color32, top: Color32) -> Color32 {
    let k = 1.0 - top.a() as f32 / 255.0;
    let mix = |t: u8, u: u8| (t as f32 + u as f32 * k).round().min(255.0) as u8;
    Color32::from_rgba_premultiplied(mix(top.r(), under.r()), mix(top.g(), under.g()), mix(top.b(), under.b()), mix(top.a(), under.a()))
}

/// A ring's piece coloured by `color(across the ring 0..1, along it 0..1)`.
#[allow(clippy::too_many_arguments)]
fn shaded_sector(p: &egui::Painter, c: Pos2, r0: f32, r1: &dyn Fn(f32) -> f32, a0: f32, a1: f32, color: impl Fn(f32, f32) -> Color32) {
    let along = ((a1 - a0).abs() / 2.0).ceil().max(6.0) as usize;
    let across = 12usize;
    let mut mesh = egui::Mesh::default();
    for i in 0..=across {
        let tr = i as f32 / across as f32;
        for k in 0..=along {
            let ta = k as f32 / along as f32;
            let deg = a0 + (a1 - a0) * ta;
            let a = deg.to_radians();
            let r = r0 + (r1(deg) - r0) * tr;
            mesh.colored_vertex(c + vec2(a.cos(), -a.sin()) * r, color(tr, ta));
        }
    }
    let row = (along + 1) as u32;
    for i in 0..across as u32 {
        for k in 0..along as u32 {
            let q = i * row + k;
            mesh.add_triangle(q, q + 1, q + row);
            mesh.add_triangle(q + 1, q + row + 1, q + row);
        }
    }
    p.add(mesh);
}

/// Round ones: how far out a slice `step` degrees wide has room for text
/// `font` high with `pad` above and below.
fn roomy(font: f32, pad: f32, step: f32) -> f32 {
    (font * 0.65 + pad) / (step.to_radians() / 2.0).sin().max(0.01)
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
    let round_font = FontId::proportional(g.font * ROUND_TEXT);
    // (The one pointed at last: over its neighbours as it stands out.)
    let angs = angles(theme, n);
    let is_lit = |i: &usize| l.hot == Some(*i) || (!l.last && l.chosen == Some(*i));
    let order: Vec<usize> = (0..n).filter(|i| !is_lit(i)).chain((0..n).filter(is_lit)).collect();
    for i in order {
        let a = angs[i];
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
                let len = (g.ry - r0).max(60.0 * s) * l.grow;
                let (r0, out) = if lit { (r0 - 4.0 * s, 14.0 * s) } else { (r0, 0.0) };
                let r1 = |d: f32| g.reach(r0, len, d) + out;
                let fill = if lit { accent } else { v.extreme_bg_color.gamma_multiply(0.92) };
                let line = if lit { on_accent.gamma_multiply(0.5) } else { border };
                sector(p, g.anchor, r0, &r1, a - step / 2.0, a + step / 2.0, fill.gamma_multiply(alpha), Stroke::new(1.2, line.gamma_multiply(alpha)));
                // (Out from the narrow middle to where there's room above
                // and below it; clear of the outer edge.)
                let from = (r0 + (r1(a) - r0) * 0.24).max(roomy(g.font * ROUND_TEXT, 6.0 * s, step));
                along(p, g.anchor, a, from, r1(a) - from - 26.0 * s, &it.label, &round_font, color, false);
            }
            Theme::Glow => {
                let r0 = g.outer + 12.0 * s;
                let len = (g.ry - r0).max(60.0 * s) * l.grow;
                let out = if lit { 10.0 * s } else { 0.0 };
                let r1 = |d: f32| g.reach(r0, len, d) + out;
                let (a0, a1) = (a - step / 2.0, a + step / 2.0);
                // A soft dark shade, fading out before the edges; the one
                // pointed at glows from its middle, fading out the same.
                let fade = |t: f32, m: f32| {
                    let e = |x: f32| {
                        let x = (x / m).clamp(0.0, 1.0);
                        x * x * (3.0 - 2.0 * x)
                    };
                    e(t) * e(1.0 - t)
                };
                shaded_sector(p, g.anchor, r0, &r1, a0, a1, |tr, ta| {
                    let shade = Color32::BLACK.gamma_multiply(alpha * 0.6 * fade(ta, 0.3) * fade(tr, 0.25) * (1.0 - 0.3 * tr));
                    if lit {
                        blend(shade, accent.gamma_multiply(alpha * 0.95 * fade(ta, 0.5) * fade(tr, 0.45)))
                    } else {
                        shade
                    }
                });
                // Short dividers, their colour kept.
                let edge = Stroke::new(1.4, border.gamma_multiply(alpha));
                for b in [a0, a1] {
                    let br = b.to_radians();
                    let d = vec2(br.cos(), -br.sin());
                    let span = r1(b) - out - r0;
                    p.line_segment([g.anchor + d * (r0 + 0.3 * span), g.anchor + d * (r0 + 0.7 * span)], edge);
                }
                let c = if lit { v.strong_text_color().gamma_multiply(alpha) } else { color };
                let from = (r0 + (r1(a) - r0) * 0.24).max(roomy(g.font * ROUND_TEXT, 6.0 * s, step));
                along(p, g.anchor, a, from, r1(a) - from - 26.0 * s, &it.label, &round_font, c, false);
            }
            Theme::Daisy => {
                // From narrow at the circle to as wide as fits (a gap
                // either side), then round.
                let r0 = g.outer + 8.0 * s;
                let len = (g.ry * 0.94 - r0).max(60.0 * s) * l.grow;
                let shape = |r1: f32| {
                    let w = ((r1 - 0.15 * (r1 - r0)) * (step.to_radians() / 2.0).sin() * 0.78).clamp(6.0 * s, 34.0 * s * g.stretch.sqrt());
                    (w, (w * 0.25).max(2.0 * s), r1 - w)
                };
                let hw_of = |(w, w0, cap): (f32, f32, f32)| {
                    move |r: f32| {
                        if r <= cap {
                            w0 + (w - w0) * ((r - r0) / (cap - r0).max(1.0)).clamp(0.0, 1.0)
                        } else {
                            (w * w - (r - cap).powi(2)).max(0.0).sqrt()
                        }
                    }
                };
                // The text where the petal's wide enough for it with room
                // above and below (smaller in narrow ones), clear of the
                // round end; the one pointed at bigger, the petal reaching
                // out as far as its whole text takes.
                let most = g.font * ROUND_TEXT * if lit { 1.12 } else { 1.0 };
                let mut r1 = g.reach(r0, len, a) + if lit { 16.0 * s } else { 0.0 };
                let mut run = text_run(hw_of(shape(r1)), r0, r1, most, 5.0 * s);
                if lit {
                    for _ in 0..4 {
                        let gw = p.layout_no_wrap(it.label.clone(), FontId::proportional(run.0), color).size().x;
                        let short = gw + 12.0 * s - (run.2 - run.1 - 4.0 * s);
                        if short <= 0.0 {
                            break;
                        }
                        r1 += short * 1.1;
                        run = text_run(hw_of(shape(r1)), r0, r1, most, 5.0 * s);
                    }
                }
                let (w, w0, cap) = shape(r1);
                let nrm = vec2(-u.y, u.x);
                let mut pts = vec![g.anchor + u * r0 + nrm * w0, g.anchor + u * cap + nrm * w];
                pts.extend((1..16).map(|k| {
                    let t = std::f32::consts::PI * k as f32 / 16.0;
                    g.anchor + u * (cap + w * t.sin()) + nrm * (w * t.cos())
                }));
                pts.extend([g.anchor + u * cap - nrm * w, g.anchor + u * r0 - nrm * w0]);
                let fill = if lit { accent } else { v.extreme_bg_color.gamma_multiply(0.92) };
                let line = if lit { on_accent.gamma_multiply(0.5) } else { border };
                p.add(egui::Shape::convex_polygon(pts, fill.gamma_multiply(alpha), Stroke::new(1.2, line.gamma_multiply(alpha))));
                let (fs, from, to) = run;
                along(p, g.anchor, a, from, (to - from - 4.0 * s).max(10.0), &it.label, &FontId::proportional(fs), color, true);
            }
            Theme::Petals => {
                // Wider the further out (as their slice is), widest well
                // out where the text goes; a gap between neighbours.
                let r0 = g.outer + 6.0 * s;
                let len = (g.ry * 0.92 - r0).max(60.0 * s) * l.grow;
                let half = (step.to_radians() / 2.0).sin();
                let hw_of = |r1: f32| {
                    move |r: f32| {
                        let t = ((r - r0) / (r1 - r0)).clamp(0.0, 1.0);
                        0.78 * half * r * (std::f32::consts::PI * t.powf(1.5)).sin().max(0.0)
                    }
                };
                // The text where the petal's wide enough for it with room
                // above and below (smaller in narrow ones); the one pointed
                // at bigger, the petal reaching out as far as its whole
                // text takes.
                let most = g.font * ROUND_TEXT * if lit { 1.12 } else { 1.0 };
                let mut r1 = g.reach(r0, len, a) + if lit { 16.0 * s } else { 0.0 };
                let mut run = text_run(hw_of(r1), r0, r1, most, 5.0 * s);
                if lit {
                    for _ in 0..4 {
                        let gw = p.layout_no_wrap(it.label.clone(), FontId::proportional(run.0), color).size().x;
                        let short = gw + 12.0 * s - (run.2 - run.1);
                        if short <= 0.0 {
                            break;
                        }
                        r1 += short * 1.3;
                        run = text_run(hw_of(r1), r0, r1, most, 5.0 * s);
                    }
                }
                let fill = if lit { accent } else { v.extreme_bg_color.gamma_multiply(0.92) };
                let line = if lit { on_accent.gamma_multiply(0.5) } else { border };
                petal(p, g.anchor, u, r0, r1, hw_of(r1), fill.gamma_multiply(alpha), Stroke::new(1.2, line.gamma_multiply(alpha)));
                let (fs, from, to) = run;
                along(p, g.anchor, a, from, (to - from).max(10.0), &it.label, &FontId::proportional(fs), color, true);
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
        assert!(most(Theme::Lines, 680.0, 1.0, 1.0) >= 17 && most(Theme::Lines, 400.0, 1.0, 1.0) < most(Theme::Lines, 680.0, 1.0, 1.0));
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
        assert_eq!(angles(Theme::Daisy, 4), r);
        assert_eq!(angles(Theme::Glow, 4), r);
        // Round: roomier than on an arc.
        assert!(most(Theme::Glow, 1000.0, 1.0, 1.0) < most(Theme::Lines, 1000.0, 1.0, 1.0) && most(Theme::Petals, 1000.0, 1.0, 1.0) < most(Theme::Glow, 1000.0, 1.0, 1.0) && most(Theme::Petals, 1000.0, 1.0, 2.0) > most(Theme::Petals, 1000.0, 1.0, 1.0));
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
    let geo = Geo { anchor, outer, s, ry: (rect.height() / 2.0 - 10.0).min(170.0 * s), font: 12.0, stretch: prefs.length };
    let look = Look { rx: geo.ry * 1.25, alpha: fade, swing: 0.0, hot: Some(hot), chosen: None, others: 1.0, last: true, grow: 1.0 };
    let items: Vec<Shown> = labels.iter().map(|l| Shown { label: l.to_string(), enabled: true }).collect();
    paint_level(&painter, theme, &geo, &look, &items, v);
}
