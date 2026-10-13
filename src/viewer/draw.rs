//! Drawing the viewer.

use egui::{Align2, Color32, FontFamily, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::syntax::NAMES;
use super::text::{Row, TAB};
use super::theme::Theme;
use super::{Content, ImageState, TextState, Viewer, image::Pixels};
use crate::config::ViewerPrefs;
use crate::i18n::tr;

/// Clicks in the viewer's header.
pub enum HeaderAct {
    Close,
    Settings,
    External,
}

pub fn viewer_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("viewer".into()))
}

/// The whole viewer: a header line, then the content.
pub fn draw(ui: &mut Ui, v: &mut Viewer, prefs: &ViewerPrefs, theme: &Theme) -> Option<HeaderAct> {
    let mut act = None;
    let vis = ui.visuals().clone();
    // Header: name, details, buttons.
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let p = ui.painter();
    let mut x = head.min.x + 6.0;
    let name = p.layout_no_wrap(v.name.clone(), FontId::proportional(16.0), vis.strong_text_color());
    p.galley(pos2(x, head.center().y - name.size().y / 2.0), name.clone(), vis.strong_text_color());
    x += name.size().x + 14.0;
    let details = details(v);
    p.text(pos2(x, head.center().y), Align2::LEFT_CENTER, details, FontId::proportional(13.0), vis.weak_text_color());
    let mut bx = head.max.x - 30.0;
    let mut buttons: Vec<(&str, &str, HeaderAct)> = vec![("✕", tr("Close"), HeaderAct::Close), ("↗", tr("Open in another app"), HeaderAct::External)];
    if v.is_text() {
        buttons.push(("⚙", tr("Viewer settings"), HeaderAct::Settings));
    }
    for (glyph, tip, a) in buttons {
        let r = Rect::from_min_size(pos2(bx, head.min.y + 3.0), Vec2::splat(28.0));
        let resp = ui.interact(r, ui.id().with(("viewer-btn", glyph)), Sense::click()).on_hover_text(tip);
        if resp.hovered() {
            ui.painter().rect_filled(r, 6.0, vis.widgets.hovered.weak_bg_fill);
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(16.0), vis.text_color());
        if resp.clicked() {
            act = Some(a);
        }
        bx -= 32.0;
    }
    ui.add_space(4.0);
    match &mut v.content {
        Content::Text(t) => draw_text(ui, t, prefs, theme),
        Content::Image(im) => draw_image(ui, im),
        Content::Error(e) => {
            ui.centered_and_justified(|ui| ui.label(egui::RichText::new(e.as_str()).color(vis.error_fg_color)));
        }
    }
    act
}

fn details(v: &Viewer) -> String {
    match &v.content {
        Content::Text(t) => {
            let d = &t.doc;
            let mut parts = vec![d.lang.unwrap_or(tr("Plain text")).to_string(), format!("{} {}", d.lines.len(), tr("lines"))];
            if d.crlf {
                parts.push("CRLF".into());
            }
            if d.lossy {
                parts.push(tr("not UTF-8").into());
            }
            if d.truncated {
                parts.push(tr("shown in part").into());
            }
            parts.join(" · ")
        }
        Content::Image(im) => {
            let mut parts = Vec::new();
            if im.siblings.len() > 1 {
                parts.push(format!("{}/{}", im.index + 1, im.siblings.len()));
            }
            if let Some([w, h]) = im.pic.full {
                parts.push(format!("{w}×{h}"));
            }
            if let Ok(m) = std::fs::metadata(&im.pic.path) {
                parts.push(crate::app::human_size(m.len()));
            }
            if im.pic.pixels.is_some() {
                parts.push(format!("{:.0}%", im.scale() / im.actual.max(1e-6) * 100.0));
            }
            parts.join(" · ")
        }
        Content::Error(_) => String::new(),
    }
}

