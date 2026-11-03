//! Touches for text, shared by the text fields and the text editor:
//!
//! - handles under the caret, or a choice's two ends, while the screen is
//!   touched (as on a phone): a finger drags them;
//! - a magnifier over the place being chosen (a handle dragged; with the
//!   mouse too, if set, a drag choosing text);
//! - a typed letter that grows a little and hops as it comes.

use std::f32::consts::PI;

use egui::emath::TSTransform;
use egui::{Align2, Color32, Context, FontId, Id, LayerId, Order, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};

/// How long a typed letter's hop lasts (seconds).
pub const HOP: f32 = 0.24;
/// The magnifier's size, and how much bigger it shows.
const LENS: Vec2 = vec2(150.0, 58.0);
const ZOOM: f32 = 1.6;
/// A handle's drop.
const R: f32 = 10.0;

/// What the settings and the input say, for the drawing code (set each
/// frame).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fx {
    /// The screen was touched last: handles show.
    pub touch: bool,
    /// The magnifier: 0 never, 1 for a finger, 2 for the mouse too.
    pub lens: u8,
    /// Typed letters hop.
    pub hop: bool,
    /// Nothing over the panes (no menu, no window): the editor's handles
    /// may show.
    pub free: bool,
}

fn fx_id() -> Id {
    Id::new("textfx")
}

pub fn set(ctx: &Context, fx: Fx) {
    ctx.data_mut(|d| d.insert_temp(fx_id(), fx));
}

pub fn get(ctx: &Context) -> Fx {
    ctx.data(|d| d.get_temp(fx_id())).unwrap_or_default()
}

// ------------------------------------------------------------ the hop

/// A letter `age` seconds old: how much bigger and how far up it is
/// (None: it's done).
pub fn hop(age: f32) -> Option<(f32, f32)> {
    if !(0.0..HOP).contains(&age) {
        return None;
    }
    let s = (age / HOP * PI).sin();
    Some((1.0 + 0.28 * s, 2.5 * s))
}

/// A letter hopping: its place (`cell`, a glyph's box) covered with `bg`,
/// the letter drawn over it bigger and higher.
pub fn paint_hop(p: &Painter, cell: Rect, chr: char, font: &FontId, color: Color32, bg: Color32, age: f32) {
    let Some((scale, up)) = hop(age) else { return };
    p.rect_filled(cell, 0.0, bg);
    let f = FontId::new(font.size * scale, font.family.clone());
    p.text(cell.center() - vec2(0.0, up), Align2::CENTER_CENTER, chr, f, color);
    p.ctx().request_repaint();
    #[cfg(test)]
    p.ctx().data_mut(|d| *d.get_temp_mut_or_default::<u32>(Id::new("hops-drawn")) += 1);
}

/// Letters typed lately into a field (char index, when): kept by the
/// field's id; a letter typed at `at` (the others after it shift on).
pub fn typed(ctx: &Context, id: Id, at: Option<usize>, count: usize) -> Vec<(usize, f64)> {
    let key = id.with("typed");
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| {
        let list: &mut Vec<(usize, f64)> = d.get_temp_mut_or_default(key);
        list.retain(|(_, t)| ((now - t) as f32) < HOP);
        if let Some(at) = at {
            for (i, _) in list.iter_mut() {
                if *i >= at {
                    *i += count;
                }
            }
            list.extend((at..at + count).map(|i| (i, now)));
        }
        list.clone()
    })
}

/// Forget the letters typed into a field (a change that wasn't typing).
pub fn forget(ctx: &Context, id: Id) {
    ctx.data_mut(|d| d.remove::<Vec<(usize, f64)>>(id.with("typed")));
}

// ------------------------------------------------------------ handles

/// Where handles go: under the caret, or under a choice's two ends (each
/// the foot of its caret: the bottom of its line).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ends {
    Caret(Pos2),
    Range(Pos2, Pos2),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Caret,
    Start,
    End,
}

/// The handles' box, for a finger (`foot`: where the drop hangs from).
fn grab_rect(which: Which, foot: Pos2) -> Rect {
    let c = drop_center(which, foot);
    Rect::from_center_size(c, Vec2::splat(40.0)).union(Rect::from_center_size(foot + vec2(0.0, 4.0), vec2(20.0, 8.0)))
}

fn drop_center(which: Which, foot: Pos2) -> Pos2 {
    match which {
        Which::Caret => foot + vec2(0.0, R * 1.45),
        Which::Start => foot + vec2(-R, R),
        Which::End => foot + vec2(R, R),
    }
}

