//! Drawing the viewer.

use egui::{Align2, Color32, FontFamily, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::syntax::NAMES;
use super::text::{Row, TAB};
use super::theme::Theme;
use super::{Content, ImageState, TextState, Viewer, image::Pixels};
use crate::config::ViewerPrefs;
use crate::i18n::tr;

/// Clicks in the viewer.
pub enum HeaderAct {
    Close,
    Settings,
    External,
    Save,
    ToggleMd,
    /// A link in rendered Markdown.
    Link(String),
    /// Player buttons.
    Run(crate::app::Cmd),
    Volume(f32),
}

pub fn viewer_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("viewer".into()))
}

/// Is the Markdown drawn rendered right now?
pub fn md_rendered(t: &TextState, prefs: &ViewerPrefs) -> bool {
    t.doc.lang == Some("Markdown") && prefs.md_rendered && t.ed.find.is_none() && !t.ed.editing
}

fn header_button(ui: &mut Ui, r: Rect, label: &str, tip: &str, on: bool) -> bool {
    let vis = ui.visuals().clone();
    let resp = ui.interact(r, ui.id().with(("viewer-btn", label, r.min.x as i32, r.min.y as i32)), Sense::click()).on_hover_text(tip);
    if on {
        ui.painter().rect_filled(r, 6.0, vis.selection.bg_fill.gamma_multiply(0.45));
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 6.0, vis.widgets.hovered.weak_bg_fill);
    }
    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(15.0), vis.text_color());
    resp.clicked()
}

/// The whole viewer: a header line, then the content.
pub fn draw(ui: &mut Ui, v: &mut Viewer, prefs: &ViewerPrefs, theme: &Theme) -> Option<HeaderAct> {
    let mut act = None;
    let vis = ui.visuals().clone();
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let mut x = head.min.x + 6.0;
    let dirty = matches!(&v.content, Content::Text(t) if t.ed.dirty);
    let title = if dirty { format!("● {}", v.name) } else { v.name.clone() };
    let name = ui.painter().layout_no_wrap(title, FontId::proportional(16.0), vis.strong_text_color());
    ui.painter().galley(pos2(x, head.center().y - name.size().y / 2.0), name.clone(), vis.strong_text_color());
    x += name.size().x + 14.0;
    ui.painter().text(pos2(x, head.center().y), Align2::LEFT_CENTER, details(v), FontId::proportional(13.0), vis.weak_text_color());
    let mut bx = head.max.x - 30.0;
    let btn = |bx: f32, w: f32| Rect::from_min_size(pos2(bx, head.min.y + 3.0), vec2(w, 28.0));
    if header_button(ui, btn(bx, 28.0), "✕", tr("Close"), false) {
        act = Some(HeaderAct::Close);
    }
    bx -= 32.0;
    if header_button(ui, btn(bx, 28.0), "↗", tr("Open in another app"), false) {
        act = Some(HeaderAct::External);
    }
    if let Content::Text(t) = &v.content {
        bx -= 32.0;
        if header_button(ui, btn(bx, 28.0), "⚙", tr("Viewer settings"), false) {
            act = Some(HeaderAct::Settings);
        }
        if dirty {
            let w = 110.0;
            bx -= w + 4.0;
            if header_button(ui, btn(bx, w), tr("Save"), "Ctrl+S", true) {
                act = Some(HeaderAct::Save);
            }
        }
        if t.doc.lang == Some("Markdown") {
            // GitHub view ↔ source.
            let rendered = md_rendered(t, prefs);
            for (label, on) in [(tr("Source"), !rendered), (tr("Rendered"), rendered)] {
                let w = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(15.0), vis.text_color()).size().x + 20.0;
                bx -= w + 4.0;
                if header_button(ui, btn(bx, w), label, tr("Markdown: as on GitHub or as source"), on) && !on {
                    act = Some(HeaderAct::ToggleMd);
                }
            }
        }
    }
    ui.add_space(4.0);
    match &mut v.content {
        Content::Text(t) => {
            if t.ed.find.is_some() {
                draw_find(ui, t, theme);
            }
            if md_rendered(t, prefs) {
                if let Some(l) = draw_md(ui, t, prefs, theme, v.path.parent().unwrap_or(std::path::Path::new("/"))) {
                    act = Some(HeaderAct::Link(l));
                }
            } else {
                draw_text(ui, t, prefs, theme);
            }
        }
        Content::Image(im) => draw_image(ui, im),
        Content::Audio(a) => {
            if let Some(x) = draw_audio(ui, a, prefs) {
                act = Some(x);
            }
        }
        Content::Error(e) => {
            ui.centered_and_justified(|ui| ui.label(egui::RichText::new(e.as_str()).color(vis.error_fg_color)));
        }
    }
    act
}

