//! Drawing the terminal page: its own tab strip and the character grid
//! (box drawing, blocks and braille drawn by hand so tables and graphs of
//! terminal programs join up), plus the mouse.

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use egui::{Align2, Color32, FontFamily, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::Terminal;
use super::scheme::{Scheme, c32};
use crate::i18n::tr;
use crate::icons::Icons;

pub enum Act {
    Tab(usize),
    CloseTab(usize),
    New,
    Keys,
    History,
    Saved,
    Settings,
    /// Right click: the page's menu.
    Menu,
    /// Clicked into the grid.
    Focus,
    /// Mouse selection finished: text to keep for pasting.
    Selected(String),
    Paste,
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("viewer".into()))
}

const STRIP_H: f32 = 38.0;

/// `strip`: the tab strip and its tools (left out while the pad's keyboard
/// types into a small window, for more lines).
pub fn draw(ui: &mut Ui, t: &mut Terminal, scheme: &Scheme, size: f32, icons: &mut Icons, focused: bool, strip_shown: bool) -> Option<Act> {
    let mut act = None;
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());
    let vis = ui.visuals().clone();

    let strip = Rect::from_min_size(full.min, vec2(full.width(), if strip_shown { STRIP_H } else { 0.0 }));
    if strip_shown {
        // ---- tab strip
        let mut x = strip.min.x;
        let btn = |x: f32| Rect::from_min_size(pos2(x, strip.min.y + 3.0), Vec2::splat(STRIP_H - 6.0));
        // Right side: settings, saved, history, keys.
        let mut rx = strip.max.x - (STRIP_H - 6.0);
        let tools: [(&[&str], &str, &str, fn() -> Act); 4] = [
            (&["preferences-system", "configure", "settings", "emblem-system"], "⚙", tr("Terminal settings"), || Act::Settings),
            (&["bookmarks", "bookmark-new", "starred"], "★", tr("Saved commands"), || Act::Saved),
            (&["document-open-recent", "history", "appointment-soon"], "⟲", tr("Command history"), || Act::History),
            (&["input-keyboard", "keyboard", "preferences-desktop-keyboard"], "⌨", tr("Quick keys"), || Act::Keys),
        ];
        for (names, fb, tip, a) in tools {
            if crate::view::tool_button(ui, btn(rx), icons, names, fb, tip, false).clicked() {
                act = Some(a());
            }
            rx -= STRIP_H - 2.0;
        }
        let tabs_end = rx + (STRIP_H - 6.0) - 8.0;
        let n = t.sessions.len().max(1);
        let plus_w = STRIP_H - 6.0;
        let tab_w = ((tabs_end - x - plus_w - 6.0) / n as f32).clamp(60.0, 240.0);
        for (i, s) in t.sessions.iter().enumerate() {
            let r = Rect::from_min_size(pos2(x, strip.min.y + 3.0), vec2(tab_w - 4.0, STRIP_H - 6.0));
            let resp = ui.interact(r, ui.id().with(("term-tab", i)), Sense::click());
            let cur = i == t.active;
            let ringing = s.bell.is_some_and(|b| b.elapsed().as_secs_f32() < 0.6);
            if cur {
                ui.painter().rect(r, 8.0, crate::view::tint(&vis, 0.35), Stroke::new(1.5, vis.selection.bg_fill), StrokeKind::Inside);
            } else if resp.hovered() || ringing {
                ui.painter().rect_filled(r, 8.0, vis.widgets.hovered.weak_bg_fill);
            } else {
                ui.painter().rect_stroke(r, 8.0, vis.widgets.noninteractive.bg_stroke, StrokeKind::Inside);
            }
            let close = Rect::from_center_size(pos2(r.max.x - 14.0, r.center().y), Vec2::splat(20.0));
            let title = if s.exited { format!("{} — {}", s.title, tr("finished")) } else if s.title.is_empty() { tr("Terminal").to_string() } else { s.title.clone() };
            let g = crate::view::elided_text(ui, &title, FontId::proportional(14.0), if cur { vis.strong_text_color() } else { vis.text_color() }, r.width() - 40.0);
            ui.painter().galley(pos2(r.min.x + 10.0, r.center().y - g.size().y / 2.0), g, vis.text_color());
            let cr = ui.interact(close, ui.id().with(("term-tab-close", i)), Sense::click());
            if cr.hovered() {
                ui.painter().rect_filled(close, 5.0, vis.widgets.active.weak_bg_fill);
            }
            ui.painter().text(close.center(), Align2::CENTER_CENTER, "×", FontId::proportional(16.0), vis.weak_text_color());
            if cr.clicked() {
                act = Some(Act::CloseTab(i));
            } else if resp.clicked() {
                act = Some(Act::Tab(i));
            } else if resp.middle_clicked() {
                act = Some(Act::CloseTab(i));
            }
            x += tab_w;
        }
        let plus = btn(x);
        if crate::view::tool_button(ui, plus, icons, &["list-add", "tab-new"], "+", tr("New terminal tab"), false).clicked() {
            act = Some(Act::New);
        }

        // ---- the grid
    }
    let area = Rect::from_min_max(pos2(full.min.x, strip.max.y + if strip_shown { 4.0 } else { 0.0 }), full.max);
    let bg = c32(scheme.bg);
    ui.painter().rect_filled(area, 8.0, bg);
    let stroke = if focused { Stroke::new(1.5, vis.selection.bg_fill) } else { Stroke::new(1.0, vis.widgets.noninteractive.bg_stroke.color) };
    ui.painter().rect_stroke(area, 8.0, stroke, StrokeKind::Inside);
    let inner = area.shrink2(vec2(10.0, 8.0));
    let font = font(size);
    let (cw, ch) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    let (cw, ch) = (cw.max(1.0), ch.max(1.0));
    let cols = ((inner.width() / cw).floor() as usize).max(2);
    let rows = ((inner.height() / ch).floor() as usize).max(1);
    t.cell = (cw, ch);
    t.grid = (cols, rows);
    let ppp = ui.pixels_per_point();
    let cell = (cw * ppp, ch * ppp);
    let selecting = t.selecting;
    let Some(s) = t.sessions.get_mut(t.active) else {
        ui.painter().text(inner.center(), Align2::CENTER_CENTER, tr("No terminal open"), FontId::proportional(16.0), vis.weak_text_color());
        return act;
    };
    s.resize(cols, rows, cell);
    let origin = inner.min;

    let resp = ui.interact(area, ui.id().with("term-grid"), Sense::click_and_drag());
    let mut term = s.term.lock();
    let mode = *term.mode();
    let offset = term.grid().display_offset() as i32;
    let to_point = |p: Pos2| -> (Point, Side) {
        let col = ((p.x - origin.x) / cw).floor().clamp(0.0, cols as f32 - 1.0);
        let side = if (p.x - origin.x) / cw - col < 0.5 { Side::Left } else { Side::Right };
        let row = ((p.y - origin.y) / ch).floor().clamp(0.0, rows as f32 - 1.0) as i32;
        (Point::new(Line(row - offset), Column(col as usize)), side)
    };
    // ---- mouse
    let shift = ui.input(|i| i.modifiers.shift);
    let report = mode.intersects(TermMode::MOUSE_MODE) && !shift;
    let mut send: Vec<u8> = Vec::new();
    if resp.clicked() || resp.drag_started() {
        act.get_or_insert(Act::Focus);
    }
    if resp.secondary_clicked() {
        act = Some(Act::Menu);
    }
    if resp.middle_clicked() {
        act = Some(Act::Paste);
    }
    let hover = resp.hover_pos();
    if report {
        let sgr = mode.contains(TermMode::SGR_MOUSE);
        let enc = |b: u8, p: Point, press: bool| -> Vec<u8> {
            let (x, y) = (p.column.0 + 1, (p.line.0 + offset) as usize + 1);
            if sgr {
                format!("\x1b[<{b};{x};{y}{}", if press { 'M' } else { 'm' }).into_bytes()
            } else {
                let b = if press { b } else { 3 };
                vec![0x1b, b'[', b'M', 32 + b, (32 + x.min(222)) as u8, (32 + y.min(222)) as u8]
            }
        };
        let (pressed, released) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released()));
        if let Some(p) = hover {
            let (pt, _) = to_point(p);
            if pressed && area.contains(p) {
                send.extend(enc(0, pt, true));
            }
            if released {
                send.extend(enc(0, pt, false));
            }
            if resp.dragged() && mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                send.extend(enc(32, pt, true));
            }
        }
    } else if let Some(p) = resp.interact_pointer_pos() {
        let (pt, side) = to_point(p);
        if resp.drag_started() {
            let ty = if ui.input(|i| i.modifiers.alt) { SelectionType::Block } else { SelectionType::Simple };
            term.selection = Some(Selection::new(ty, pt, side));
        } else if resp.dragged() {
            if let Some(sel) = term.selection.as_mut() {
                sel.update(pt, side);
            }
        } else if resp.double_clicked() {
            term.selection = Some(Selection::new(SelectionType::Semantic, pt, side));
        } else if resp.triple_clicked() {
            term.selection = Some(Selection::new(SelectionType::Lines, pt, side));
        } else if resp.clicked() && !selecting {
            term.selection = None;
        }
        if resp.drag_stopped() || resp.double_clicked() || resp.triple_clicked() {
            if let Some(text) = term.selection_to_string().filter(|s| !s.is_empty()) {
                act = Some(Act::Selected(text));
            }
        }
    }
    // Wheel: the program's own scrolling, else the scrollback.
    if resp.hovered() || resp.contains_pointer() {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            let lines = (dy / ch).round() as i32;
            if lines != 0 {
                if report {
                    if let Some(p) = hover {
                        let (pt, _) = to_point(p);
                        let b = if lines > 0 { 64 } else { 65 };
                        let (x, y) = (pt.column.0 + 1, (pt.line.0 + offset) as usize + 1);
                        for _ in 0..lines.unsigned_abs().min(5) {
                            send.extend(format!("\x1b[<{b};{x};{y}M").into_bytes());
                        }
                    }
                } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
                    let k = if lines > 0 { super::keys::K::Up } else { super::keys::K::Down };
                    for _ in 0..lines.unsigned_abs().min(10) {
                        send.extend(super::keys::encode(k, super::keys::Mods::NONE, mode));
                    }
                } else {
                    term.scroll_display(Scroll::Delta(lines));
                }
            }
        }
    }

    // ---- paint
    let painter = ui.painter_at(area.shrink(1.0));
    let content = term.renderable_content();
    let colors = content.colors;
    let sel = content.selection;
    let cursor = content.cursor;
    let offset = content.display_offset as i32;
    let accent = vis.selection.bg_fill;
    let sel_bg = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 190);
    struct Run {
        x: f32,
        text: String,
        color: Color32,
        bold: bool,
    }
    let mut row_runs: Vec<Run> = Vec::new();
    let mut lines: Vec<(Pos2, Pos2, Color32)> = Vec::new();
    let mut last_row = i32::MIN;
    let flush = |runs: &mut Vec<Run>, painter: &Painter, y: f32| {
        for r in runs.drain(..) {
            painter.text(pos2(r.x, y), Align2::LEFT_TOP, &r.text, font.clone(), r.color);
            if r.bold {
                painter.text(pos2(r.x + 0.6, y), Align2::LEFT_TOP, &r.text, font.clone(), r.color);
            }
        }
    };
    for c in content.display_iter {
        let vrow = c.point.line.0 + offset;
        if vrow < 0 || vrow as usize >= rows {
            continue;
        }
        let y = origin.y + vrow as f32 * ch;
        if vrow != last_row {
            flush(&mut row_runs, &painter, origin.y + last_row as f32 * ch);
            last_row = vrow;
        }
        let flags = c.cell.flags;
        if flags.contains(Flags::WIDE_CHAR_SPACER) || flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let x = origin.x + c.point.column.0 as f32 * cw;
        let w = if flags.contains(Flags::WIDE_CHAR) { 2.0 * cw } else { cw };
        let bold = flags.contains(Flags::BOLD);
        let mut fg = scheme.color(c.cell.fg, colors, bold, flags.contains(Flags::DIM));
        let mut bgc = scheme.color(c.cell.bg, colors, false, false);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bgc);
        }
        let rect = Rect::from_min_size(pos2(x, y), vec2(w, ch));
        if bgc != scheme.bg || flags.contains(Flags::INVERSE) {
            painter.rect_filled(rect.expand(0.3), 0.0, c32(bgc));
        }
        let selected = sel.is_some_and(|r| r.contains(c.point));
        if selected {
            painter.rect_filled(rect, 0.0, sel_bg);
        }
        let on_cursor = c.point == cursor.point && cursor.shape == CursorShape::Block && focused;
        let color = if on_cursor { c32(scheme.bg) } else { c32(fg) };
        if flags.contains(Flags::HIDDEN) || c.cell.c == ' ' || c.cell.c == '\0' {
            // Nothing to write.
        } else if special(&painter, rect, c.cell.c, color) {
        } else if c.cell.c.is_ascii_graphic() && c.cell.zerowidth().is_none() && !on_cursor {
            match row_runs.last_mut() {
                Some(r) if r.color == color && r.bold == bold && (r.x + r.text.len() as f32 * cw - x).abs() < 0.5 => r.text.push(c.cell.c),
                _ => row_runs.push(Run { x, text: c.cell.c.to_string(), color, bold }),
            }
        } else {
            let mut s = c.cell.c.to_string();
            if let Some(z) = c.cell.zerowidth() {
                s.extend(z);
            }
            // Drawn after the block cursor's fill.
            if !on_cursor {
                painter.text(pos2(x + w / 2.0, y), Align2::CENTER_TOP, &s, font.clone(), color);
            }
        }
        if flags.intersects(Flags::ALL_UNDERLINES) {
            lines.push((pos2(x, y + ch - 1.5), pos2(x + w, y + ch - 1.5), c32(fg)));
        }
        if flags.contains(Flags::STRIKEOUT) {
            lines.push((pos2(x, y + ch / 2.0), pos2(x + w, y + ch / 2.0), c32(fg)));
        }
    }
    flush(&mut row_runs, &painter, origin.y + last_row as f32 * ch);
    for (a, b, c) in lines {
        painter.line_segment([a, b], Stroke::new(1.0, c));
    }
    // ---- cursor
    let cvrow = cursor.point.line.0 + offset;
    if cursor.shape != CursorShape::Hidden && cvrow >= 0 && (cvrow as usize) < rows {
        let x = origin.x + cursor.point.column.0 as f32 * cw;
        let y = origin.y + cvrow as f32 * ch;
        let cell_c = term.grid()[cursor.point].c;
        let wide = term.grid()[cursor.point].flags.contains(Flags::WIDE_CHAR);
        let r = Rect::from_min_size(pos2(x, y), vec2(if wide { 2.0 * cw } else { cw }, ch));
        let cc = c32(colors[alacritty_terminal::vte::ansi::NamedColor::Cursor as usize].unwrap_or(scheme.cursor));
        match cursor.shape {
            CursorShape::Block if focused => {
                painter.rect_filled(r, 1.0, cc);
                if cell_c != ' ' && !special(&painter, r, cell_c, c32(scheme.bg)) {
                    painter.text(pos2(r.center().x, y), Align2::CENTER_TOP, cell_c.to_string(), font.clone(), c32(scheme.bg));
                }
            }
            CursorShape::Beam => {
                painter.rect_filled(Rect::from_min_size(r.min, vec2(2.0, ch)), 0.0, cc);
            }
            CursorShape::Underline => {
                painter.rect_filled(Rect::from_min_max(pos2(r.min.x, r.max.y - 2.0), r.max), 0.0, cc);
            }
            _ => {
                painter.rect_stroke(r.shrink(0.5), 1.0, Stroke::new(1.0, cc), StrokeKind::Inside);
            }
        }
    }
    // Scrolled back: say how far.
    if offset > 0 {
        let label = format!("↑ {offset}");
        let g = painter.layout_no_wrap(label, FontId::proportional(13.0), vis.strong_text_color());
        let r = Rect::from_min_size(pos2(area.max.x - g.size().x - 22.0, area.min.y + 8.0), g.size() + vec2(12.0, 6.0));
        painter.rect_filled(r, 6.0, vis.extreme_bg_color.gamma_multiply(0.9));
        painter.galley(r.min + vec2(6.0, 3.0), g, vis.strong_text_color());
    }
    if selecting {
        let label = tr("Choosing text to copy");
        let g = painter.layout_no_wrap(label.to_string(), FontId::proportional(14.0), Color32::WHITE);
        let r = Rect::from_center_size(pos2(area.center().x, area.max.y - 22.0), g.size() + vec2(20.0, 10.0));
        painter.rect_filled(r, 8.0, accent);
        painter.galley(r.min + vec2(10.0, 5.0), g, Color32::WHITE);
    }
    drop(term);
    if !send.is_empty() {
        s.write(send);
    }
    act
}