/// A drop hanging from `foot`: under a caret its point up, under a
/// choice's start its point up and right, under its end up and left.
pub fn paint_drop(p: &Painter, which: Which, foot: Pos2, color: Color32) {
    let c = drop_center(which, foot);
    p.circle_filled(c, R, color);
    match which {
        Which::Caret => {
            let h = R * 0.725;
            p.add(Shape::convex_polygon(vec![foot, pos2(foot.x + h, foot.y + h), c, pos2(foot.x - h, foot.y + h)], color, Stroke::NONE));
        }
        Which::Start => {
            p.rect_filled(Rect::from_min_max(pos2(foot.x - R, foot.y), pos2(foot.x, foot.y + R)), 0.0, color);
        }
        Which::End => {
            p.rect_filled(Rect::from_min_max(foot, pos2(foot.x + R, foot.y + R)), 0.0, color);
        }
    }
}

/// The handles (when the screen was touched last): drawn above all, under
/// `ends` within `clip`; one dragged: which, and the place in the text it's
/// at now (the middle of the line above the drop, `line_h` high). The
/// magnifier follows it, if set.
pub fn handles(ctx: &Context, id: Id, ends: Ends, line_h: f32, clip: Rect, color: Color32, text_layer: LayerId) -> Option<(Which, Pos2)> {
    let fx = get(ctx);
    let held: Option<(Which, Vec2)> = ctx.data(|d| d.get_temp(id.with("held")));
    if !fx.touch && held.is_none() {
        return None;
    }
    let list: Vec<(Which, Pos2)> = match ends {
        Ends::Caret(f) => vec![(Which::Caret, f)],
        Ends::Range(a, b) => vec![(Which::Start, a), (Which::End, b)],
    };
    let mut out = None;
    let mut still = false;
    for (which, foot) in list {
        let dragging = held.is_some_and(|(w, _)| w == which);
        // Out of sight (scrolled away): none, unless it's being dragged.
        if !clip.expand(1.0).contains(foot) && !dragging {
            continue;
        }
        let grab = grab_rect(which, foot);
        let layer = LayerId::new(Order::Foreground, id.with(("handle", which as u8)));
        let p = ctx.layer_painter(layer);
        paint_drop(&p, which, foot, color);
        let r = egui::Area::new(id.with(("handle-area", which as u8))).order(Order::Foreground).fixed_pos(grab.min).show(ctx, |ui| ui.allocate_exact_size(grab.size(), Sense::drag()).1).inner;
        crate::textfx::grip(ctx, grab);
        if r.drag_started() {
            let at = r.interact_pointer_pos().unwrap_or(foot);
            ctx.data_mut(|d| d.insert_temp(id.with("held"), (which, at - foot)));
        }
        if r.dragged() {
            still = true;
            if let Some(pt) = ctx.pointer_latest_pos() {
                let off = held.filter(|(w, _)| *w == which).map_or(Vec2::ZERO, |(_, o)| o);
                let at = pt - off - vec2(0.0, line_h / 2.0);
                if fx.lens >= 1 {
                    ask_lens(ctx, text_layer, at, line_h);
                }
                out = Some((which, at));
            }
        }
    }
    if !still && held.is_some() {
        ctx.data_mut(|d| d.remove::<(Which, Vec2)>(id.with("held")));
    }
    out
}

/// A handle's box: a finger put there drags it at once (no scrolling).
pub fn grip(ctx: &Context, r: Rect) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<Rect>>(Id::new("text-grips")).push(r));
}

/// The handles' boxes this frame (for the touch screen's next).
pub fn take_grips(ctx: &Context) -> Vec<Rect> {
    ctx.data_mut(|d| d.remove_temp::<Vec<Rect>>(Id::new("text-grips"))).unwrap_or_default()
}

// ------------------------------------------------------------ the magnifier

#[derive(Clone, Copy, Debug)]
struct LensAsk {
    layer: LayerId,
    at: Pos2,
    line_h: f32,
}

/// Show what's at `at` (painted on `layer`) bigger, above it, this frame.
pub fn ask_lens(ctx: &Context, layer: LayerId, at: Pos2, line_h: f32) {
    ctx.data_mut(|d| d.insert_temp(Id::new("lens-ask"), LensAsk { layer, at, line_h }));
}

/// A mouse drag choosing text: the magnifier there, if set for the mouse.
pub fn mouse_lens(ctx: &Context, layer: LayerId, at: Pos2, line_h: f32) {
    let fx = get(ctx);
    if fx.lens >= 2 && !fx.touch {
        ask_lens(ctx, layer, at, line_h);
    }
}

