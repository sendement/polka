//! The torrents page, and the window that adds one.

use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::app::{AddDlg, AddEv};
use super::{Item, State};
use crate::app::human_size;
use crate::i18n::tr;
use crate::view::{elided_text, paint_row_bg};

/// What a click on the page asks.
pub enum Act {
    Close,
    Magnet,
    Pick(usize),
    Toggle(usize),
    Reveal(usize),
    /// Right click or its ⋯: pick, then the actions.
    Context(usize),
}

fn header_button(ui: &mut Ui, r: Rect, label: &str, tip: &str) -> bool {
    let vis = ui.visuals().clone();
    let resp = ui.interact(r, ui.id().with(("torrent-btn", label)), Sense::click()).on_hover_text(tip);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
    if resp.hovered() {
        ui.painter().rect_filled(r, 6.0, vis.widgets.hovered.weak_bg_fill);
    }
    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(15.0), vis.text_color());
    resp.clicked()
}

/// How a torrent is doing, in words.
pub fn state_line(t: &Item) -> String {
    let size = if t.finished() { human_size(t.total) } else { format!("{} / {}", human_size(t.done), human_size(t.total)) };
    let pct = format!("{:.0}%", t.fraction() * 100.0);
    match &t.state {
        State::Starting => format!("{} · {}", tr("Starting…"), size),
        State::Paused => format!("{} · {pct} · {size}", tr("Paused")),
        State::Done => format!("{} · {size}", tr("Downloaded")),
        State::Error(e) => format!("{}: {e}", tr("Error")),
        State::Seeding => format!("{} · {size} · ↑ {} · {} {}", tr("Sharing"), super::speed(t.up), tr("peers:"), t.peers),
        State::Downloading => {
            let mut s = format!("{pct} · {size} · ↓ {} ↑ {} · {} {}", super::speed(t.down), super::speed(t.up), tr("peers:"), t.peers);
            if let Some(e) = t.eta {
                s.push_str(&format!(" · {} {}", super::eta(e), tr("left")));
            }
            s
        }
    }
}