/// Box drawing, blocks, braille and powerline arrows, drawn to fill the
/// cell exactly. False: not one of those.
pub fn special(p: &Painter, r: Rect, c: char, color: Color32) -> bool {
    let u = c as u32;
    match u {
        0x2500..=0x257f => box_lines(p, r, u, color),
        0x2580..=0x259f => {
            blocks(p, r, u, color);
            true
        }
        0x2800..=0x28ff => {
            braille(p, r, (u - 0x2800) as u8, color);
            true
        }
        0xe0b0 | 0xe0b2 => {
            let pts = if u == 0xe0b0 { vec![r.left_top(), r.right_center(), r.left_bottom()] } else { vec![r.right_top(), r.right_bottom(), r.left_center()] };
            p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
            true
        }
        0xe0b1 | 0xe0b3 => {
            let pts = if u == 0xe0b1 { [r.left_top(), r.right_center(), r.left_bottom()] } else { [r.right_top(), r.left_center(), r.right_bottom()] };
            p.line_segment([pts[0], pts[1]], Stroke::new(1.0, color));
            p.line_segment([pts[1], pts[2]], Stroke::new(1.0, color));
            true
        }
        _ => false,
    }
}

/// Weights up, right, down, left: 0 none, 1 light, 2 heavy, 3 double.
fn box_weights(u: u32) -> Option<[u8; 4]> {
    Some(match u {
        0x2500 | 0x2504 | 0x2508 | 0x254c => [0, 1, 0, 1],
        0x2501 | 0x2505 | 0x2509 | 0x254d => [0, 2, 0, 2],
        0x2502 | 0x2506 | 0x250a | 0x254e => [1, 0, 1, 0],
        0x2503 | 0x2507 | 0x250b | 0x254f => [2, 0, 2, 0],
        0x250c | 0x256d => [0, 1, 1, 0],
        0x250d => [0, 2, 1, 0],
        0x250e => [0, 1, 2, 0],
        0x250f => [0, 2, 2, 0],
        0x2510 | 0x256e => [0, 0, 1, 1],
        0x2511 => [0, 0, 1, 2],
        0x2512 => [0, 0, 2, 1],
        0x2513 => [0, 0, 2, 2],
        0x2514 | 0x2570 => [1, 1, 0, 0],
        0x2515 => [1, 2, 0, 0],
        0x2516 => [2, 1, 0, 0],
        0x2517 => [2, 2, 0, 0],
        0x2518 | 0x256f => [1, 0, 0, 1],
        0x2519 => [1, 0, 0, 2],
        0x251a => [2, 0, 0, 1],
        0x251b => [2, 0, 0, 2],
        0x251c => [1, 1, 1, 0],
        0x251d => [1, 2, 1, 0],
        0x2520 => [2, 1, 2, 0],
        0x2523 => [2, 2, 2, 0],
        0x2524 => [1, 0, 1, 1],
        0x2525 => [1, 0, 1, 2],
        0x2528 => [2, 0, 2, 1],
        0x252b => [2, 0, 2, 2],
        0x252c => [0, 1, 1, 1],
        0x252f => [0, 2, 1, 2],
        0x2530 => [0, 1, 2, 1],
        0x2533 => [0, 2, 2, 2],
        0x2534 => [1, 1, 0, 1],
        0x2537 => [1, 2, 0, 2],
        0x2538 => [2, 1, 0, 1],
        0x253b => [2, 2, 0, 2],
        0x253c => [1, 1, 1, 1],
        0x253f => [1, 2, 1, 2],
        0x2542 => [2, 1, 2, 1],
        0x254b => [2, 2, 2, 2],
        0x2550 => [0, 3, 0, 3],
        0x2551 => [3, 0, 3, 0],
        0x2552 => [0, 3, 1, 0],
        0x2553 => [0, 1, 3, 0],
        0x2554 => [0, 3, 3, 0],
        0x2555 => [0, 0, 1, 3],
        0x2556 => [0, 0, 3, 1],
        0x2557 => [0, 0, 3, 3],
        0x2558 => [1, 3, 0, 0],
        0x2559 => [3, 1, 0, 0],
        0x255a => [3, 3, 0, 0],
        0x255b => [1, 0, 0, 3],
        0x255c => [3, 0, 0, 1],
        0x255d => [3, 0, 0, 3],
        0x255e => [1, 3, 1, 0],
        0x255f => [3, 1, 3, 0],
        0x2560 => [3, 3, 3, 0],
        0x2561 => [1, 0, 1, 3],
        0x2562 => [3, 0, 3, 1],
        0x2563 => [3, 0, 3, 3],
        0x2564 => [0, 3, 1, 3],
        0x2565 => [0, 1, 3, 1],
        0x2566 => [0, 3, 3, 3],
        0x2567 => [1, 3, 0, 3],
        0x2568 => [3, 1, 0, 1],
        0x2569 => [3, 3, 0, 3],
        0x256a => [1, 3, 1, 3],
        0x256b => [3, 1, 3, 1],
        0x256c => [3, 3, 3, 3],
        0x2574 => [0, 0, 0, 1],
        0x2575 => [1, 0, 0, 0],
        0x2576 => [0, 1, 0, 0],
        0x2577 => [0, 0, 1, 0],
        0x2578 => [0, 0, 0, 2],
        0x2579 => [2, 0, 0, 0],
        0x257a => [0, 2, 0, 0],
        0x257b => [0, 0, 2, 0],
        0x257c => [0, 2, 0, 1],
        0x257d => [1, 0, 2, 0],
        0x257e => [0, 1, 0, 2],
        0x257f => [2, 0, 1, 0],
        _ => return None,
    })
}

