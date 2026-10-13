//! Drawing the disk usage page.

use std::f32::consts::{PI, TAU};
use std::sync::atomic::Ordering;

use egui::epaint::{Mesh, PathShape, PathStroke};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::{Kind, Mode, Node, RINGS, Ready, State, Usage, ring_segs, squarify};
use crate::app::human_size;
use crate::i18n::{tr, trn};
use crate::icons::Icons;
use crate::view::{elided_text, lookup_px, paint_row_bg};

pub enum Act {
    Close,
    Mode(Mode),
    Rescan,
    /// Pick an item (child indices from the root).
    Pick(Vec<usize>),
    /// Pick a folder and put it in the middle.
    Zoom(Vec<usize>),
    ZoomOut,
    /// A crumb of the path: the middle becomes that folder (its depth).
    Crumb(usize),
    /// Measure the start screen's item.
    Scan(usize),
    Cancel,
    /// Right click: pick, then the actions.
    Context(Vec<usize>),
}

/// Width of the facts beside the chart.
const SIDE: f32 = 320.0;
/// Thinnest piece drawn in the rings (a fraction of the circle).
const MIN_FRAC: f64 = 0.6 / 360.0;

pub fn color(mid: f64, depth: usize, kind: Kind) -> Color32 {
    if let Kind::Rest(_) = kind {
        return Color32::from_gray(150);
    }
    let d = depth.saturating_sub(1) as f32;
    let mut sat = (0.60 - 0.06 * d).max(0.30);
    let mut val = (0.84 + 0.02 * d).min(0.90);
    if kind == Kind::File {
        sat *= 0.62;
        val -= 0.04;
    }
    egui::ecolor::Hsva::new(mid as f32, sat, val, 1.0).into()
}

fn kind_label(n: &Node) -> String {
    match n.kind {
        Kind::Dir => trn("Folder, %n files", n.files as usize),
        Kind::File => tr("File").to_string(),
        Kind::Rest(k) => trn("%n small files", k as usize),
    }
}

pub fn item_name(n: &Node) -> String {
    match n.kind {
        Kind::Rest(k) => trn("%n small files", k as usize),
        _ => n.name.to_string(),
    }
}

fn header_button(ui: &mut Ui, r: Rect, label: &str, tip: &str, on: bool) -> bool {
    let vis = ui.visuals().clone();
    let resp = ui.interact(r, ui.id().with(("usage-btn", label)), Sense::click()).on_hover_text(tip);
    if on {
        ui.painter().rect_filled(r, 6.0, vis.selection.bg_fill.gamma_multiply(0.45));
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 6.0, vis.widgets.hovered.weak_bg_fill);
    }
    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(15.0), vis.text_color());
    resp.clicked()
}