/// Where the magnifier goes for a place in the text: centred above it,
/// on the screen.
pub fn lens_rect(screen: Rect, at: Pos2, line_h: f32) -> Rect {
    let mut r = Rect::from_center_size(pos2(at.x, at.y - line_h / 2.0 - 14.0 - LENS.y / 2.0), LENS);
    if r.min.y < screen.min.y + 4.0 {
        // No room above: below, past the handle.
        r = r.translate(vec2(0.0, LENS.y + line_h + 14.0 + 3.0 * R + 14.0));
    }
    let dx = (screen.min.x + 4.0 - r.min.x).max(0.0) + (screen.max.x - 4.0 - r.max.x).min(0.0);
    r.translate(vec2(dx, 0.0))
}

/// The magnifier asked for this frame, drawn above everything (at the
/// frame's end: what it shows is painted by then).
pub fn draw_lens(ctx: &Context) {
    #[cfg(test)]
    ctx.data_mut(|d| d.remove::<Rect>(Id::new("lens-drawn")));
    let Some(ask) = ctx.data_mut(|d| {
        let a = d.get_temp::<LensAsk>(Id::new("lens-ask"));
        d.remove::<LensAsk>(Id::new("lens-ask"));
        a
    }) else {
        return;
    };
    let screen = ctx.content_rect();
    let lens = lens_rect(screen, ask.at, ask.line_h);
    let src = Rect::from_center_size(ask.at, LENS / ZOOM);
    let t = TSTransform::new(lens.center().to_vec2() - ask.at.to_vec2() * ZOOM, ZOOM);
    let shapes: Vec<(Rect, Shape)> = ctx.graphics(|g| {
        g.get(ask.layer)
            .map(|l| l.all_entries().filter(|c| !matches!(c.shape, Shape::Callback(_)) && c.clip_rect.intersects(src) && c.shape.visual_bounding_rect().intersects(src)).map(|c| (c.clip_rect, c.shape.clone())).collect())
            .unwrap_or_default()
    });
    let layer = LayerId::new(Order::Tooltip, Id::new("lens"));
    let p = ctx.layer_painter(layer);
    let v = ctx.global_style().visuals.clone();
    p.rect_filled(lens.translate(vec2(0.0, 3.0)).expand(1.0), 12.0, Color32::from_black_alpha(70));
    p.rect_filled(lens, 12.0, v.extreme_bg_color);
    let inner = lens.shrink(3.0);
    for (clip, mut s) in shapes {
        s.transform(t);
        p.with_clip_rect(t.mul_rect(clip).intersect(inner)).add(s);
    }
    p.rect_stroke(lens, 12.0, Stroke::new(1.5, v.widgets.noninteractive.bg_stroke.color), StrokeKind::Outside);
    #[cfg(test)]
    ctx.data_mut(|d| d.insert_temp(Id::new("lens-drawn"), lens));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_rises_and_settles() {
        assert_eq!(hop(-0.1), None);
        assert_eq!(hop(HOP), None);
        let (s0, u0) = hop(0.0).unwrap();
        assert!((s0 - 1.0).abs() < 1e-4 && u0.abs() < 1e-4);
        let (s, u) = hop(HOP / 2.0).unwrap();
        assert!(s > 1.2 && u > 2.0, "{s} {u}");
    }

    #[test]
    fn lens_stays_on_screen() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let r = lens_rect(screen, pos2(400.0, 300.0), 20.0);
        assert!(r.max.y < 290.0 && (r.center().x - 400.0).abs() < 0.1);
        let r = lens_rect(screen, pos2(2.0, 10.0), 20.0);
        assert!(screen.contains_rect(r), "{r:?}");
        assert!(r.min.y > 20.0, "not moved below: {r:?}");
        let r = lens_rect(screen, pos2(799.0, 300.0), 20.0);
        assert!(r.max.x <= 796.0);
    }

    #[test]
    fn typed_letters_shift_and_expire() {
        let ctx = Context::default();
        let id = Id::new("f");
        let l = typed(&ctx, id, Some(3), 1);
        assert_eq!(l.iter().map(|x| x.0).collect::<Vec<_>>(), vec![3]);
        let l = typed(&ctx, id, Some(0), 2);
        assert_eq!(l.iter().map(|x| x.0).collect::<Vec<_>>(), vec![5, 0, 1]);
        forget(&ctx, id);
        assert!(typed(&ctx, id, None, 0).is_empty());
    }
}