fn box_lines(p: &Painter, r: Rect, u: u32, color: Color32) -> bool {
    let Some(w) = box_weights(u) else { return false };
    let light = (r.width() / 8.0).round().max(1.0);
    let c = r.center();
    let c = pos2(c.x.round(), c.y.round());
    let fill = |a: Pos2, b: Pos2| p.rect_filled(Rect::from_two_pos(a, b), 0.0, color);
    for (dir, &wt) in w.iter().enumerate() {
        if wt == 0 {
            continue;
        }
        let th = if wt == 2 { light * 2.0 } else { light };
        let half = th / 2.0;
        // Lines run from past the centre (so corners close) to the edge.
        let ext = if w.iter().any(|&x| x == 3) { light * 1.5 } else { half };
        let offsets: &[f32] = if wt == 3 { &[-light, light] } else { &[0.0] };
        for &o in offsets {
            let _ = match dir {
                0 => fill(pos2(c.x + o - half, r.min.y), pos2(c.x + o + half, c.y + ext)),
                1 => fill(pos2(c.x - ext, c.y + o - half), pos2(r.max.x, c.y + o + half)),
                2 => fill(pos2(c.x + o - half, c.y - ext), pos2(c.x + o + half, r.max.y)),
                _ => fill(pos2(r.min.x, c.y + o - half), pos2(c.x + ext, c.y + o + half)),
            };
        }
    }
    true
}