pub fn draw(ui: &mut Ui, u: &mut Usage, icons: &mut Icons) -> Option<Act> {
    let mut act = None;
    let vis = ui.visuals().clone();
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let mut x = head.min.x + 6.0;
    let title = ui.painter().layout_no_wrap(tr("Disk usage").into(), FontId::proportional(16.0), vis.strong_text_color());
    ui.painter().galley(pos2(x, head.center().y - title.size().y / 2.0), title.clone(), vis.strong_text_color());
    x += title.size().x + 14.0;
    let mut bx = head.max.x - 30.0;
    let btn = |bx: f32, w: f32| Rect::from_min_size(pos2(bx, head.min.y + 3.0), vec2(w, 28.0));
    if header_button(ui, btn(bx, 28.0), "✕", tr("Close"), false) {
        act = Some(Act::Close);
    }
    if u.ready().is_some() {
        bx -= 32.0;
        if header_button(ui, btn(bx, 28.0), "⟳", tr("Measure again"), false) {
            act = Some(Act::Rescan);
        }
        for m in Mode::ALL.iter().rev() {
            let label = mode_name(*m);
            let w = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(15.0), vis.text_color()).size().x + 20.0;
            bx -= w + 4.0;
            if header_button(ui, btn(bx, w), label, tr("How to show it (□)"), u.mode == *m) && u.mode != *m {
                act = Some(Act::Mode(*m));
            }
        }
    }
    // The path to the folder in the middle, each part clickable.
    if let Some(r) = u.ready() {
        let mut parts = vec![root_name(&r.tree)];
        parts.extend(r.tree.names(&r.center).iter().map(|n| n.to_string()));
        let room = bx - 12.0;
        let n = parts.len();
        let font = FontId::proportional(14.0);
        for (i, p) in parts.iter().enumerate() {
            if x > room - 40.0 {
                break;
            }
            let last = i + 1 == n;
            let color = if last { vis.strong_text_color() } else { vis.text_color() };
            let g = elided_text(ui, p, font.clone(), color, (room - x).min(260.0));
            let rect = Rect::from_min_size(pos2(x, head.center().y - 12.0), vec2(g.size().x + 12.0, 24.0));
            let resp = ui.interact(rect, ui.id().with(("usage-crumb", i)), Sense::click());
            if resp.hovered() && !last {
                ui.painter().rect_filled(rect, 5.0, vis.widgets.hovered.weak_bg_fill);
            }
            ui.painter().galley(pos2(x + 6.0, head.center().y - g.size().y / 2.0), g.clone(), color);
            if resp.clicked() && !last {
                act = Some(Act::Crumb(i));
            }
            x = rect.max.x + 2.0;
            if !last {
                ui.painter().text(pos2(x, head.center().y), Align2::LEFT_CENTER, "›", font.clone(), vis.weak_text_color());
                x += 12.0;
            }
        }
    }
    ui.add_space(4.0);
    let area = ui.available_rect_before_wrap();
    ui.allocate_rect(area, Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    let ui = &mut child;
    let a = match &mut u.state {
        State::Start { targets, cursor } => draw_start(ui, area, targets, *cursor, icons),
        State::Scanning(s) => {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
            draw_scanning(ui, area, s)
        }
        State::Ready(r) => {
            let reveal = std::mem::take(&mut u.reveal);
            match u.mode {
                Mode::List => draw_list(ui, area, r, reveal),
                m => {
                    let wide = area.width() >= 760.0;
                    let chart = if wide { Rect::from_min_max(area.min, pos2(area.max.x - SIDE - 8.0, area.max.y)) } else { area };
                    let mut a = if m == Mode::Rings { draw_rings(ui, chart, r) } else { draw_tiles(ui, chart, r) };
                    if wide {
                        let side = Rect::from_min_max(pos2(area.max.x - SIDE, area.min.y), area.max);
                        if let Some(x) = draw_side(ui, side, r) {
                            a = Some(x);
                        }
                    }
                    a
                }
            }
        }
    };
    a.or(act)
}

/// The measured folder's own name ("/" for the root).
fn root_name(tree: &Node) -> String {
    tree.name.rsplit('/').find(|s| !s.is_empty()).unwrap_or("/").to_string()
}

pub fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Rings => tr("Rings"),
        Mode::Tiles => tr("Tiles"),
        Mode::List => tr("List"),
    }
}

// ---------------------------------------------------------------- start