/// A row as a layout job: highlight colours, tabs as spaces.
fn row_job(t: &TextState, row: Row, font: &FontId, theme: &Theme, colors: &[Color32]) -> egui::text::LayoutJob {
    let line = t.doc.line(row.line as usize);
    let (start, end) = (row.start as usize, row.end as usize);
    let mut job = egui::text::LayoutJob::default();
    let mut push = |text: &str, class: Option<u8>| {
        if text.is_empty() {
            return;
        }
        let name = class.map(|c| NAMES[c as usize]).unwrap_or("");
        let color = class.map_or(theme.fg, |c| colors[c as usize]);
        let italics = matches!(name, "comment" | "text.emphasis" | "markup.italic");
        let underline = if matches!(name, "text.uri" | "markup.link") { Stroke::new(1.0, color) } else { Stroke::NONE };
        let text = if text.contains('\t') { text.replace('\t', &" ".repeat(TAB)) } else { text.to_string() };
        job.append(&text, 0.0, egui::TextFormat { font_id: font.clone(), color, italics, underline, ..Default::default() });
    };
    let empty = Vec::new();
    let spans = t.doc.spans.get().and_then(|s| s.get(row.line as usize)).unwrap_or(&empty);
    let mut at = start;
    for s in spans.iter().filter(|s| s.end > start && s.start < end) {
        let (a, b) = (s.start.max(start), s.end.min(end));
        if a > at {
            push(&line[at..a], None);
        }
        if b > a.max(at) {
            push(&line[a.max(at)..b], Some(s.class));
        }
        at = at.max(b);
    }
    if at < end {
        push(&line[at..end], None);
    }
    job
}