fn blocks(p: &Painter, r: Rect, u: u32, color: Color32) {
    let (w, h) = (r.width(), r.height());
    let part = |x0: f32, y0: f32, x1: f32, y1: f32| {
        p.rect_filled(Rect::from_min_max(pos2(r.min.x + x0 * w, r.min.y + y0 * h), pos2(r.min.x + x1 * w, r.min.y + y1 * h)), 0.0, color);
    };
    match u {
        0x2580 => part(0.0, 0.0, 1.0, 0.5),
        0x2581..=0x2588 => part(0.0, 1.0 - (u - 0x2580) as f32 / 8.0, 1.0, 1.0),
        0x2589..=0x258f => part(0.0, 0.0, (0x2590 - u) as f32 / 8.0, 1.0),
        0x2590 => part(0.5, 0.0, 1.0, 1.0),
        0x2591..=0x2593 => {
            let a = [0.25, 0.5, 0.75][(u - 0x2591) as usize];
            p.rect_filled(r, 0.0, color.gamma_multiply(a));
        }
        0x2594 => part(0.0, 0.0, 1.0, 0.125),
        0x2595 => part(0.875, 0.0, 1.0, 1.0),
        _ => {
            // Quadrants: bits for upper-left, upper-right, lower-left, lower-right.
            let q: u8 = match u {
                0x2596 => 0b0010,
                0x2597 => 0b0001,
                0x2598 => 0b1000,
                0x2599 => 0b1011,
                0x259a => 0b1001,
                0x259b => 0b1110,
                0x259c => 0b1101,
                0x259d => 0b0100,
                0x259e => 0b0110,
                _ => 0b0111,
            };
            for (bit, (x, y)) in [(8, (0.0, 0.0)), (4, (0.5, 0.0)), (2, (0.0, 0.5)), (1, (0.5, 0.5))] {
                if q & bit != 0 {
                    part(x, y, x + 0.5, y + 0.5);
                }
            }
        }
    }
}