fn draw_start(ui: &mut Ui, area: Rect, targets: &[super::Target], cursor: usize, icons: &mut Icons) -> Option<Act> {
    let vis = ui.visuals().clone();
    let mut act = None;
    let w = area.width().min(680.0) - 32.0;
    let x0 = area.center().x - w / 2.0;
    let mut y = area.min.y + 24.0;
    ui.painter().text(pos2(x0, y), Align2::LEFT_TOP, tr("What to measure?"), FontId::proportional(20.0), vis.strong_text_color());
    y += 34.0;
    ui.painter().text(pos2(x0, y), Align2::LEFT_TOP, tr("Folders are measured with everything inside; other disks mounted within are skipped."), FontId::proportional(13.0), vis.weak_text_color());
    y += 30.0;
    egui::ScrollArea::vertical().id_salt("usage-start").max_height(area.max.y - y).show(&mut ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(x0 - 4.0, y), pos2(x0 + w + 4.0, area.max.y)))), |ui| {
        for (i, t) in targets.iter().enumerate() {
            let h = if t.space.is_some() { 78.0 } else { 58.0 };
            let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
            paint_row_bg(ui, rect, i == cursor, false, resp.hovered());
            if i == cursor {
                resp.scroll_to_me(None);
            }
            let icon = Rect::from_center_size(pos2(rect.min.x + 30.0, rect.min.y + 29.0), Vec2::splat(32.0));
            if let Some(u) = icons.named(t.icon, lookup_px(ui, 32.0)) {
                egui::Image::new(u).paint_at(ui, icon);
            }
            let tx = rect.min.x + 60.0;
            ui.painter().text(pos2(tx, rect.min.y + 10.0), Align2::LEFT_TOP, &t.label, FontId::proportional(16.0), vis.strong_text_color());
            let g = elided_text(ui, &t.path.display().to_string(), FontId::proportional(13.0), vis.weak_text_color(), rect.max.x - tx - 12.0);
            ui.painter().galley(pos2(tx, rect.min.y + 32.0), g, vis.weak_text_color());
            if let Some((total, used)) = t.space {
                let bar = Rect::from_min_size(pos2(tx, rect.min.y + 56.0), vec2((rect.max.x - tx - 12.0) * 0.6, 8.0));
                ui.painter().rect_filled(bar, 4.0, vis.extreme_bg_color);
                let f = used as f32 / total.max(1) as f32;
                let fill = if f > 0.9 { vis.error_fg_color } else { vis.selection.bg_fill };
                ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f, bar.height())), 4.0, fill);
                let text = format!("{} / {} · {} {}", human_size(used), human_size(total), human_size(total - used), tr("free"));
                ui.painter().text(pos2(bar.max.x + 10.0, bar.center().y), Align2::LEFT_CENTER, text, FontId::proportional(12.5), vis.text_color());
            }
            if resp.clicked() {
                act = Some(Act::Scan(i));
            }
        }
    });
    act
}

// ---------------------------------------------------------------- scanning

fn draw_scanning(ui: &mut Ui, area: Rect, s: &super::Scan) -> Option<Act> {
    let vis = ui.visuals().clone();
    let p = &s.progress;
    let c = area.center();
    let w = area.width().min(560.0) - 32.0;
    let spin = Rect::from_center_size(pos2(c.x, c.y - 70.0), Vec2::splat(40.0));
    ui.put(spin, egui::Spinner::new().size(40.0));
    ui.painter().text(pos2(c.x, c.y - 26.0), Align2::CENTER_CENTER, tr("Measuring…"), FontId::proportional(18.0), vis.strong_text_color());
    let files = p.files.load(Ordering::Relaxed);
    let bytes = p.bytes.load(Ordering::Relaxed);
    let line = format!("{} · {} · {:.0} {}", trn("%n files", files as usize), human_size(bytes), s.started.elapsed().as_secs_f32(), tr("sec"));
    ui.painter().text(pos2(c.x, c.y + 2.0), Align2::CENTER_CENTER, line, FontId::proportional(14.0), vis.text_color());
    if let Some(total) = s.expect {
        let bar = Rect::from_center_size(pos2(c.x, c.y + 28.0), vec2(w, 8.0));
        ui.painter().rect_filled(bar, 4.0, vis.extreme_bg_color);
        let f = (bytes as f32 / total.max(1) as f32).min(1.0);
        ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f, 8.0)), 4.0, vis.selection.bg_fill);
    }
    let cur = p.current.lock().map(|c| c.display().to_string()).unwrap_or_default();
    let g = elided_text(ui, &cur, FontId::proportional(12.5), vis.weak_text_color(), w);
    ui.painter().galley(pos2(c.x - g.size().x / 2.0, c.y + 44.0), g, vis.weak_text_color());
    let b = Rect::from_center_size(pos2(c.x, c.y + 96.0), vec2(120.0, 32.0));
    if ui.put(b, egui::Button::new(tr("Cancel"))).clicked() {
        return Some(Act::Cancel);
    }
    None
}

// ---------------------------------------------------------------- rings