fn draw_text(ui: &mut Ui, t: &mut TextState, prefs: &ViewerPrefs, theme: &Theme) {
    let font = viewer_font(prefs.font_size);
    let (char_w, font_h) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    let row_h = (font_h * 1.25).round();
    t.char_w = char_w;
    t.row_h = row_h;
    let area = ui.available_rect_before_wrap();
    ui.painter().rect_filled(area, 6.0, theme.bg);
    let digits = t.doc.lines.len().to_string().len() as f32;
    let gutter = if prefs.line_numbers { digits * char_w + 28.0 } else { 14.0 };
    let cols = prefs.wrap.then(|| ((area.width() - gutter - 24.0) / char_w).floor().max(8.0) as usize);
    if t.rows_for != Some(cols) {
        // Keep the same line at the top across re-wrapping.
        let top_line = t.rows.get((t.scroll.y / row_h) as usize).map(|r| r.line);
        t.rows = super::text::rows(&t.doc, cols);
        t.rows_for = Some(cols);
        if let Some(l) = top_line {
            let r = t.rows.iter().position(|r| r.line == l).unwrap_or(0);
            t.scroll_to = Some(vec2(0.0, r as f32 * row_h));
        }
    }
    let colors = theme.table();
    let content_w = if prefs.wrap { area.width() - 16.0 } else { gutter + t.doc.widest as f32 * char_w + 40.0 };
    let content_h = t.rows.len() as f32 * row_h + row_h;
    let mut sa = egui::ScrollArea::both().id_salt("viewer-text").auto_shrink([false, false]);
    if let Some(s) = t.scroll_to.take() {
        sa = sa.scroll_offset(s);
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    let out = sa.show_viewport(&mut child, |ui, vp| {
        ui.set_min_size(vec2(content_w, content_h));
        let origin = ui.min_rect().min;
        let first = (vp.min.y / row_h).floor().max(0.0) as usize;
        let last = ((vp.max.y / row_h).ceil() as usize + 1).min(t.rows.len());
        let painter = ui.painter().clone();
        // The line under the mouse.
        if let Some(p) = ui.ctx().pointer_hover_pos().filter(|p| ui.clip_rect().contains(*p)) {
            let hl = ((p.y - origin.y) / row_h).floor() as usize;
            if let Some(line) = t.rows.get(hl).map(|r| r.line) {
                let (mut a, mut b) = (hl, hl);
                while a > 0 && t.rows[a - 1].line == line {
                    a -= 1;
                }
                while b + 1 < t.rows.len() && t.rows[b + 1].line == line {
                    b += 1;
                }
                let r = Rect::from_min_max(pos2(origin.x + vp.min.x, origin.y + a as f32 * row_h), pos2(origin.x + vp.max.x, origin.y + (b + 1) as f32 * row_h));
                painter.rect_filled(r, 0.0, theme.line);
            }
        }
        for i in first..last {
            let row = t.rows[i];
            let y = origin.y + i as f32 * row_h;
            let g = painter.layout_job(row_job(t, row, &font, theme, &colors));
            painter.galley(pos2(origin.x + gutter, y + (row_h - font_h) / 2.0), g, theme.fg);
        }
        // Line numbers stay put when scrolling sideways.
        let gx = origin.x + vp.min.x;
        let strip = Rect::from_min_max(pos2(gx, origin.y + vp.min.y), pos2(gx + gutter - 8.0, origin.y + vp.max.y));
        painter.rect_filled(strip, 0.0, theme.bg);
        if prefs.line_numbers {
            let nf = FontId::new(prefs.font_size * 0.85, font.family.clone());
            for i in first..last {
                let row = t.rows[i];
                if row.start == 0 {
                    let y = origin.y + i as f32 * row_h + row_h / 2.0;
                    painter.text(pos2(gx + gutter - 16.0, y), Align2::RIGHT_CENTER, (row.line + 1).to_string(), nf.clone(), theme.gutter);
                }
            }
            painter.vline(gx + gutter - 8.0, strip.y_range(), Stroke::new(1.0, theme.gutter.gamma_multiply(0.3)));
        }
    });
    t.scroll = out.state.offset;
    t.page_rows = ((out.inner_rect.height() / row_h) as usize).saturating_sub(1).max(1);
    ui.advance_cursor_after_rect(area);
}

fn draw_image(ui: &mut Ui, im: &mut ImageState) {
    let ctx = ui.ctx().clone();
    im.pic.poll(&ctx);
    let area = ui.available_rect_before_wrap();
    ui.advance_cursor_after_rect(area);
    ui.painter().rect_filled(area, 6.0, Color32::from_gray(16));
    let resp = ui.interact(area, ui.id().with("image-view"), Sense::click_and_drag());
    if let Some(e) = &im.pic.error {
        ui.painter().text(area.center(), Align2::CENTER_CENTER, e, FontId::proportional(15.0), ui.visuals().error_fg_color);
        return;
    }
    // The texture and its size in texture pixels.
    let (tex, tex_size) = match &im.pic.pixels {
        Some(Pixels::Texture(t)) => (Some(egui::load::SizedTexture::from_handle(t)), t.size_vec2()),
        Some(Pixels::Uri(u)) => match egui::Image::new(u.clone()).load_for_size(&ctx, area.size()) {
            Ok(egui::load::TexturePoll::Ready { texture }) => (Some(texture), texture.size),
            Ok(_) => (None, Vec2::ZERO),
            Err(e) => {
                ui.painter().text(area.center(), Align2::CENTER_CENTER, e.to_string(), FontId::proportional(15.0), ui.visuals().error_fg_color);
                return;
            }
        },
        None => (None, Vec2::ZERO),
    };
    if im.pic.path.to_string_lossy().to_lowercase().ends_with(".gif") {
        // egui's loader steps animated GIFs by time.
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
    }
    let Some(tex) = tex.filter(|_| tex_size.x > 0.0) else {
        ui.put(Rect::from_center_size(area.center(), Vec2::splat(32.0)), egui::Spinner::new());
        return;
    };
    let ppp = ctx.pixels_per_point();
    let full_w = im.pic.full.map_or(tex_size.x, |f| f[0] as f32);
    // Points per texture pixel that show the picture 1:1 on screen.
    im.actual = full_w / tex_size.x / ppp;
    let room = area.shrink(8.0).size();
    im.fit = (room.x / tex_size.x).min(room.y / tex_size.y).min(im.actual);
    im.view = area.size();

    // Mouse: wheel zooms at the pointer, drag pans, double click 1:1 ↔ fit.
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 300.0);
        if scroll.abs() > 0.5 {
            let k = (scroll / 300.0).exp();
            let before = im.scale();
            if let Some(p) = resp.hover_pos() {
                let rel = p - area.center() - im.pan;
                im.zoom_by(k);
                let after = im.scale();
                im.pan -= rel * (after / before - 1.0);
                if im.zoom.is_none() {
                    im.pan = Vec2::ZERO;
                }
            }
        }
    }
    if resp.dragged() && im.zoom.is_some() {
        im.pan += resp.drag_delta();
    }
    if resp.double_clicked() {
        im.toggle_actual();
    }
    let size = tex_size * im.scale();
    // Keep at least part of the picture on screen.
    let lim = ((size - area.size()) / 2.0).max(Vec2::ZERO) + Vec2::splat(40.0);
    im.pan = im.pan.clamp(-lim, lim);
    let rect = Rect::from_center_size(area.center() + im.pan, size);
    let painter = ui.painter().with_clip_rect(area);
    // Checkerboard under transparent pictures.
    let cell = 12.0;
    let shown = rect.intersect(area);
    if shown.width() * shown.height() < 4.0e6 {
        let (x0, y0) = (shown.min.x - (shown.min.x - rect.min.x).rem_euclid(cell * 2.0), shown.min.y - (shown.min.y - rect.min.y).rem_euclid(cell * 2.0));
        let mut y = y0;
        let mut row = 0;
        while y < shown.max.y {
            let mut x = x0 + if row % 2 == 1 { cell } else { 0.0 };
            while x < shown.max.x {
                painter.rect_filled(Rect::from_min_size(pos2(x, y), Vec2::splat(cell)).intersect(shown), 0.0, Color32::from_gray(40));
                x += cell * 2.0;
            }
            y += cell;
            row += 1;
        }
    }
    painter.image(tex.id, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}