pub fn draw(ui: &mut Ui, items: &[Item], cursor: usize, error: Option<&str>, focused: bool, icons: &mut crate::icons::Icons) -> Option<Act> {
    let mut act = None;
    let vis = ui.visuals().clone();
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let title = ui.painter().layout_no_wrap(tr("Torrents").into(), FontId::proportional(16.0), vis.strong_text_color());
    let x = head.min.x + 6.0;
    ui.painter().galley(pos2(x, head.center().y - title.size().y / 2.0), title.clone(), vis.strong_text_color());
    // All of them together: how fast.
    let (down, up) = items.iter().fold((0.0, 0.0), |(d, u), t| (d + t.down, u + t.up));
    if !items.is_empty() {
        let sum = format!("↓ {}   ↑ {}", super::speed(down), super::speed(up));
        ui.painter().text(pos2(x + title.size().x + 16.0, head.center().y), Align2::LEFT_CENTER, sum, FontId::proportional(13.5), vis.weak_text_color());
    }
    let mut bx = head.max.x - 30.0;
    let btn = |bx: f32, w: f32| Rect::from_min_size(pos2(bx, head.min.y + 3.0), vec2(w, 28.0));
    if header_button(ui, btn(bx, 28.0), "✕", tr("Close")) {
        act = Some(Act::Close);
    }
    let label = format!("+ {}", tr("Magnet link"));
    let w = ui.painter().layout_no_wrap(label.clone(), FontId::proportional(15.0), vis.text_color()).size().x + 20.0;
    bx -= w + 6.0;
    if header_button(ui, btn(bx, w), &label, tr("Add a magnet link…")) {
        act = Some(Act::Magnet);
    }
    ui.add_space(4.0);
    let area = ui.available_rect_before_wrap();
    ui.allocate_rect(area, Sense::hover());
    if let Some(e) = error {
        ui.painter().text(area.center(), Align2::CENTER_CENTER, format!("{}: {e}", tr("The torrent client couldn't start")), FontId::proportional(15.0), vis.error_fg_color);
        return act;
    }
    if items.is_empty() {
        let c = area.center();
        ui.painter().text(pos2(c.x, c.y - 30.0), Align2::CENTER_CENTER, tr("No torrents yet"), FontId::proportional(20.0), vis.strong_text_color());
        ui.painter().text(pos2(c.x, c.y + 2.0), Align2::CENTER_CENTER, tr("Open a .torrent file in the panes, or add a magnet link."), FontId::proportional(14.0), vis.weak_text_color());
        return act;
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    egui::ScrollArea::vertical().id_salt("torrents").auto_shrink([false, false]).show(&mut child, |ui| {
        for (i, t) in items.iter().enumerate() {
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 66.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, i == cursor, &t.name));
            let here = i == cursor;
            paint_row_bg(ui, rect, here && focused, here && !focused, resp.hovered());
            if here && focused {
                resp.scroll_to_me(None);
            }
            // Its buttons, right.
            let mut rx = rect.max.x - 8.0;
            // (A theme icon if there's one, else the glyph.)
            let mut button = |ui: &mut Ui, glyph: &str, icon: &'static [&'static str], tip: &str| {
                rx -= 32.0;
                let b = Rect::from_center_size(pos2(rx + 16.0, rect.center().y), Vec2::splat(30.0));
                let r = ui.interact(b, ui.id().with(("torrent-row", i, glyph)), Sense::click()).on_hover_text(tip);
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
                if r.hovered() {
                    ui.painter().rect_filled(b, 6.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                match (!icon.is_empty()).then(|| icons.named(icon, crate::view::lookup_px(ui, 18.0))).flatten() {
                    Some(u) => egui::Image::new(u).paint_at(ui, Rect::from_center_size(b.center(), Vec2::splat(18.0))),
                    None => {
                        ui.painter().text(b.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(16.0), ui.visuals().text_color());
                    }
                }
                r.clicked()
            };
            if button(ui, "⋯", &[], tr("Actions")) {
                act = Some(Act::Context(i));
            }
            if button(ui, "↗", &["folder-open", "document-open-folder", "folder"], tr("Show in folder")) {
                act = Some(Act::Reveal(i));
            }
            let paused = matches!(t.state, State::Paused | State::Done | State::Error(_));
            if button(ui, if paused { "▶" } else { "⏸" }, &[], if paused { tr("Resume") } else { tr("Pause") }) {
                act = Some(Act::Toggle(i));
            }
            // Name, a bar, how it's doing.
            let tx = rect.min.x + 14.0;
            let room = rx - tx - 10.0;
            let g = elided_text(ui, &t.name, FontId::proportional(15.5), vis.strong_text_color(), room);
            ui.painter().galley(pos2(tx, rect.min.y + 8.0), g, vis.strong_text_color());
            let bar = Rect::from_min_size(pos2(tx, rect.min.y + 32.0), vec2(room, 6.0));
            ui.painter().rect_filled(bar, 3.0, vis.extreme_bg_color);
            let fill = match t.state {
                State::Error(_) => vis.error_fg_color,
                State::Paused | State::Done => vis.weak_text_color(),
                State::Seeding => Color32::from_rgb(0x4c, 0xaf, 0x50),
                _ => vis.selection.bg_fill,
            };
            ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * t.fraction().clamp(0.0, 1.0), bar.height())), 3.0, fill);
            let line = elided_text(ui, &state_line(t), FontId::proportional(13.0), vis.text_color(), room);
            ui.painter().galley(pos2(tx, rect.min.y + 44.0), line, vis.text_color());
            if act.is_none() {
                if resp.secondary_clicked() {
                    act = Some(Act::Context(i));
                } else if resp.double_clicked() {
                    act = Some(Act::Reveal(i));
                } else if resp.clicked() {
                    act = Some(Act::Pick(i));
                }
            }
        }
    });
    act
}

/// A path with the home folder as "~".
fn tilde(p: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|h| p.strip_prefix(&h).ok().map(|r| r.to_path_buf())) {
        Some(r) if r.as_os_str().is_empty() => "~".into(),
        Some(r) => format!("~/{}", r.display()),
        None => p.display().to_string(),
    }
}