/// A ring piece from angle a0 to a1 between two radii.
fn sector(c: Pos2, r0: f32, r1: f32, a0: f32, a1: f32) -> (Vec<Pos2>, Vec<Pos2>) {
    let steps = (((a1 - a0) * r1 / 4.0).ceil() as usize).clamp(1, 256);
    let at = |r: f32, a: f32| c + vec2(a.cos(), a.sin()) * r;
    let outer: Vec<Pos2> = (0..=steps).map(|k| at(r1, a0 + (a1 - a0) * k as f32 / steps as f32)).collect();
    let inner: Vec<Pos2> = (0..=steps).map(|k| at(r0, a0 + (a1 - a0) * k as f32 / steps as f32)).collect();
    (inner, outer)
}

fn fill_sector(ui: &Ui, inner: &[Pos2], outer: &[Pos2], color: Color32) {
    let mut m = Mesh::default();
    for (i, (a, b)) in inner.iter().zip(outer).enumerate() {
        m.colored_vertex(*a, color);
        m.colored_vertex(*b, color);
        if i > 0 {
            let k = (i * 2) as u32;
            m.add_triangle(k - 2, k - 1, k);
            m.add_triangle(k - 1, k + 1, k);
        }
    }
    ui.painter().add(m);
}

fn outline(inner: &[Pos2], outer: &[Pos2]) -> Vec<Pos2> {
    outer.iter().copied().chain(inner.iter().rev().copied()).collect()
}

/// Text that reads on a piece's colour.
fn ink(_fill: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(20, 20, 24, 230)
}

fn draw_rings(ui: &mut Ui, rect: Rect, r: &Ready) -> Option<Act> {
    let vis = ui.visuals().clone();
    let center = r.center_node();
    let c = rect.center();
    let big = (rect.width().min(rect.height()) / 2.0 - 14.0).max(60.0);
    let r0 = big * 0.24;
    let wr = (big - r0) / RINGS as f32;
    let segs = ring_segs(center, RINGS, MIN_FRAC);
    let resp = ui.interact(rect, ui.id().with("usage-rings"), Sense::click());
    let hover = resp.hover_pos();
    // What the pointer is over: the middle, a piece, or nothing.
    let mut hit: Option<usize> = None;
    let mut over_middle = false;
    if let Some(p) = hover {
        let d = p - c;
        let dist = d.length();
        if dist < r0 {
            over_middle = true;
        } else if dist < big {
            let depth = ((dist - r0) / wr) as usize + 1;
            let f = ((d.y.atan2(d.x) + PI / 2.0).rem_euclid(TAU) / TAU) as f64;
            hit = segs.iter().position(|s| s.depth == depth && s.f0 <= f && f < s.f1);
        }
    }
    let base = r.center.len();
    let sel_rel = r.sel.get(base..).unwrap_or(&[]);
    let mut picked = None;
    let mut labels = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        let n = center.at(&s.path);
        let a0 = s.f0 as f32 * TAU - PI / 2.0;
        let a1 = s.f1 as f32 * TAU - PI / 2.0;
        let ri = r0 + (s.depth - 1) as f32 * wr + 1.0;
        let (inner, outer) = sector(c, ri, ri + wr - 1.5, a0, a1);
        let mut col = color((s.f0 + s.f1) / 2.0, s.depth, n.kind);
        if hit == Some(i) {
            col = col.lerp_to_gamma(Color32::WHITE, 0.3);
        }
        fill_sector(ui, &inner, &outer, col);
        ui.painter().add(PathShape::closed_line(outline(&inner, &outer), PathStroke::new(1.0, vis.panel_fill)));
        if s.path.as_slice() == sel_rel {
            picked = Some(outline(&inner, &outer));
        }
        // Names on the pieces wide enough (drawn after all the pieces).
        let mid_r = ri + wr / 2.0;
        let arc = (a1 - a0) * mid_r;
        if arc > 56.0 && wr > 16.0 && s.depth <= 2 {
            let am = (a0 + a1) / 2.0;
            labels.push((c + vec2(am.cos(), am.sin()) * mid_r, item_name(n), (arc * 0.8).min(wr * 1.5), col));
        }
    }
    for (at, text, w, col) in labels {
        let g = ui.painter().layout_no_wrap(text, FontId::proportional(12.0), ink(col));
        if g.size().x <= w {
            ui.painter().galley(at - g.size() / 2.0, g, ink(col));
        }
    }
    if let Some(o) = picked {
        ui.painter().add(PathShape::closed_line(o.clone(), PathStroke::new(4.0, vis.panel_fill)));
        ui.painter().add(PathShape::closed_line(o, PathStroke::new(2.5, vis.strong_text_color())));
    }
    // The middle: the folder, its size; a click goes up.
    let mid_fill = if over_middle { vis.widgets.hovered.weak_bg_fill } else { vis.extreme_bg_color };
    ui.painter().circle_filled(c, r0 - 2.0, mid_fill);
    let name = if r.center.is_empty() { root_name(&r.tree) } else { center.name.to_string() };
    let g = elided_text(ui, &name, FontId::proportional(14.0), vis.strong_text_color(), r0 * 1.7);
    ui.painter().galley(pos2(c.x - g.size().x / 2.0, c.y - g.size().y - 1.0), g, vis.strong_text_color());
    ui.painter().text(pos2(c.x, c.y + 2.0), Align2::CENTER_TOP, human_size(center.size), FontId::proportional(13.0), vis.text_color());
    if hover.is_some() {
        if let Some(i) = hit {
            let s = &segs[i];
            let n = center.at(&s.path);
            resp.clone().on_hover_text_at_pointer(tip(n, center));
        } else if over_middle && !r.center.is_empty() {
            resp.clone().on_hover_text_at_pointer(tr("Up a level"));
        }
    }
    let full = |rel: &[usize]| {
        let mut p = r.center.clone();
        p.extend_from_slice(rel);
        p
    };
    if resp.clicked() {
        if over_middle {
            return Some(Act::ZoomOut);
        }
        if let Some(i) = hit {
            let path = full(&segs[i].path);
            return Some(if center.at(&segs[i].path).is_dir() { Act::Zoom(path) } else { Act::Pick(path) });
        }
    }
    if resp.secondary_clicked() {
        if let Some(i) = hit {
            return Some(Act::Context(full(&segs[i].path)));
        }
    }
    None
}

