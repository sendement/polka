//! Tagging the chosen files: every tag offered with a check (all of them
//! have it, some do, none), a new one typed in, a search for one.

use std::path::PathBuf;

use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::app::{App, Focus, Osk, OskPurpose, Overlay};
use crate::i18n::{tr, trn};
use crate::input::Action;
use crate::vfs::Location;

pub struct TagEditor {
    pub paths: Vec<PathBuf>,
    /// Offered: the tags known so far and those the items have.
    pub tags: Vec<String>,
    /// How many of the items have each.
    pub count: Vec<usize>,
    /// Over the tags, then "New tag…" (at `tags.len()`).
    pub cursor: usize,
}

pub enum TagEv {
    Toggle(usize),
    New,
    Find(usize),
    Close,
}

impl TagEditor {
    fn new(paths: Vec<PathBuf>) -> Self {
        let mut ed = TagEditor { paths, tags: crate::tags::known(), count: Vec::new(), cursor: 0 };
        ed.recount();
        ed
    }

    /// The items' tags again (after a change).
    fn recount(&mut self) {
        let on: Vec<Vec<String>> = self.paths.iter().map(|p| crate::tags::read(p, true).0).collect();
        for t in on.iter().flatten() {
            if !self.tags.contains(t) {
                self.tags.push(t.clone());
            }
        }
        self.tags.sort_by_key(|t| t.to_lowercase());
        self.count = self.tags.iter().map(|t| on.iter().filter(|o| o.contains(t)).count()).collect();
        self.cursor = self.cursor.min(self.tags.len());
    }
}

impl App {
    /// "Tags…" on the chosen items.
    pub fn tags_open(&mut self) {
        let paths = self.local_targets();
        if paths.is_empty() {
            return self.toast(tr("Only files on disk can have tags"), true);
        }
        self.overlay = Some(Overlay::Tags(TagEditor::new(paths)));
    }

    pub fn tags_event(&mut self, ev: TagEv) {
        let Some(Overlay::Tags(ed)) = self.overlay.as_mut() else { return };
        match ev {
            TagEv::Toggle(i) => {
                ed.cursor = i;
                let Some(tag) = ed.tags.get(i).cloned() else { return };
                // All have it: off everywhere; else on everywhere.
                let on = ed.count[i] < ed.paths.len();
                let paths = ed.paths.clone();
                self.tag_items(&paths, &[tag], on);
            }
            TagEv::New => {
                let Some(Overlay::Tags(ed)) = self.overlay.take() else { return };
                self.overlay = Some(Overlay::Osk(Osk::new(tr("New tag"), "", OskPurpose::NewTag { ed: Box::new(ed) })));
            }
            TagEv::Find(i) => {
                let Some(tag) = ed.tags.get(i).cloned() else { return };
                let first = ed.paths[0].parent().map(|d| d.to_path_buf()).unwrap_or_else(|| "/".into());
                let root = self.pane().loc.local_path().map_or(first, |p| p.to_path_buf());
                self.overlay = None;
                self.focus = Focus::Panes;
                let query = if tag.contains(' ') { tag.replace(' ', "") } else { tag };
                let cfg = self.cfg.clone();
                self.pane_mut().navigate(Location::Search { root, query: format!("#{query}"), hidden: cfg.show_hidden }, None, &cfg);
            }
            TagEv::Close => self.overlay = None,
        }
    }

    /// Put `tags` on the items (or take them off), then show it.
    fn tag_items(&mut self, paths: &[PathBuf], tags: &[String], on: bool) {
        let mut failed = 0;
        for p in paths {
            let (mut have, _) = crate::tags::read(p, true);
            for t in tags {
                if on && !have.contains(t) {
                    have.push(t.clone());
                } else if !on {
                    have.retain(|x| x != t);
                }
            }
            if let Err(e) = crate::tags::write(p, &have) {
                failed += 1;
                crate::log::line(format!("tags on {}: {e}", p.display()));
            }
        }
        if failed > 0 {
            self.toast(trn("Couldn't tag %n item(s)", failed), true);
        }
        if let Some(Overlay::Tags(ed)) = self.overlay.as_mut() {
            ed.recount();
        }
        self.reload_all();
    }

    /// A new tag typed in: onto the items, then back to the list.
    pub fn tags_typed(&mut self, mut ed: TagEditor, text: &str) {
        let new = crate::tags::parse(text);
        self.overlay = None;
        if !new.is_empty() {
            let paths = ed.paths.clone();
            self.tag_items(&paths, &new, true);
            ed.recount();
            ed.cursor = ed.tags.iter().position(|t| *t == new[0]).unwrap_or(ed.cursor);
        }
        self.overlay = Some(Overlay::Tags(ed));
    }