/// The window adding a torrent: where it goes, its files to choose.
pub fn draw_add(ui: &mut Ui, d: &mut AddDlg, folder: &std::path::Path) -> Option<AddEv> {
    let mut ev = None;
    ui.set_width(crate::view::dialog_w().min(560.0));
    let v = ui.visuals().clone();
    ui.label(egui::RichText::new(tr("Download a torrent")).strong().size(18.0));
    let head = if d.name.is_empty() { d.title.clone() } else { d.name.clone() };
    ui.label(egui::RichText::new(head).weak());
    ui.add_space(6.0);
    if let Some(e) = &d.error {
        ui.label(egui::RichText::new(e).color(v.error_fg_color));
    } else if !d.ready() {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(18.0));
            ui.label(if d.magnet { tr("Looking for peers that have its list of files…") } else { tr("Reading it…") });
        });
    } else {
        // Where it goes.
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
        let place = match (&d.here, d.to_here) {
            (Some(h), true) => h.clone(),
            _ => folder.to_path_buf(),
        };
        let target = super::target_folder(&place, &d.name, d.files.len());
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr("Save to")));
        if d.cursor == 0 {
            ui.painter().rect(rect, 6.0, crate::view::tint(&v, 0.3), Stroke::new(1.5, v.selection.bg_fill), StrokeKind::Inside);
        } else if r.hovered() {
            ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
        }
        let lw = ui.painter().text(pos2(rect.min.x + 10.0, rect.center().y), Align2::LEFT_CENTER, tr("Save to"), FontId::proportional(14.0), v.text_color()).width();
        let arrows = if d.here.is_some() { "◀ ▶" } else { "" };
        let aw = if arrows.is_empty() { 0.0 } else { 40.0 };
        ui.painter().text(pos2(rect.max.x - 10.0, rect.center().y), Align2::RIGHT_CENTER, arrows, FontId::proportional(14.0), v.strong_text_color());
        let lx = rect.min.x + 10.0 + lw + 12.0;
        let g = elided_text(ui, &tilde(&target), FontId::proportional(14.0), v.strong_text_color(), rect.max.x - lx - 14.0 - aw);
        ui.painter().galley(pos2(lx, rect.center().y - g.size().y / 2.0), g, v.strong_text_color());
        if r.clicked() {
            ev = Some(AddEv::Dest);
        }
        ui.add_space(4.0);
        // Its files.
        let (n, size) = d.chosen();
        ui.horizontal(|ui| {
            ui.label(format!("{}: {n} / {} · {}", tr("Files"), d.files.len(), human_size(size)));
            if d.files.len() > 1 && ui.button(tr("All / none")).clicked() {
                ev = Some(AddEv::All);
            }
        });
        let max_h = (ui.ctx().content_rect().height() - 300.0).max(140.0);
        let scroll = d.scrolled != Some(d.cursor);
        egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
            for (i, (path, len, on)) in d.files.iter().enumerate() {
                let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, *on, path));
                if d.cursor == i + 1 {
                    ui.painter().rect(rect, 6.0, crate::view::tint(&v, 0.3), Stroke::new(1.5, v.selection.bg_fill), StrokeKind::Inside);
                    if scroll {
                        r.scroll_to_me(None);
                    }
                } else if r.hovered() {
                    ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
                }
                let b = Rect::from_center_size(pos2(rect.min.x + 16.0, rect.center().y), Vec2::splat(16.0));
                ui.painter().rect_stroke(b, 3.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
                if *on {
                    ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(13.0), v.strong_text_color());
                }
                let sz = human_size(*len);
                let sw = ui.painter().layout_no_wrap(sz.clone(), FontId::proportional(13.0), v.weak_text_color()).size().x;
                ui.painter().text(pos2(rect.max.x - 8.0, rect.center().y), Align2::RIGHT_CENTER, sz, FontId::proportional(13.0), v.weak_text_color());
                let c = if *on { v.text_color() } else { v.weak_text_color() };
                let g = elided_text(ui, path, FontId::proportional(14.0), c, rect.width() - 50.0 - sw - 16.0);
                ui.painter().galley(pos2(rect.min.x + 34.0, rect.center().y - g.size().y / 2.0), g, c);
                if r.clicked() {
                    ev = Some(AddEv::Toggle(i));
                }
            }
        });
        if scroll {
            d.scrolled = Some(d.cursor);
        }
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let can = d.ready() && d.chosen().0 > 0;
            if ui.add_enabled(can, egui::Button::new(tr("Download"))).clicked() {
                ev = Some(AddEv::Download);
            }
            if ui.button(tr("Cancel")).clicked() {
                ev = Some(AddEv::Cancel);
            }
        });
    });
    ev
}