fn tip(n: &Node, center: &Node) -> String {
    let pct = 100.0 * n.size as f64 / center.size.max(1) as f64;
    format!("{}\n{} · {pct:.1}%\n{}", item_name(n), human_size(n.size), kind_label(n))
}

// ---------------------------------------------------------------- tiles

struct Tile {
    rect: Rect,
    path: Vec<usize>,
}

#[allow(clippy::too_many_arguments)]
fn tiles(ui: &Ui, n: &Node, path: &mut Vec<usize>, rect: Rect, depth: usize, f0: f64, span: f64, out: &mut Vec<Tile>) {
    if n.size == 0 {
        return;
    }
    let vis = ui.visuals();
    let sizes: Vec<f64> = n.children.iter().map(|c| c.size as f64).collect();
    let rects = squarify(&sizes, rect);
    let mut at = f0;
    for (i, (c, r)) in n.children.iter().zip(rects).enumerate() {
        let w = span * c.size as f64 / n.size as f64;
        let r = r.shrink(1.0);
        let (f, mid) = (at, at + w / 2.0);
        at += w;
        if r.width() < 2.0 || r.height() < 2.0 {
            continue;
        }
        path.push(i);
        let col = color(mid, depth, c.kind);
        let open = c.is_dir() && depth < Mode::Tiles.depth() && r.width() > 70.0 && r.height() > 46.0 && !c.children.is_empty();
        if open {
            // A folder: its name on a strip, what's inside below.
            ui.painter().rect_filled(r, 3.0, col.lerp_to_gamma(vis.panel_fill, 0.55));
            let strip = Rect::from_min_size(r.min, vec2(r.width(), 18.0));
            let text = format!("{} · {}", c.name, human_size(c.size));
            let g = elided_text(ui, &text, FontId::proportional(12.0), vis.strong_text_color(), r.width() - 8.0);
            ui.painter().galley(pos2(strip.min.x + 4.0, strip.center().y - g.size().y / 2.0), g, vis.strong_text_color());
            out.push(Tile { rect: r, path: path.clone() });
            let inner = Rect::from_min_max(pos2(r.min.x + 2.0, r.min.y + 19.0), pos2(r.max.x - 2.0, r.max.y - 2.0));
            tiles(ui, c, path, inner, depth + 1, f, w, out);
        } else {
            ui.painter().rect_filled(r, 2.0, col);
            if r.width() > 46.0 && r.height() > 18.0 {
                let g = elided_text(ui, &item_name(c), FontId::proportional(12.0), ink(col), r.width() - 8.0);
                ui.painter().galley(pos2(r.min.x + 4.0, r.min.y + 3.0), g, ink(col));
                if r.height() > 36.0 {
                    ui.painter().text(pos2(r.min.x + 4.0, r.min.y + 19.0), Align2::LEFT_TOP, human_size(c.size), FontId::proportional(11.5), ink(col));
                }
            }
            out.push(Tile { rect: r, path: path.clone() });
        }
        path.pop();
    }
}