/// A Rust sample for the settings preview.
pub const SAMPLE: &str = "// Polka: просмотр текста\nfn main() {\n    let name = \"мир\";\n    println!(\"Привет, {name}!\");\n}\n";

/// Sidebar of viewer settings, drawn next to the open file.
pub fn draw_settings(ui: &mut Ui, st: &crate::app::ViewerSettings, prefs: &ViewerPrefs) -> Option<(usize, isize)> {
    use crate::app::vrow;
    let mut ev = None;
    let vis = ui.visuals().clone();
    let w = 380.0;
    ui.set_width(w);
    ui.label(egui::RichText::new(tr("Viewer settings")).strong().size(18.0));
    ui.add_space(6.0);
    ui.spacing_mut().item_spacing.y = 4.0;
    let font_name = prefs.font.clone().unwrap_or_else(|| tr("System monospace").to_string());
    let rows: [(usize, &str, String, bool); 5] = [
        (vrow::THEME, tr("Theme"), prefs.theme.clone(), true),
        (vrow::FONT, tr("Font"), font_name, true),
        (vrow::SIZE, tr("Size"), format!("{:.0}", prefs.font_size), true),
        (vrow::WRAP, tr("Wrap lines"), String::new(), false),
        (vrow::NUMBERS, tr("Line numbers"), String::new(), false),
    ];
    for (r_i, label, value, arrows) in rows {
        let (rect, r) = ui.allocate_exact_size(vec2(w, 42.0), Sense::click());
        if st.row == r_i {
            ui.painter().rect(rect, 8.0, vis.selection.bg_fill.gamma_multiply(0.25), Stroke::new(1.5, vis.selection.bg_fill), StrokeKind::Inside);
        } else if r.hovered() {
            ui.painter().rect_filled(rect, 8.0, vis.widgets.hovered.weak_bg_fill.gamma_multiply(0.5));
        }
        if r.clicked() {
            ev = Some((r_i, 0));
        }
        ui.painter().text(pos2(rect.min.x + 12.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(15.0), vis.text_color());
        if arrows {
            let right = Rect::from_center_size(pos2(rect.max.x - 22.0, rect.center().y), Vec2::splat(28.0));
            let left = Rect::from_center_size(pos2(rect.max.x - 212.0, rect.center().y), Vec2::splat(28.0));
            for (b, g, d) in [(left, "◀", -1), (right, "▶", 1)] {
                let br = ui.interact(b, ui.id().with(("vstep", r_i, d)), Sense::click());
                ui.painter().rect_filled(b, 6.0, if br.hovered() { vis.widgets.hovered.bg_fill } else { vis.widgets.inactive.bg_fill });
                ui.painter().text(b.center(), Align2::CENTER_CENTER, g, FontId::proportional(12.0), vis.text_color());
                if br.clicked() {
                    ev = Some((r_i, d));
                }
            }
            let mid = pos2((left.max.x + right.min.x) / 2.0, rect.center().y);
            let max_w = right.min.x - left.max.x - 8.0;
            let g = crate::view::elided_text(ui, &value, FontId::proportional(14.0), vis.strong_text_color(), max_w);
            ui.painter().galley(mid - g.size() / 2.0, g, vis.strong_text_color());
        } else {
            let on = if r_i == vrow::WRAP { prefs.wrap } else { prefs.line_numbers };
            let b = Rect::from_center_size(pos2(rect.max.x - 22.0, rect.center().y), Vec2::splat(20.0));
            ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, vis.strong_text_color()), StrokeKind::Inside);
            if on {
                ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), vis.strong_text_color());
            }
        }
    }
    // Preview in the chosen theme and font.
    ui.add_space(6.0);
    let theme = super::theme::by_name(&prefs.theme);
    let font = viewer_font(prefs.font_size);
    let no = std::sync::atomic::AtomicUsize::new(0);
    let spans = super::syntax::highlight("Rust", SAMPLE, &no).unwrap_or_default();
    let colors = theme.table();
    let mut job = egui::text::LayoutJob::default();
    let mut at = 0;
    for s in &spans {
        if s.start > at {
            job.append(&SAMPLE[at..s.start], 0.0, egui::TextFormat { font_id: font.clone(), color: theme.fg, ..Default::default() });
        }
        if s.end > at.max(s.start) {
            let italics = NAMES[s.class as usize] == "comment";
            job.append(&SAMPLE[at.max(s.start)..s.end], 0.0, egui::TextFormat { font_id: font.clone(), color: colors[s.class as usize], italics, ..Default::default() });
        }
        at = at.max(s.end);
    }
    job.append(SAMPLE[at..].trim_end(), 0.0, egui::TextFormat { font_id: font.clone(), color: theme.fg, ..Default::default() });
    job.wrap.max_width = w - 24.0;
    let g = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(vec2(w, g.size().y + 20.0), Sense::hover());
    ui.painter().rect_filled(rect, 8.0, theme.bg);
    ui.painter().galley(rect.min + vec2(12.0, 10.0), g, theme.fg);
    ui.add_space(6.0);
    let (rect, r) = ui.allocate_exact_size(vec2(w, 38.0), Sense::click());
    let focused = st.row == vrow::CLOSE;
    let fill = if focused { vis.selection.bg_fill.gamma_multiply(0.5) } else if r.hovered() { vis.widgets.hovered.weak_bg_fill } else { vis.widgets.inactive.weak_bg_fill };
    ui.painter().rect(rect, 8.0, fill, if focused { Stroke::new(1.5, vis.selection.bg_fill) } else { Stroke::NONE }, StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, tr("Close"), FontId::proportional(15.0), vis.text_color());
    if r.clicked() {
        ev = Some((vrow::CLOSE, 0));
    }
    ev
}