/// Search bar: query, options, count, arrows.
fn draw_find(ui: &mut Ui, t: &mut TextState, theme: &Theme) {
    let vis = ui.visuals().clone();
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
    ui.add_space(4.0);
    let Some(f) = t.ed.find.as_mut() else { return };
    let field = Rect::from_min_size(bar.min + vec2(4.0, 3.0), vec2((bar.width() * 0.45).clamp(200.0, 520.0), 30.0));
    let fr = ui.interact(field, ui.id().with("find-field"), Sense::click());
    if fr.clicked() {
        f.typing = true;
    }
    let border = if f.error.is_some() { vis.error_fg_color } else if f.typing { vis.selection.bg_fill } else { vis.widgets.noninteractive.bg_stroke.color };
    ui.painter().rect(field, 6.0, vis.extreme_bg_color, Stroke::new(1.5, border), StrokeKind::Inside);
    let font = FontId::proportional(15.0);
    let painter = ui.painter().with_clip_rect(field.shrink(2.0));
    let (shown, color) = if f.query.is_empty() && !f.typing { (tr("Search…").to_string(), vis.weak_text_color()) } else { (f.query.clone(), vis.text_color()) };
    let g = painter.layout_no_wrap(shown, font.clone(), color);
    let tx = field.min.x + 10.0 - (g.size().x - field.width() + 30.0).max(0.0);
    painter.galley(pos2(tx, field.center().y - g.size().y / 2.0), g.clone(), color);
    if f.typing {
        let cx = tx + if f.query.is_empty() { 0.0 } else { g.size().x } + 1.0;
        painter.vline(cx, (field.center().y - 9.0)..=(field.center().y + 9.0), Stroke::new(1.5, vis.text_color()));
    }
    let mut x = field.max.x + 8.0;
    let mut toggles: Vec<(&str, &str, bool, u8)> = vec![("Aa", tr("Match case"), f.case, 0), ("W", tr("Whole words"), f.word, 1), (".*", tr("Regular expression"), f.regex, 2)];
    let mut clicked = None;
    for (label, tip, on, id) in toggles.drain(..) {
        let r = Rect::from_min_size(pos2(x, bar.min.y + 4.0), vec2(34.0, 28.0));
        if header_button(ui, r, label, tip, on) {
            clicked = Some(id);
        }
        x += 38.0;
    }
    if let Some(id) = clicked {
        let f = t.ed.find.as_mut().unwrap();
        match id {
            0 => f.case = !f.case,
            1 => f.word = !f.word,
            _ => f.regex = !f.regex,
        }
    }
    t.find_refresh();
    let f = t.ed.find.as_ref().unwrap();
    let status = match (&f.error, f.matches.len(), f.current) {
        (Some(e), ..) => e.clone(),
        (None, 0, _) if !f.query.is_empty() => tr("No matches").to_string(),
        (None, 0, _) => String::new(),
        (None, n, Some(c)) => format!("{} {} {}{}", c + 1, tr("of"), n, if n >= super::edit::MAX_MATCHES { "+" } else { "" }),
        (None, n, None) => n.to_string(),
    };
    let sc = if f.error.is_some() { vis.error_fg_color } else { vis.weak_text_color() };
    ui.painter().text(pos2(x + 6.0, bar.center().y), Align2::LEFT_CENTER, status, FontId::proportional(14.0), sc);
    let mut rx = bar.max.x - 30.0;
    let mut step = None;
    for (label, tip, d) in [("✕", tr("Close"), 0), ("↓", tr("Next"), 1), ("↑", tr("Previous"), -1)] {
        if header_button(ui, Rect::from_min_size(pos2(rx, bar.min.y + 4.0), vec2(28.0, 28.0)), label, tip, false) {
            step = Some(d);
        }
        rx -= 32.0;
    }
    match step {
        Some(0) => t.ed.find = None,
        Some(d) => t.find_step(d),
        None => {}
    }
    let _ = theme;
}