fn draw_tiles(ui: &mut Ui, rect: Rect, r: &Ready) -> Option<Act> {
    let vis = ui.visuals().clone();
    let center = r.center_node();
    let area = rect.shrink(6.0);
    let resp = ui.interact(area, ui.id().with("usage-tiles"), Sense::click());
    let mut out = Vec::new();
    tiles(ui, center, &mut Vec::new(), area, 1, 0.0, 1.0, &mut out);
    let base = r.center.len();
    let sel_rel = r.sel.get(base..).unwrap_or(&[]);
    // The deepest tile under the pointer.
    let hit = resp.hover_pos().and_then(|p| out.iter().rposition(|t| t.rect.contains(p)));
    if let Some(i) = hit {
        ui.painter().rect_filled(out[i].rect, 2.0, Color32::from_white_alpha(40));
    }
    if let Some(t) = out.iter().find(|t| t.path.as_slice() == sel_rel) {
        ui.painter().rect_stroke(t.rect, 3.0, Stroke::new(4.0, vis.panel_fill), StrokeKind::Outside);
        ui.painter().rect_stroke(t.rect, 3.0, Stroke::new(2.5, vis.strong_text_color()), StrokeKind::Outside);
    } else if let Some(t) = out.iter().filter(|t| sel_rel.starts_with(&t.path)).max_by_key(|t| t.path.len()) {
        // Too small to draw: its nearest drawn folder is marked instead.
        ui.painter().rect_stroke(t.rect, 3.0, Stroke::new(2.0, vis.strong_text_color().gamma_multiply(0.6)), StrokeKind::Outside);
    }
    let full = |rel: &[usize]| {
        let mut p = r.center.clone();
        p.extend_from_slice(rel);
        p
    };
    if let Some(i) = hit {
        let n = center.at(&out[i].path);
        resp.clone().on_hover_text_at_pointer(tip(n, center));
        if resp.clicked() {
            let path = full(&out[i].path);
            return Some(if n.is_dir() { Act::Zoom(path) } else { Act::Pick(path) });
        }
        if resp.secondary_clicked() {
            return Some(Act::Context(full(&out[i].path)));
        }
    }
    None
}

// ---------------------------------------------------------------- facts