fn braille(p: &Painter, r: Rect, bits: u8, color: Color32) {
    // Dot order: 1 2 3 down the left, 4 5 6 down the right, then 7 8.
    const DOTS: [(u8, u8); 8] = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)];
    let (dx, dy) = (r.width() / 2.0, r.height() / 4.0);
    let rad = (dx.min(dy) * 0.32).max(0.8);
    for (i, (cx, cy)) in DOTS.iter().enumerate() {
        if bits & (1 << i) != 0 {
            p.circle_filled(pos2(r.min.x + dx * (*cx as f32 + 0.5), r.min.y + dy * (*cy as f32 + 0.5)), rad, color);
        }
    }
}

// ---------------------------------------------------------------- panels

/// The quick keys panel: (key index, close after) when one is clicked.
pub fn draw_keys(ui: &mut Ui, q: &mut super::app::QuickKeys, pad: crate::input::PadKind) -> Option<(usize, bool)> {
    let mut ev = None;
    let vis = ui.visuals().clone();
    let cols = super::app::QK_COLS;
    let (bw, bh, gap) = (96.0, 44.0, 6.0);
    ui.set_width(cols as f32 * (bw + gap) - gap);
    ui.label(egui::RichText::new(&q.title).strong().size(16.0));
    ui.add_space(6.0);
    let rows = q.keys.len().div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(cols as f32 * (bw + gap) - gap, rows as f32 * (bh + gap) - gap), Sense::hover());
    for (i, k) in q.keys.iter().enumerate() {
        let (r, c) = (i / cols, i % cols);
        let rect = Rect::from_min_size(area.min + vec2(c as f32 * (bw + gap), r as f32 * (bh + gap)), vec2(bw, bh));
        let resp = ui.interact(rect, ui.id().with(("qk", i)), Sense::click());
        if resp.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
            q.cursor = i;
        }
        let cur = i == q.cursor;
        if cur {
            ui.painter().rect(rect, 8.0, crate::view::tint(&vis, 0.45), Stroke::new(2.0, vis.selection.bg_fill), StrokeKind::Inside);
        } else {
            ui.painter().rect(rect, 8.0, vis.widgets.inactive.weak_bg_fill, vis.widgets.noninteractive.bg_stroke, StrokeKind::Inside);
        }
        let hint = super::keys::hint(k.hint);
        if hint.is_empty() {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, k.label, FontId::proportional(15.0), vis.strong_text_color());
        } else {
            ui.painter().text(rect.center_top() + vec2(0.0, 6.0), Align2::CENTER_TOP, k.label, FontId::proportional(14.0), vis.strong_text_color());
            let g = crate::view::elided_text(ui, hint, FontId::proportional(11.0), vis.weak_text_color(), bw - 8.0);
            ui.painter().galley(pos2(rect.center().x - g.size().x / 2.0, rect.max.y - g.size().y - 5.0), g, vis.weak_text_color());
        }
        if resp.clicked() {
            ev = Some((i, false));
        }
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        use crate::input::Action;
        for (a, l) in [(Action::Accept, tr("Press")), (Action::Select, tr("Press and close")), (Action::Back, tr("Close"))] {
            crate::view::glyph(ui, a, pad);
            ui.label(egui::RichText::new(l).size(13.0).color(vis.weak_text_color()));
            ui.add_space(10.0);
        }
    });
    ev
}