    pub fn tags_input(&mut self, a: Action) {
        let Some(Overlay::Tags(ed)) = self.overlay.as_mut() else { return };
        let n = ed.tags.len();
        match a {
            Action::Up => ed.cursor = ed.cursor.saturating_sub(1),
            Action::Down => ed.cursor = (ed.cursor + 1).min(n),
            Action::Accept if ed.cursor == n => self.tags_event(TagEv::New),
            Action::Accept => {
                let i = ed.cursor;
                self.tags_event(TagEv::Toggle(i));
            }
            Action::Actions => self.tags_event(TagEv::New),
            Action::Select if ed.cursor < n => {
                let i = ed.cursor;
                self.tags_event(TagEv::Find(i));
            }
            Action::Back | Action::Menu => self.tags_event(TagEv::Close),
            _ => {}
        }
    }
}

pub fn draw(ui: &mut Ui, ed: &mut TagEditor) -> Option<TagEv> {
    let mut ev = None;
    let v = ui.visuals().clone();
    ui.set_min_width(380.0);
    ui.label(egui::RichText::new(tr("Tags")).strong().size(18.0));
    let what = match ed.paths.as_slice() {
        [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        many => trn("%n items", many.len()),
    };
    ui.label(egui::RichText::new(what).weak());
    ui.add_space(6.0);
    let n = ed.tags.len();
    egui::ScrollArea::vertical().id_salt("tags").max_height(340.0).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        if n == 0 {
            ui.label(egui::RichText::new(tr("No tags yet")).weak());
        }
        for i in 0..=n {
            let (rect, r) = ui.allocate_exact_size(vec2(360.0, 32.0), Sense::click());
            let cur = i == ed.cursor;
            if cur {
                ui.painter().rect(rect, 6.0, crate::view::tint(&v, 0.45), Stroke::new(1.5, v.selection.bg_fill), StrokeKind::Inside);
            } else if r.hovered() {
                ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
            }
            if i == n {
                ui.painter().text(pos2(rect.min.x + 12.0, rect.center().y), Align2::LEFT_CENTER, format!("+  {}", tr("New tag…")), FontId::proportional(15.0), v.text_color());
                if r.clicked() {
                    ev = Some(TagEv::New);
                }
                continue;
            }
            // A check: all, some (a dash) or none of the items have it.
            let cb = Rect::from_center_size(pos2(rect.min.x + 18.0, rect.center().y), Vec2::splat(16.0));
            let (all, some) = (ed.count[i] == ed.paths.len(), ed.count[i] > 0);
            ui.painter().rect(cb, 3.0, if some { v.selection.bg_fill } else { Color32::TRANSPARENT }, Stroke::new(1.5, if some { v.selection.bg_fill } else { v.weak_text_color() }), StrokeKind::Inside);
            if all {
                ui.painter().line(vec![pos2(cb.min.x + 3.5, cb.center().y), pos2(cb.center().x - 1.0, cb.max.y - 4.0), pos2(cb.max.x - 3.0, cb.min.y + 4.0)], Stroke::new(2.0, Color32::WHITE));
            } else if some {
                ui.painter().line_segment([pos2(cb.min.x + 4.0, cb.center().y), pos2(cb.max.x - 4.0, cb.center().y)], Stroke::new(2.0, Color32::WHITE));
            }
            let dot = pos2(rect.min.x + 40.0, rect.center().y);
            ui.painter().circle_filled(dot, 5.0, crate::tags::color(&ed.tags[i], v.dark_mode));
            ui.painter().text(pos2(rect.min.x + 54.0, rect.center().y), Align2::LEFT_CENTER, &ed.tags[i], FontId::proportional(15.0), v.text_color());
            // Look for it (the mouse's way; □ on the pad).
            let find = Rect::from_center_size(pos2(rect.max.x - 18.0, rect.center().y), Vec2::splat(26.0));
            let fr = ui.interact(find, ui.id().with(("tag-find", i)), Sense::click());
            if r.hovered() || fr.hovered() || cur {
                if fr.hovered() {
                    ui.painter().rect_filled(find, 5.0, v.widgets.hovered.bg_fill);
                }
                ui.painter().text(find.center(), Align2::CENTER_CENTER, "🔍", FontId::proportional(14.0), v.text_color());
            }
            if fr.on_hover_text(tr("Find")).clicked() {
                ev = Some(TagEv::Find(i));
            } else if r.clicked() {
                ev = Some(TagEv::Toggle(i));
            }
        }
    });
    ui.add_space(6.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.button(tr("Done")).clicked() {
            ev = Some(TagEv::Close);
        }
    });
    ev
}