fn draw_side(ui: &mut Ui, rect: Rect, r: &Ready) -> Option<Act> {
    let vis = ui.visuals().clone();
    let mut act = None;
    ui.painter().rect_filled(rect, 8.0, vis.faint_bg_color);
    let center = r.center_node();
    let x = rect.min.x + 14.0;
    let w = rect.width() - 28.0;
    let mut y = rect.min.y + 12.0;
    let name = if r.center.is_empty() { root_name(&r.tree) } else { center.name.to_string() };
    let g = elided_text(ui, &name, FontId::proportional(16.0), vis.strong_text_color(), w);
    ui.painter().galley(pos2(x, y), g, vis.strong_text_color());
    y += 22.0;
    let path = match r.tree.names(&r.center) {
        n if n.is_empty() => r.tree.name.to_string(),
        n => format!("{}/{}", r.tree.name.trim_end_matches('/'), n.join("/")),
    };
    let g = elided_text(ui, &path, FontId::proportional(12.0), vis.weak_text_color(), w);
    ui.painter().galley(pos2(x, y), g, vis.weak_text_color());
    y += 22.0;
    ui.painter().text(pos2(x, y), Align2::LEFT_TOP, human_size(center.size), FontId::proportional(22.0), vis.strong_text_color());
    y += 32.0;
    ui.painter().text(pos2(x, y), Align2::LEFT_TOP, trn("%n files", center.files as usize), FontId::proportional(13.0), vis.weak_text_color());
    y += 26.0;
    // The pick.
    if let Some(n) = r.sel_node() {
        let rel = &r.sel[r.center.len()..];
        let (f0, f1) = super::frac_of(center, rel);
        let sw = Rect::from_min_size(pos2(x, y + 2.0), Vec2::splat(14.0));
        ui.painter().rect_filled(sw, 3.0, color((f0 + f1) / 2.0, rel.len(), n.kind));
        let g = elided_text(ui, &item_name(n), FontId::proportional(15.0), vis.strong_text_color(), w - 22.0);
        ui.painter().galley(pos2(x + 22.0, y), g, vis.strong_text_color());
        y += 22.0;
        let pct = 100.0 * n.size as f64 / center.size.max(1) as f64;
        ui.painter().text(pos2(x + 22.0, y), Align2::LEFT_TOP, format!("{} · {pct:.1}%", human_size(n.size)), FontId::proportional(13.5), vis.text_color());
        y += 20.0;
        ui.painter().text(pos2(x + 22.0, y), Align2::LEFT_TOP, kind_label(n), FontId::proportional(12.5), vis.weak_text_color());
        y += 28.0;
    }
    ui.painter().hline(rect.x_range().shrink(10.0), y, Stroke::new(1.0, vis.widgets.noninteractive.bg_stroke.color));
    y += 10.0;
    ui.painter().text(pos2(x, y), Align2::LEFT_TOP, tr("Biggest here"), FontId::proportional(13.0), vis.weak_text_color());
    y += 22.0;
    let top = r.sel.get(r.center.len()).copied();
    let row_h = 30.0;
    let bottom = rect.max.y - 30.0;
    for (i, c) in center.children.iter().enumerate() {
        if y + row_h > bottom {
            break;
        }
        let row = Rect::from_min_size(pos2(rect.min.x + 6.0, y), vec2(rect.width() - 12.0, row_h - 2.0));
        let resp = ui.interact(row, ui.id().with(("usage-side", i)), Sense::click());
        paint_row_bg(ui, row, top == Some(i), false, resp.hovered());
        let (f0, f1) = super::frac_of(center, &[i]);
        let col = color((f0 + f1) / 2.0, 1, c.kind);
        ui.painter().circle_filled(pos2(x + 5.0, row.center().y), 5.0, col);
        let size = human_size(c.size);
        let sg = ui.painter().layout_no_wrap(size, FontId::proportional(12.5), vis.text_color());
        let g = elided_text(ui, &item_name(c), FontId::proportional(13.5), vis.text_color(), w - sg.size().x - 30.0);
        ui.painter().galley(pos2(x + 16.0, row.center().y - g.size().y / 2.0 - 2.0), g, vis.text_color());
        ui.painter().galley(pos2(row.max.x - 8.0 - sg.size().x, row.center().y - sg.size().y / 2.0 - 2.0), sg, vis.text_color());
        let frac = c.size as f32 / center.size.max(1) as f32;
        let bar = Rect::from_min_size(pos2(x + 16.0, row.max.y - 5.0), vec2((w - 20.0) * frac, 2.5));
        ui.painter().rect_filled(bar, 1.0, col);
        if resp.clicked() {
            let mut p = r.center.clone();
            p.push(i);
            act = Some(Act::Pick(p));
        }
        if resp.double_clicked() && c.is_dir() {
            let mut p = r.center.clone();
            p.push(i);
            act = Some(Act::Zoom(p));
        }
        y += row_h;
    }
    footer(ui, pos2(x, rect.max.y - 22.0), r);
    act
}