/// Rendered Markdown; returns a clicked link.
fn draw_md(ui: &mut Ui, t: &mut TextState, prefs: &ViewerPrefs, theme: &Theme, dir: &std::path::Path) -> Option<String> {
    if t.md.as_ref().is_none_or(|(g, _)| *g != t.doc.generation) {
        t.md = Some((t.doc.generation, super::md::parse(&t.doc.text)));
        t.anchors.clear();
    }
    let area = ui.available_rect_before_wrap();
    ui.painter().rect_filled(area, 6.0, theme.bg);
    let mut sa = egui::ScrollArea::vertical().id_salt("viewer-md").auto_shrink([false, false]);
    if let Some(y) = t.md_scroll_to.take() {
        sa = sa.vertical_scroll_offset(y.max(0.0));
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink2(vec2(0.0, 2.0))));
    let mut click = None;
    let (doc, anchors) = (&t.md.as_ref().unwrap().1, &mut t.anchors);
    let out = sa.show(&mut child, |ui| {
        let avail = ui.available_width();
        let w = (avail - 64.0).min(900.0);
        let top = ui.min_rect().min.y;
        ui.add_space(20.0);
        ui.horizontal_top(|ui| {
            ui.add_space(((avail - w) / 2.0).max(16.0));
            ui.vertical(|ui| {
                ui.set_width(w);
                let mut c = super::md::Ctx { theme, base: prefs.font_size + 1.0, mono: prefs.font_size * 0.92, dir, anchors, top, click: None };
                super::md::show(ui, doc, &mut c);
                click = c.click.map(|super::md::Click::Link(l)| l);
            });
        });
    });
    t.md_scroll = out.state.offset.y;
    t.page_rows = ((out.inner_rect.height() / (prefs.font_size * 1.6)) as usize).max(1);
    ui.advance_cursor_after_rect(area);
    click
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
            if !im.pic.cursor_sizes.is_empty() {
                let sizes: Vec<String> = im.pic.cursor_sizes.iter().map(|s| s.to_string()).collect();
                parts.push(format!("{}: {}", tr("cursor sizes"), sizes.join(", ")));
            }
            if im.pic.frames.len() > 1 {
                parts.push(format!("{} {}", im.pic.frames.len(), tr("frames")));
            }
            if let Ok(m) = std::fs::metadata(&im.pic.path) {
                parts.push(crate::app::human_size(m.len()));
            }
            if im.pic.pixels.is_some() {
                parts.push(format!("{:.0}%", im.scale() / im.actual.max(1e-6) * 100.0));
            }
            parts.join(" · ")
        }
        Content::Audio(a) => {
            let m = &a.meta;
            let mut parts = Vec::new();
            if a.tracks.len() > 1 {
                parts.push(format!("{}/{}", a.index + 1, a.tracks.len()));
            }
            if let Some(ext) = v.name.rsplit_once('.').map(|(_, e)| e.to_uppercase()) {
                parts.push(ext);
            }
            if let Some(r) = m.rate {
                parts.push(format!("{} {}", trim_num(r as f32 / 1000.0), tr("kHz")));
            }
            if let Some(b) = m.bits {
                parts.push(format!("{b} {}", tr("bit")));
            }
            match m.channels {
                Some(1) => parts.push(tr("mono").into()),
                Some(2) => parts.push(tr("stereo").into()),
                Some(c) => parts.push(format!("{c} {}", tr("ch."))),
                None => {}
            }
            if let Some(b) = m.bitrate {
                parts.push(format!("{b} {}", tr("kbit/s")));
            }
            if let Ok(md) = std::fs::metadata(a.path()) {
                parts.push(crate::app::human_size(md.len()));
            }
            parts.join(" · ")
        }
        Content::Error(_) => String::new(),
    }
}