/// Terminal settings: (row, step) on a click.
/// Its rows and the scheme's colours; `standalone` (over the terminal):
/// with its title and a Close row.
pub fn draw_settings(ui: &mut Ui, row: usize, p: &crate::config::TermPrefs, font_name: Option<&str>, dark: bool, w: f32, standalone: bool) -> Option<(usize, isize)> {
    use super::app::trow;
    let mut ev = None;
    let vis = ui.visuals().clone();
    ui.set_width(w);
    if standalone {
        ui.label(egui::RichText::new(tr("Terminal settings")).strong().size(18.0));
        ui.add_space(6.0);
    }
    ui.spacing_mut().item_spacing.y = 4.0;
    let shell = p.shell.clone().unwrap_or_else(|| format!("{} ({})", tr("System default"), super::shell_name(p)));
    let rows: [(usize, &str, String, bool); 7] = [
        (trow::SCHEME, tr("Colours"), p.scheme.clone(), true),
        (trow::FONT, tr("Font"), font_name.map_or_else(|| tr("System monospace").to_string(), str::to_string), true),
        (trow::SIZE, tr("Size"), format!("{:.0}", p.font_size), true),
        (trow::SHELL, tr("Shell"), shell, true),
        (trow::CURSOR, tr("Cursor"), super::cursor_names()[p.cursor.min(2) as usize].to_string(), true),
        (trow::SCROLLBACK, tr("Scrollback lines"), p.scrollback.to_string(), true),
        (trow::HERE, tr("Start in the panel's folder"), String::new(), false),
    ];
    for (r_i, label, value, arrows) in rows {
        let (rect, r) = ui.allocate_exact_size(vec2(w, 42.0), Sense::click());
        if row == r_i {
            ui.painter().rect(rect, 8.0, crate::view::tint(&vis, 0.25), Stroke::new(1.5, vis.selection.bg_fill), StrokeKind::Inside);
        } else if r.hovered() {
            ui.painter().rect_filled(rect, 8.0, vis.widgets.hovered.weak_bg_fill.gamma_multiply(0.5));
        }
        if r.clicked() {
            ev = Some((r_i, 0));
        }
        ui.painter().text(pos2(rect.min.x + 12.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(15.0), vis.text_color());
        if arrows {
            let right = Rect::from_center_size(pos2(rect.max.x - 22.0, rect.center().y), Vec2::splat(28.0));
            let left = Rect::from_center_size(pos2(rect.max.x - 222.0, rect.center().y), Vec2::splat(28.0));
            for (b, g, d) in [(left, "◀", -1), (right, "▶", 1)] {
                let br = ui.interact(b, ui.id().with(("tstep", r_i, d)), Sense::click());
                ui.painter().rect_filled(b, 6.0, if br.hovered() { vis.widgets.hovered.bg_fill } else { vis.widgets.inactive.bg_fill });
                ui.painter().text(b.center(), Align2::CENTER_CENTER, g, FontId::proportional(12.0), vis.text_color());
                if br.clicked() {
                    ev = Some((r_i, d));
                }
            }
            let mid = pos2((left.max.x + right.min.x) / 2.0, rect.center().y);
            let g = crate::view::elided_text(ui, &value, FontId::proportional(14.0), vis.strong_text_color(), right.min.x - left.max.x - 8.0);
            ui.painter().galley(mid - g.size() / 2.0, g, vis.strong_text_color());
        } else {
            let b = Rect::from_center_size(pos2(rect.max.x - 22.0, rect.center().y), Vec2::splat(20.0));
            ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, vis.strong_text_color()), StrokeKind::Inside);
            if p.here {
                ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), vis.strong_text_color());
            }
        }
    }
    // The scheme's colours.
    ui.add_space(6.0);
    let s = super::scheme::resolve(&p.scheme, dark);
    let (r, _) = ui.allocate_exact_size(vec2(w, 64.0), Sense::hover());
    ui.painter().rect_filled(r, 8.0, c32(s.bg));
    let f = font(p.font_size.min(18.0));
    ui.painter().text(r.min + vec2(12.0, 8.0), Align2::LEFT_TOP, "~ $ ls -la", f.clone(), c32(s.fg));
    let sw = (w - 24.0) / 16.0;
    for (i, c) in s.ansi.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(r.min.x + 12.0 + i as f32 * sw, r.max.y - 22.0), vec2(sw - 2.0, 14.0));
        ui.painter().rect_filled(cell, 3.0, c32(*c));
    }
    if !standalone {
        return ev;
    }
    let (cr, cresp) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click());
    if row == trow::CLOSE {
        ui.painter().rect(cr, 8.0, crate::view::tint(&vis, 0.25), Stroke::new(1.5, vis.selection.bg_fill), StrokeKind::Inside);
    }
    ui.painter().text(cr.center(), Align2::CENTER_CENTER, tr("Close"), FontId::proportional(15.0), vis.text_color());
    if cresp.clicked() {
        ev = Some((trow::CLOSE, 0));
    }
    ev
}
