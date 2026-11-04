//! The torrents page, and the window that adds one.

use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::app::{AddDlg, AddEv, Node, Ticked};
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

/// A file's tile in the add window.
const TILE: Vec2 = vec2(116.0, 112.0);

/// The window adding a torrent: where it goes (◀ ▶ through the places
/// offered, "Other folder…" to type one), its files as tiles to tick.
pub fn draw_add(ui: &mut Ui, d: &mut AddDlg, icons: &mut crate::icons::Icons) -> Option<AddEv> {
    let mut ev = None;
    let width = crate::view::dialog_w().min(620.0);
    ui.set_width(width);
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
        let other_w = ui.painter().layout_no_wrap(tr("Other folder…").to_string(), FontId::proportional(14.0), v.text_color()).size().x + 24.0;
        let (row, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
        let rect = Rect::from_min_max(row.min, pos2(row.max.x - other_w - 8.0, row.max.y));
        let r = ui.interact(rect, ui.id().with("torrent-dest"), Sense::click());
        let target = super::target_folder(d.dest_folder(), &d.name, d.files.len());
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr("Save to")));
        if d.cursor == 0 {
            ui.painter().rect(rect, 6.0, crate::view::tint(&v, 0.3), Stroke::new(1.5, v.selection.bg_fill), StrokeKind::Inside);
        } else if r.hovered() {
            ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
        }
        let lw = ui.painter().text(pos2(rect.min.x + 10.0, rect.center().y), Align2::LEFT_CENTER, tr("Save to"), FontId::proportional(14.0), v.text_color()).width();
        let many = d.dests.len() > 1;
        let aw = if many { 40.0 } else { 0.0 };
        if many {
            ui.painter().text(pos2(rect.max.x - 10.0, rect.center().y), Align2::RIGHT_CENTER, "◀ ▶", FontId::proportional(14.0), v.strong_text_color());
        }
        let lx = rect.min.x + 10.0 + lw + 12.0;
        let g = elided_text(ui, &tilde(&target), FontId::proportional(14.0), v.strong_text_color(), rect.max.x - lx - 14.0 - aw);
        ui.painter().galley(pos2(lx, rect.center().y - g.size().y / 2.0), g, v.strong_text_color());
        if r.clicked() {
            ev = Some(AddEv::Dest);
        }
        let ob = Rect::from_min_max(pos2(row.max.x - other_w, row.min.y + 4.0), pos2(row.max.x, row.max.y - 4.0));
        let or = ui.interact(ob, ui.id().with("torrent-other"), Sense::click());
        or.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr("Other folder…")));
        ui.painter().rect_filled(ob, 6.0, if or.hovered() { v.widgets.hovered.weak_bg_fill } else { v.widgets.inactive.weak_bg_fill });
        ui.painter().text(ob.center(), Align2::CENTER_CENTER, tr("Other folder…"), FontId::proportional(14.0), v.text_color());
        if or.clicked() {
            ev = Some(AddEv::Other);
        }
        ui.add_space(4.0);
        // Its files and folders, as tiles to tick; a folder opens.
        let (n, size) = d.chosen();
        ui.horizontal(|ui| {
            ui.label(format!("{}: {n} / {} · {}", tr("Files"), d.files.len(), human_size(size)));
            if d.files.len() > 1 && ui.button(tr("All / none")).clicked() {
                ev = Some(AddEv::All);
            }
        });
        // Inside a folder: where (each part a way back up).
        if !d.dir.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let parts: Vec<&str> = d.dir.split('/').collect();
                if ui.link(&d.name).clicked() {
                    ev = Some(AddEv::Open(String::new()));
                }
                for (k, p) in parts.iter().enumerate() {
                    ui.label(egui::RichText::new(" › ").weak());
                    if k + 1 == parts.len() {
                        ui.label(egui::RichText::new(*p).strong());
                    } else if ui.link(*p).clicked() {
                        ev = Some(AddEv::Open(parts[..=k].join("/")));
                    }
                }
            });
        }
        let list = d.listing();
        let per_line = ((width - 14.0) / TILE.x).floor().max(1.0) as usize;
        d.per_line = per_line;
        let lines = list.len().div_ceil(per_line);
        let max_h = (ui.ctx().content_rect().height() - 320.0).max(TILE.y + 8.0);
        let scroll = d.scrolled != Some(d.cursor);
        egui::ScrollArea::vertical().id_salt(("torrent-tiles", d.dir.clone())).max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
            let (room, _) = ui.allocate_exact_size(vec2(width - 14.0, lines as f32 * TILE.y), Sense::hover());
            for (k, node) in list.iter().enumerate() {
                let at = room.min + vec2((k % per_line) as f32 * TILE.x, (k / per_line) as f32 * TILE.y);
                let cell = Rect::from_min_size(at, TILE).shrink(3.0);
                let (name, ticked, info) = match node {
                    Node::Up => ("..".to_string(), None, String::new()),
                    Node::Dir(p, n) => {
                        let (t, _, sz) = d.dir_state(p);
                        (n.clone(), Some(t), human_size(sz))
                    }
                    Node::File(i) => {
                        let (path, len, on) = &d.files[*i];
                        let n = std::path::Path::new(path).file_name().map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                        (n, Some(if *on { Ticked::All } else { Ticked::None }), human_size(*len))
                    }
                };
                let r = ui.interact(cell, ui.id().with(("torrent-tile", d.dir.clone(), k)), Sense::click());
                let on = ticked != Some(Ticked::None);
                match node {
                    Node::File(_) => r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, &name)),
                    _ => r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name)),
                }
                if d.cursor == k + 1 {
                    ui.painter().rect(cell, 8.0, crate::view::tint(&v, 0.3), Stroke::new(1.5, v.selection.bg_fill), StrokeKind::Inside);
                    if scroll {
                        ui.scroll_to_rect(cell, None);
                    }
                } else if r.hovered() {
                    ui.painter().rect_filled(cell, 8.0, v.widgets.hovered.weak_bg_fill);
                }
                // Its icon (a folder's, or by the file's name), dimmed when
                // nothing in it is taken.
                let ir = Rect::from_center_size(pos2(cell.center().x, cell.min.y + 28.0), Vec2::splat(40.0));
                let px = crate::view::lookup_px(ui, 40.0);
                let icon = match node {
                    Node::Up => icons.named(&["go-up", "folder-open", "folder"], px),
                    Node::Dir(..) => icons.named(&["folder", "inode-directory"], px),
                    Node::File(_) => {
                        let e = crate::vfs::Entry { name: name.clone(), is_dir: false, is_link: false, size: 0, mtime: None, mode: 0o644, tags: Vec::new() };
                        icons.for_entry(&e, None, px)
                    }
                };
                if let Some(u) = icon {
                    egui::Image::new(u).tint(if on { Color32::WHITE } else { Color32::from_white_alpha(90) }).paint_at(ui, ir);
                }
                // A folder: how many files in it, on its icon.
                if let Node::Dir(p, _) = node {
                    let count = d.under(p).len();
                    let g = ui.painter().layout_no_wrap(count.to_string(), FontId::proportional(11.0), Color32::WHITE);
                    let pill = Rect::from_center_size(ir.right_bottom() - vec2(2.0, 4.0), vec2(g.size().x + 10.0, g.size().y + 3.0));
                    ui.painter().rect_filled(pill, pill.height() / 2.0, v.selection.bg_fill);
                    ui.painter().galley(pill.center() - g.size() / 2.0, g, Color32::WHITE);
                    let _ = ui.interact(pill, ui.id().with(("torrent-dir-count", p)), Sense::hover()).on_hover_text(crate::i18n::trn("%n files", count));
                }
                let c = if on { v.text_color() } else { v.weak_text_color() };
                // (All of it within the tile: cut short, never over the next.)
                let p = ui.painter().with_clip_rect(cell.intersect(ui.clip_rect()));
                let g = crate::view::elided_centered(ui, &name, FontId::proportional(12.5), c, cell.width() - 10.0, 2);
                let gh = g.size().y;
                p.galley(pos2(cell.center().x, ir.max.y + 4.0), g, c);
                let g = crate::view::elided_centered(ui, &info, FontId::proportional(11.0), v.weak_text_color(), cell.width() - 10.0, 1);
                p.galley(pos2(cell.center().x, ir.max.y + 6.0 + gh), g, v.weak_text_color());
                // The tick, top left: ✔ all, – some (a folder), empty none.
                let mut on_tick = false;
                if let Some(t) = ticked {
                    let b = Rect::from_min_size(cell.min + vec2(6.0, 6.0), Vec2::splat(18.0));
                    let hit = b.expand(6.0);
                    on_tick = ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| hit.contains(p));
                    let full = t != Ticked::None;
                    ui.painter().rect_filled(b, 4.0, if full { v.selection.bg_fill } else { v.extreme_bg_color });
                    ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, if full { v.selection.bg_fill } else { v.weak_text_color() }), StrokeKind::Inside);
                    match t {
                        Ticked::All => {
                            ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(13.0), Color32::WHITE);
                        }
                        Ticked::Some => {
                            ui.painter().line_segment([b.center() - vec2(4.5, 0.0), b.center() + vec2(4.5, 0.0)], Stroke::new(2.0, Color32::WHITE));
                        }
                        Ticked::None => {}
                    }
                    if let Node::Dir(p, _) = node {
                        // (The tick itself: all of it in or out.)
                        let tr_ = ui.interact(hit, ui.id().with(("torrent-dir-tick", p)), Sense::click());
                        tr_.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, t == Ticked::All, &name));
                        if tr_.clicked() {
                            ev = Some(AddEv::ToggleDir(p.clone()));
                        }
                    }
                }
                // (A file in a folder of the torrent: its whole path.)
                let r = match node {
                    Node::File(i) if d.files[*i].0.contains('/') => r.on_hover_text(&d.files[*i].0),
                    _ => r,
                };
                if r.clicked() && !(on_tick && matches!(node, Node::Dir(..))) {
                    ev = Some(match node {
                        Node::Up => AddEv::Up,
                        Node::Dir(p, _) => AddEv::Open(p.clone()),
                        Node::File(i) => AddEv::Toggle(*i),
                    });
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