fn footer(ui: &Ui, at: Pos2, r: &Ready) {
    let vis = ui.visuals();
    let mut s = format!("{} {:.1} {}", tr("Measured in"), r.took.as_secs_f32(), tr("sec"));
    if r.errors > 0 {
        s += &format!(" · {}", trn("%n folders unreadable", r.errors as usize));
    }
    if r.cancelled {
        s += &format!(" · {}", tr("stopped early"));
    }
    ui.painter().text(at, Align2::LEFT_TOP, s, FontId::proportional(12.0), vis.weak_text_color());
}

// ---------------------------------------------------------------- list

fn draw_list(ui: &mut Ui, area: Rect, r: &Ready, reveal: bool) -> Option<Act> {
    let vis = ui.visuals().clone();
    let mut act = None;
    let center = r.center_node();
    let top = r.sel.get(r.center.len()).copied();
    let max = center.children.first().map_or(1, |c| c.size.max(1));
    let list = Rect::from_min_max(area.min, pos2(area.max.x, area.max.y - 26.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
    egui::ScrollArea::vertical().id_salt("usage-list").auto_shrink([false, false]).show(&mut child, |ui| {
        let w = ui.available_width();
        for (i, c) in center.children.iter().enumerate() {
            let (row, resp) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click());
            let (f0, f1) = super::frac_of(center, &[i]);
            let col = color((f0 + f1) / 2.0, 1, c.kind);
            // The bar behind the row shows the size.
            let bar = Rect::from_min_size(row.min + vec2(2.0, 4.0), vec2((row.width() - 4.0) * c.size as f32 / max as f32, row.height() - 8.0));
            ui.painter().rect_filled(bar, 5.0, col.gamma_multiply(0.28));
            paint_row_bg(ui, row, top == Some(i), false, resp.hovered());
            if reveal && top == Some(i) {
                resp.scroll_to_me(Some(egui::Align::Center));
            }
            ui.painter().rect_filled(Rect::from_center_size(pos2(row.min.x + 18.0, row.center().y), Vec2::splat(14.0)), 3.0, col);
            let pct = 100.0 * c.size as f64 / center.size.max(1) as f64;
            let right = format!("{}   {pct:5.1}%", human_size(c.size));
            let rg = ui.painter().layout_no_wrap(right, FontId::monospace(13.0), vis.text_color());
            let rx = row.max.x - 12.0 - rg.size().x;
            ui.painter().galley(pos2(rx, row.center().y - rg.size().y / 2.0), rg, vis.text_color());
            let label = if c.is_dir() { format!("{}/", c.name) } else { item_name(c) };
            let g = elided_text(ui, &label, FontId::proportional(15.0), vis.strong_text_color(), (rx - row.min.x - 200.0).max(80.0));
            ui.painter().galley(pos2(row.min.x + 36.0, row.center().y - g.size().y / 2.0), g.clone(), vis.strong_text_color());
            if c.is_dir() {
                let x = row.min.x + 44.0 + g.size().x;
                ui.painter().text(pos2(x, row.center().y), Align2::LEFT_CENTER, trn("%n files", c.files as usize), FontId::proportional(12.5), vis.weak_text_color());
            }
            let mut p = r.center.clone();
            p.push(i);
            if resp.double_clicked() && c.is_dir() {
                act = Some(Act::Zoom(p));
            } else if resp.clicked() {
                act = Some(Act::Pick(p));
            } else if resp.secondary_clicked() {
                act = Some(Act::Context(p));
            }
        }
        if center.children.is_empty() {
            ui.label(tr("Empty"));
        }
    });
    footer(ui, pos2(area.min.x + 8.0, area.max.y - 20.0), r);
    act
}