/// 44.1, 48.
fn trim_num(x: f32) -> String {
    let s = format!("{x:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
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
    let spans = t.doc.spans.get(row.line as usize).unwrap_or(&empty);
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
    if let Some(wait) = t.doc.poll_highlight() {
        ui.ctx().request_repaint_after(wait);
    }
    t.find_refresh();
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
        let rewrap = t.rows_for.is_some_and(|c| c != cols);
        let top_line = t.rows.get((t.scroll.y / row_h) as usize).map(|r| r.line);
        t.rows = super::text::rows(&t.doc, cols);
        t.rows_for = Some(cols);
        if let (true, Some(l)) = (rewrap, top_line) {
            let r = t.rows.iter().position(|r| r.line == l).unwrap_or(0);
            t.scroll_to = Some(vec2(0.0, r as f32 * row_h));
        }
    }
    let view_h = area.height();
    // Keep the caret on screen after it moved.
    if t.ed.reveal {
        t.ed.reveal = false;
        let row = t.row_of(t.ed.caret) as f32;
        let mut s = t.scroll_to.unwrap_or(t.scroll);
        if row * row_h < s.y {
            s.y = row * row_h;
        } else if (row + 2.0) * row_h > s.y + view_h {
            s.y = (row + 2.0) * row_h - view_h;
        }
        if !prefs.wrap {
            let x = t.col_of(t.ed.caret) as f32 * char_w;
            let room = area.width() - gutter - 30.0;
            if x < s.x {
                s.x = (x - room / 3.0).max(0.0);
            } else if x > s.x + room {
                s.x = x - room * 2.0 / 3.0;
            }
        }
        if s != t.scroll {
            t.scroll_to = Some(s);
        }
    }
    let colors = theme.table();
    let content_w = if prefs.wrap { area.width() - 16.0 } else { gutter + t.doc.widest as f32 * char_w + 40.0 };
    let content_h = t.rows.len() as f32 * row_h + row_h;
    let mut sa = egui::ScrollArea::both().id_salt("viewer-text").auto_shrink([false, false]);
    if let Some(s) = t.scroll_to.take() {
        sa = sa.scroll_offset(s);
    }
    let sel = t.ed.selection();
    let sel_c = ui.visuals().selection.bg_fill.gamma_multiply(0.55);
    let match_c = Color32::from_rgba_unmultiplied(0xe5, 0xb5, 0x3a, 70);
    let current_c = Color32::from_rgba_unmultiplied(0xff, 0x9f, 0x1a, 150);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    let out = sa.show_viewport(&mut child, |ui, vp| {
        let (content, resp) = ui.allocate_exact_size(vec2(content_w.max(vp.width()), content_h.max(vp.height())), Sense::click_and_drag());
        let origin = content.min;
        let first = (vp.min.y / row_h).floor().max(0.0) as usize;
        let last = ((vp.max.y / row_h).ceil() as usize + 1).min(t.rows.len());
        let painter = ui.painter().clone();
        // Mouse: click places the caret, drag selects, double click a word.
        let at_pos = |t: &TextState, p: egui::Pos2| {
            let row = (((p.y - origin.y) / row_h).floor().max(0.0) as usize).min(t.rows.len().saturating_sub(1));
            let col = ((p.x - origin.x - gutter) / char_w).round().max(0.0) as usize;
            t.at_col(row, col)
        };
        if let Some(p) = resp.interact_pointer_pos() {
            let at = at_pos(t, p);
            let shift = ui.input(|i| i.modifiers.shift);
            if resp.double_clicked() {
                let w = t.word_at(at);
                t.ed.anchor = Some(w.start);
                t.ed.caret = w.end;
            } else if resp.drag_started() || (resp.clicked() && !shift) {
                if shift {
                    t.ed.anchor.get_or_insert(t.ed.caret);
                } else {
                    t.ed.anchor = Some(at);
                }
                t.ed.caret = at;
            } else if resp.dragged() || (resp.clicked() && shift) {
                t.ed.anchor.get_or_insert(t.ed.caret);
                t.ed.caret = at;
            }
            if let Some(f) = t.ed.find.as_mut() {
                f.typing = false;
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        // The line under the mouse.
        if let Some(p) = resp.hover_pos() {
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
        // Background ranges (selection, matches) on a row.
        let x_of = |t: &TextState, row: Row, in_line: usize| -> f32 {
            let line = t.doc.line(row.line as usize);
            origin.x + gutter + super::text::columns(&line[row.start as usize..in_line.clamp(row.start as usize, row.end as usize)]) as f32 * char_w
        };
        let paint_range = |t: &TextState, i: usize, r: &std::ops::Range<usize>, c: Color32| {
            let row = t.rows[i];
            let base = t.doc.lines[row.line as usize].0;
            let (rs, re) = (base + row.start as usize, base + row.end as usize);
            // A range running past the line end shows a little extra.
            let eol = row.end as usize == t.doc.line(row.line as usize).len();
            if r.end < rs || r.start > re || (r.start == re && !eol) || (r.end == rs && r.start < rs) {
                return;
            }
            let a = x_of(t, row, r.start.max(rs) - base);
            let mut b = x_of(t, row, r.end.min(re) - base);
            if eol && r.end > re {
                b += char_w * 0.6;
            }
            if b > a {
                let y = origin.y + i as f32 * row_h;
                painter.rect_filled(Rect::from_min_max(pos2(a, y), pos2(b, y + row_h)), 2.0, c);
            }
        };
        if let Some(f) = &t.ed.find {
            let (vs, ve) = (t.rows.get(first).map(|r| t.doc.lines[r.line as usize].0).unwrap_or(0), t.rows.get(last.saturating_sub(1)).map(|r| t.doc.lines[r.line as usize].1).unwrap_or(0));
            let from = f.matches.partition_point(|m| m.end < vs);
            for (k, m) in f.matches[from..].iter().enumerate().take_while(|(_, m)| m.start <= ve) {
                let c = if f.current == Some(from + k) { current_c } else { match_c };
                for i in first..last {
                    paint_range(t, i, m, c);
                }
            }
        }
        if let Some(s) = &sel {
            for i in first..last {
                paint_range(t, i, s, sel_c);
            }
        }
        for i in first..last {
            let row = t.rows[i];
            let y = origin.y + i as f32 * row_h;
            let g = painter.layout_job(row_job(t, row, &font, theme, &colors));
            painter.galley(pos2(origin.x + gutter, y + (row_h - font_h) / 2.0), g, theme.fg);
        }
        // Caret.
        let crow = t.row_of(t.ed.caret);
        if (first..last).contains(&crow) {
            let base = t.doc.lines[t.rows[crow].line as usize].0;
            let x = x_of(t, t.rows[crow], t.ed.caret - base);
            let y = origin.y + crow as f32 * row_h;
            let w = if t.ed.editing { 2.5 } else { 1.5 };
            painter.rect_filled(Rect::from_min_size(pos2(x - w / 2.0, y + 2.0), vec2(w, row_h - 4.0)), 0.0, theme.fg);
        }
        // Line numbers stay put when scrolling sideways.
        let gx = origin.x + vp.min.x;
        let strip = Rect::from_min_max(pos2(gx, origin.y + vp.min.y), pos2(gx + gutter - 8.0, origin.y + vp.max.y));
        painter.rect_filled(strip, 0.0, theme.bg);
        if prefs.line_numbers {
            let nf = FontId::new(prefs.font_size * 0.85, font.family.clone());
            let cur_line = t.doc.line_of(t.ed.caret) as u32;
            for i in first..last {
                let row = t.rows[i];
                if row.start == 0 {
                    let y = origin.y + i as f32 * row_h + row_h / 2.0;
                    let c = if row.line == cur_line { theme.fg } else { theme.gutter };
                    painter.text(pos2(gx + gutter - 16.0, y), Align2::RIGHT_CENTER, (row.line + 1).to_string(), nf.clone(), c);
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
    im.fit = (room.x / tex_size.x).min(room.y / tex_size.y);
    im.fit = if im.pic.hotspot.is_some() {
        // Cursors are tiny: blow them up by whole steps (crisp pixels).
        (im.fit * ppp).floor().clamp(1.0, 8.0) / ppp
    } else {
        im.fit.min(im.actual)
    };
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
    // A cursor's hotspot: where the click lands.
    if let Some((hx, hy)) = im.pic.hotspot {
        let k = im.scale();
        let p = rect.min + vec2(hx as f32 + 0.5, hy as f32 + 0.5) * k;
        let c = Color32::from_rgb(0xff, 0x40, 0x40);
        painter.line_segment([p - vec2(8.0, 0.0), p + vec2(8.0, 0.0)], Stroke::new(1.5, c));
        painter.line_segment([p - vec2(0.0, 8.0), p + vec2(0.0, 8.0)], Stroke::new(1.5, c));
    }
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

// ------------------------------------------------------------ music

/// Player: cover, tags, progress, buttons, volume; the playlist beside
/// (or below, when narrow).
fn draw_audio(ui: &mut Ui, a: &mut super::audio::AudioState, prefs: &ViewerPrefs) -> Option<HeaderAct> {
    use super::audio::{Repeat, clock};
    use crate::app::Cmd;
    let mut act = None;
    let vis = ui.visuals().clone();
    let accent = vis.selection.bg_fill;
    let area = ui.available_rect_before_wrap();
    ui.advance_cursor_after_rect(area);
    let wide = area.width() > 820.0;
    let (info, list) = if wide {
        let lw = (area.width() * 0.34).clamp(260.0, 440.0);
        let w = area.width() - lw - 12.0;
        (Rect::from_min_size(area.min, vec2(w, area.height())), Rect::from_min_max(pos2(area.min.x + w + 12.0, area.min.y), area.max))
    } else {
        let h = (area.height() * 0.62).max(300.0).min(area.height());
        (Rect::from_min_size(area.min, vec2(area.width(), h)), Rect::from_min_max(pos2(area.min.x, area.min.y + h + 8.0), area.max))
    };
    let painter = ui.painter().clone();
    painter.rect_filled(info, 8.0, vis.faint_bg_color);
    let pad = 24.0;
    // Cover and tags, then the controls; the block centred vertically.
    let controls_h = 130.0;
    let room = Rect::from_min_max(info.min + vec2(pad, pad), pos2(info.max.x - pad, info.max.y - controls_h - pad));
    let side = room.height().min(room.width() * 0.45).clamp(64.0, 420.0);
    let y0 = room.min.y + ((info.height() - 2.0 * pad - side - controls_h - 16.0) / 2.0).max(0.0);
    let top = Rect::from_min_max(pos2(room.min.x, y0), pos2(room.max.x, y0 + side));
    let cover = Rect::from_min_size(top.min, Vec2::splat(side));
    match &a.cover {
        Some(t) => {
            let s = t.size_vec2();
            let k = (side / s.x).min(side / s.y);
            let r = Rect::from_center_size(cover.center(), s * k);
            egui::Image::new(egui::load::SizedTexture::from_handle(t)).corner_radius(8.0).paint_at(ui, r);
        }
        None => {
            painter.rect_filled(cover, 8.0, accent.gamma_multiply(0.22));
            note_glyph(&painter, cover.center(), side * 0.42, accent.gamma_multiply(0.9));
        }
    }
    // Tags.
    let m = &a.meta;
    let tx = cover.max.x + 24.0;
    let tw = (top.max.x - tx).max(80.0);
    let stem = a.path().file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut y = top.min.y + 4.0;
    let line = |text: String, size: f32, color: Color32, y: &mut f32| {
        let g = painter.layout(text, FontId::proportional(size), color, tw);
        painter.galley(pos2(tx, *y), g.clone(), color);
        *y += g.size().y + 6.0;
    };
    line(m.title.clone().unwrap_or(stem), 26.0, vis.strong_text_color(), &mut y);
    if let Some(ar) = &m.artist {
        line(ar.clone(), 19.0, vis.text_color(), &mut y);
    }
    let mut album = m.album.clone().unwrap_or_default();
    if let Some(yr) = m.year {
        album = if album.is_empty() { yr.to_string() } else { format!("{album} ({yr})") };
    }
    if !album.is_empty() {
        line(album, 16.0, vis.weak_text_color(), &mut y);
    }
    let mut extra = Vec::new();
    if let Some(n) = m.track {
        extra.push(format!("{} {n}", tr("Track")));
    }
    if let Some(g) = &m.genre {
        extra.push(g.clone());
    }
    if !extra.is_empty() {
        line(extra.join(" · "), 14.0, vis.weak_text_color(), &mut y);
    }
    match &a.play {
        Err(e) if !e.is_empty() => line(e.clone(), 15.0, vis.error_fg_color, &mut y),
        Ok(p) if p.no_device.is_some() => line(format!("{}: {}", tr("No sound output"), p.no_device.as_deref().unwrap_or("")), 14.0, vis.error_fg_color, &mut y),
        _ => {}
    }

    // Progress: click or drag to seek.
    let pos = a.pos();
    let len = a.length();
    let frac = len.map_or(0.0, |l| (pos.as_secs_f32() / l.as_secs_f32().max(0.001)).clamp(0.0, 1.0));
    let by = top.max.y + 16.0 + 22.0;
    let font = FontId::proportional(14.0);
    let left = painter.layout_no_wrap(clock(pos), font.clone(), vis.text_color());
    let right = painter.layout_no_wrap(len.map_or("--:--".into(), clock), font.clone(), vis.text_color());
    let lw = left.size().x.max(right.size().x) + 12.0;
    painter.galley(pos2(info.min.x + pad, by - left.size().y / 2.0), left, vis.text_color());
    painter.galley(pos2(info.max.x - pad - right.size().x, by - right.size().y / 2.0), right, vis.text_color());
    let bar = Rect::from_min_max(pos2(info.min.x + pad + lw, by - 3.0), pos2(info.max.x - pad - lw, by + 3.0));
    let hit = ui.interact(bar.expand2(vec2(6.0, 10.0)), ui.id().with("audio-seek"), Sense::click_and_drag());
    let shown = match hit.interact_pointer_pos() {
        Some(p) if hit.dragged() || hit.clicked() => {
            let f = ((p.x - bar.min.x) / bar.width()).clamp(0.0, 1.0);
            if hit.drag_stopped() || hit.clicked() {
                act = Some(HeaderAct::Run(Cmd::AudioSeek(f)));
            }
            f
        }
        _ => frac,
    };
    painter.rect_filled(bar, 3.0, vis.widgets.inactive.bg_fill);
    painter.rect_filled(Rect::from_min_max(bar.min, pos2(bar.min.x + bar.width() * shown, bar.max.y)), 3.0, accent);
    let knob = if hit.hovered() || hit.dragged() { 8.0 } else { 6.0 };
    painter.circle_filled(pos2(bar.min.x + bar.width() * shown, bar.center().y), knob, vis.strong_text_color());

    // Repeat chip on the left, volume on the right, the buttons centred
    // between them.
    let cy = by + 62.0;
    let rep = Repeat::from(prefs.repeat);
    let label = match rep {
        Repeat::Off => tr("Repeat: off"),
        Repeat::All => tr("Repeat: all"),
        Repeat::One => tr("Repeat: one"),
    };
    let g = painter.layout_no_wrap(label.to_string(), FontId::proportional(14.0), if rep == Repeat::Off { vis.weak_text_color() } else { vis.strong_text_color() });
    let rr = Rect::from_min_size(pos2(info.min.x + pad, cy - 15.0), vec2(g.size().x + 20.0, 30.0));
    let vw = ((info.width() - 2.0 * pad - rr.width() - 240.0 - 40.0) * 0.6).clamp(50.0, 120.0);
    let vbar = Rect::from_min_max(pos2(info.max.x - pad - vw, cy - 3.0), pos2(info.max.x - pad, cy + 3.0));
    let cx = (rr.max.x + vbar.min.x - 34.0) / 2.0;
    let playing = a.playing();
    let fg = vis.strong_text_color();
    let button = |c: egui::Pos2, r: f32, id: &str, filled: bool| {
        let rect = Rect::from_center_size(c, Vec2::splat(r * 2.0));
        let resp = ui.interact(rect, ui.id().with(("audio-btn", id)), Sense::click());
        if filled {
            painter.circle_filled(c, r, if resp.hovered() { accent } else { accent.gamma_multiply(0.85) });
        } else if resp.hovered() {
            painter.circle_filled(c, r, vis.widgets.hovered.weak_bg_fill);
        }
        resp.clicked()
    };
    if button(pos2(cx, cy), 30.0, "play", true) {
        act = Some(HeaderAct::Run(Cmd::AudioToggle));
    }
    let on_accent = vis.selection.stroke.color;
    if playing {
        for dx in [-7.0, 7.0] {
            painter.rect_filled(Rect::from_center_size(pos2(cx + dx, cy), vec2(7.0, 22.0)), 1.5, on_accent);
        }
    } else {
        tri(&painter, pos2(cx + 3.0, cy), 13.0, true, on_accent);
    }
    for (dir, id, cmd) in [(-1.0f32, "prev", Cmd::AudioPrev), (1.0, "next", Cmd::AudioNext)] {
        let c = pos2(cx + dir * 78.0, cy);
        if button(c, 24.0, id, false) {
            act = Some(HeaderAct::Run(cmd));
        }
        tri(&painter, c + vec2(-dir * 2.0, 0.0), 10.0, dir > 0.0, fg);
        painter.rect_filled(Rect::from_center_size(c + vec2(dir * 9.0, 0.0), vec2(3.5, 18.0)), 1.0, fg);
    }
    let resp = ui.interact(rr, ui.id().with("audio-repeat"), Sense::click()).on_hover_text("□");
    painter.rect_filled(rr, 15.0, if rep != Repeat::Off { accent.gamma_multiply(0.35) } else if resp.hovered() { vis.widgets.hovered.weak_bg_fill } else { vis.widgets.inactive.weak_bg_fill });
    painter.galley(rr.center() - g.size() / 2.0, g, fg);
    if resp.clicked() {
        act = Some(HeaderAct::Run(Cmd::AudioRepeat));
    }
    speaker(&painter, pos2(vbar.min.x - 26.0, cy), prefs.volume, fg);
    let vhit = ui.interact(vbar.expand2(vec2(6.0, 12.0)), ui.id().with("audio-volume"), Sense::click_and_drag());
    if let Some(p) = vhit.interact_pointer_pos().filter(|_| vhit.dragged() || vhit.clicked()) {
        act = Some(HeaderAct::Volume(((p.x - vbar.min.x) / vbar.width()).clamp(0.0, 1.0)));
    }
    if vhit.hovered() {
        let d = ui.input(|i| i.smooth_scroll_delta.y);
        if d.abs() > 0.5 {
            act = Some(HeaderAct::Volume((prefs.volume + d / 600.0).clamp(0.0, 1.0)));
        }
    }
    painter.rect_filled(vbar, 3.0, vis.widgets.inactive.bg_fill);
    painter.rect_filled(Rect::from_min_max(vbar.min, pos2(vbar.min.x + vw * prefs.volume, vbar.max.y)), 3.0, accent);
    if a.volume_shown.is_some_and(|t| t.elapsed().as_secs_f32() < 1.5) || vhit.hovered() || vhit.dragged() {
        painter.text(pos2(vbar.center().x, cy + 18.0), Align2::CENTER_CENTER, format!("{:.0}%", prefs.volume * 100.0), FontId::proportional(13.0), fg);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }

    // Playlist.
    if list.height() > 40.0 {
        painter.rect_filled(list, 8.0, vis.faint_bg_color);
        let head = Rect::from_min_size(list.min, vec2(list.width(), 34.0));
        painter.text(head.left_center() + vec2(14.0, 0.0), Align2::LEFT_CENTER, format!("{} · {}", tr("Playlist"), a.tracks.len()), FontId::proportional(15.0), vis.strong_text_color());
        let body = Rect::from_min_max(pos2(list.min.x + 4.0, head.max.y), list.max - vec2(4.0, 4.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("playlist"));
        let row_h = 30.0;
        let reveal = std::mem::take(&mut a.reveal);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
            for (i, p) in a.tracks.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                if i == a.cursor {
                    ui.painter().rect_filled(r, 5.0, accent.gamma_multiply(0.45));
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 5.0, vis.widgets.hovered.weak_bg_fill);
                }
                if reveal && i == a.cursor {
                    ui.scroll_to_rect(r, None);
                }
                let current = i == a.index;
                let c = if current { vis.strong_text_color() } else { vis.text_color() };
                if current {
                    if playing {
                        for (k, h) in [0.5f32, 1.0, 0.7].iter().enumerate() {
                            let t = ui.input(|i| i.time) as f32;
                            let hh = 12.0 * (h * (0.6 + 0.4 * (t * 6.0 + k as f32 * 1.7).sin().abs()));
                            ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x + 12.0 + k as f32 * 5.0, r.center().y + 7.0 - hh), vec2(3.0, hh)), 0.5, accent);
                        }
                    } else {
                        tri(ui.painter(), pos2(r.min.x + 18.0, r.center().y), 6.0, true, accent);
                    }
                } else {
                    ui.painter().text(pos2(r.min.x + 30.0, r.center().y), Align2::RIGHT_CENTER, (i + 1).to_string(), FontId::proportional(13.0), vis.weak_text_color());
                }
                let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let g = ui.painter().layout(name, FontId::proportional(15.0), c, r.width() - 50.0);
                let clip = ui.painter().with_clip_rect(r.shrink2(vec2(0.0, 2.0)));
                clip.galley(pos2(r.min.x + 40.0, r.center().y - g.rows.first().map_or(8.0, |row| row.height()) / 2.0), g, c);
                if resp.clicked() {
                    act = Some(HeaderAct::Run(Cmd::AudioPlay(i)));
                }
            }
        });
    }
    act
}

/// A filled triangle pointing right (or left) around `c`.
fn tri(p: &egui::Painter, c: egui::Pos2, r: f32, right: bool, color: Color32) {
    let d = if right { 1.0 } else { -1.0 };
    let pts = vec![c + vec2(d * r, 0.0), c + vec2(-d * r * 0.8, -r), c + vec2(-d * r * 0.8, r)];
    p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// ♪ drawn with shapes (the UI font may lack it).
fn note_glyph(p: &egui::Painter, c: egui::Pos2, size: f32, color: Color32) {
    let head = c + vec2(-size * 0.18, size * 0.32);
    p.circle_filled(head, size * 0.17, color);
    let top = pos2(head.x + size * 0.15, c.y - size * 0.45);
    p.line_segment([pos2(top.x, head.y), top], Stroke::new(size * 0.07, color));
    p.add(egui::Shape::convex_polygon(vec![top, top + vec2(size * 0.3, size * 0.15), top + vec2(size * 0.3, size * 0.35), top + vec2(0.0, size * 0.2)], color, Stroke::NONE));
}

/// A speaker with as many waves as the volume deserves.
fn speaker(p: &egui::Painter, c: egui::Pos2, volume: f32, color: Color32) {
    p.rect_filled(Rect::from_center_size(c + vec2(-6.0, 0.0), vec2(5.0, 8.0)), 1.0, color);
    p.add(egui::Shape::convex_polygon(vec![c + vec2(-4.0, -4.0), c + vec2(2.0, -9.0), c + vec2(2.0, 9.0), c + vec2(-4.0, 4.0)], color, Stroke::NONE));
    let waves = if volume <= 0.0 { 0 } else if volume < 0.4 { 1 } else if volume < 0.75 { 2 } else { 3 };
    for k in 0..waves {
        let r = 6.0 + k as f32 * 4.0;
        let pts: Vec<egui::Pos2> = (0..=8).map(|i| {
            let t = -0.8 + 1.6 * i as f32 / 8.0;
            c + vec2(2.0 + r * t.cos(), r * t.sin())
        }).collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.6, color)));
    }
    if volume <= 0.0 {
        p.line_segment([c + vec2(6.0, -5.0), c + vec2(14.0, 5.0)], Stroke::new(1.8, color));
        p.line_segment([c + vec2(6.0, 5.0), c + vec2(14.0, -5.0)], Stroke::new(1.8, color));
    }
}
