//! The "Convert…" dialog: the chosen files sorted into pictures, sound,
//! video and archives, each group with a format of its own (a mixed
//! choice converts each kind into what fits it), the quality, and what
//! happens to the originals.

use std::collections::HashSet;
use std::path::PathBuf;

use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::app::{App, Overlay};
use crate::convert::{Group, QUALITIES, Quality, Target};
use crate::i18n::{tr, trn};
use crate::input::Action;
use crate::ops::Op;

pub struct GroupRow {
    pub group: Group,
    pub items: Vec<PathBuf>,
    pub targets: Vec<Target>,
    pub pick: usize,
}

impl GroupRow {
    pub fn target(&self) -> Target {
        self.targets[self.pick]
    }

    /// Already in the chosen format (left as they are).
    pub fn already(&self) -> usize {
        self.items.iter().filter(|p| crate::convert::needless(p, self.target())).count()
    }
}

pub struct ConvertDlg {
    pub groups: Vec<GroupRow>,
    pub quality: Quality,
    /// The originals that converted go to the trash.
    pub trash: bool,
    /// Left out, and why (password-protected, a tool missing…).
    pub refused: Vec<(String, &'static str)>,
    /// Neither of the kinds (folders, text…).
    pub other: usize,
    /// The groups' rows, then quality, originals, the buttons.
    pub row: usize,
    /// On the buttons' row: Convert, Cancel.
    pub button: usize,
}

impl ConvertDlg {
    pub fn quality_row(&self) -> usize {
        self.groups.len()
    }
    pub fn trash_row(&self) -> usize {
        self.groups.len() + 1
    }
    pub fn buttons_row(&self) -> usize {
        self.groups.len() + 2
    }

    fn step(&mut self, row: usize, d: isize) {
        if let Some(g) = self.groups.get_mut(row) {
            g.pick = (g.pick as isize + d).rem_euclid(g.targets.len() as isize) as usize;
        } else if row == self.quality_row() {
            let at = QUALITIES.iter().position(|q| *q == self.quality).unwrap_or(1) as isize;
            self.quality = QUALITIES[(at + d).clamp(0, 2) as usize];
        } else if row == self.trash_row() {
            self.trash = !self.trash;
        } else {
            self.button = (self.button as isize + d).clamp(0, 1) as usize;
        }
    }
}

fn group_name(g: Group) -> &'static str {
    match g {
        Group::Image => tr("Image"),
        Group::Audio => tr("Audio"),
        Group::Video => tr("Video"),
        Group::Archive => tr("Archive"),
    }
}

fn quality_name(q: Quality) -> &'static str {
    match q {
        Quality::High => tr("High"),
        Quality::Balanced => tr("Balanced"),
        Quality::Small => tr("Small size"),
    }
}

impl App {
    /// "Convert…" on the chosen files.
    pub fn convert_open(&mut self) {
        let paths = self.local_targets();
        let mut groups: Vec<GroupRow> = Vec::new();
        let mut refused = Vec::new();
        let mut other = 0;
        let mut sets = HashSet::new();
        for p in paths {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(g) = crate::convert::group_of(&name).filter(|_| !p.is_dir()) else {
                other += 1;
                continue;
            };
            // A multi-volume set is one archive: its first volume.
            let p = if g == Group::Archive {
                let first = crate::volumes::first(&p);
                if !sets.insert(first.clone()) {
                    continue;
                }
                first
            } else {
                p
            };
            if let Some(why) = crate::convert::refusal(&p, &name) {
                refused.push((name, why));
                continue;
            }
            match groups.iter_mut().find(|r| r.group == g) {
                Some(r) => r.items.push(p),
                None => {
                    let targets = crate::convert::targets(g);
                    if targets.is_empty() {
                        refused.push((name, tr("needs ffmpeg")));
                        continue;
                    }
                    // The last choice for this kind, else a usual one: HEIC
                    // from a phone mostly becomes JPEG.
                    let usual = match g {
                        Group::Image if name.to_lowercase().ends_with(".heic") => Target::Jpg,
                        Group::Image => Target::Png,
                        Group::Audio => Target::Mp3,
                        Group::Video => Target::Mp4,
                        Group::Archive => Target::Archive(crate::ops::ArchiveFormat::Zip),
                    };
                    let want = self.convert_last.iter().find(|(k, _)| *k == g).map(|(_, t)| *t).unwrap_or(usual);
                    let pick = targets.iter().position(|t| *t == want).unwrap_or(0);
                    groups.push(GroupRow { group: g, items: vec![p], targets, pick });
                }
            }
        }
        groups.sort_by_key(|r| crate::convert::GROUPS.iter().position(|g| *g == r.group));
        if groups.is_empty() {
            let why = refused.first().map(|(n, w)| format!("{n}: {w}")).unwrap_or_else(|| tr("Nothing here can be converted").to_string());
            return self.toast(why, true);
        }
        self.overlay = Some(Overlay::Convert(Box::new(ConvertDlg { groups, quality: Quality::Balanced, trash: false, refused, other, row: 0, button: 0 })));
    }

    pub fn convert_input(&mut self, a: Action) {
        let Some(Overlay::Convert(d)) = self.overlay.as_mut() else { return };
        let last = d.buttons_row();
        match a {
            Action::Up => d.row = d.row.saturating_sub(1),
            Action::Down => d.row = (d.row + 1).min(last),
            Action::Left | Action::Right => {
                let row = d.row;
                d.step(row, if a == Action::Left { -1 } else { 1 });
            }
            Action::Accept if d.row == last => {
                if d.button == 0 {
                    self.convert_start();
                } else {
                    self.overlay = None;
                }
            }
            Action::Accept => {
                let row = d.row;
                d.step(row, 1);
            }
            Action::Menu => self.convert_start(),
            Action::Back => self.overlay = None,
            _ => {}
        }
    }

    /// Off it goes, as a job.
    pub fn convert_start(&mut self) {
        let Some(Overlay::Convert(d)) = self.overlay.take() else { return };
        let mut items = Vec::new();
        for g in &d.groups {
            let t = g.target();
            self.convert_last.retain(|(k, _)| *k != g.group);
            self.convert_last.push((g.group, t));
            items.extend(g.items.iter().filter(|p| !crate::convert::needless(p, t)).map(|p| crate::convert::Item { src: p.clone(), target: t }));
        }
        if items.is_empty() {
            return self.toast(tr("Already in that format"), false);
        }
        self.pane_mut().selected.clear();
        self.start(Op::Convert { items, quality: d.quality, trash: d.trash });
    }
}

/// The dialog: (row, step) for a click; step 0 activates, 10+ a button,
/// 100+ a chip.
pub fn draw(ui: &mut Ui, d: &ConvertDlg) -> Option<(usize, isize)> {
    let mut ev = None;
    let v = ui.visuals().clone();
    let font = FontId::proportional(15.0);
    ui.label(egui::RichText::new(tr("Convert")).strong().size(18.0));
    ui.add_space(8.0);
    ui.spacing_mut().item_spacing.y = 4.0;
    // Chips from the left of a column on the right, wrapping as needed.
    const CHIPS_X: f32 = 150.0;
    let widths = |ui: &Ui, names: &[String]| -> Vec<f32> { names.iter().map(|n| ui.painter().layout_no_wrap(n.clone(), FontId::proportional(13.0), Color32::WHITE).size().x + 16.0).collect() };
    let lines = |ui: &Ui, names: &[String], room: f32| -> usize {
        let (mut n, mut x) = (1, 0.0);
        for w in widths(ui, names) {
            if x > 0.0 && x + w > room {
                n += 1;
                x = 0.0;
            }
            x += w + 4.0;
        }
        n
    };
    let chips = |ui: &mut Ui, rect: Rect, row: usize, names: Vec<(String, bool)>, ev: &mut Option<(usize, isize)>| {
        let labels: Vec<String> = names.iter().map(|n| n.0.clone()).collect();
        let ws = widths(ui, &labels);
        let (x0, x1) = (rect.min.x + CHIPS_X, rect.max.x - 10.0);
        let mut x = x0;
        let mut y = rect.min.y + 22.0;
        for (i, (name, on)) in names.iter().enumerate() {
            let w = ws[i];
            if x > x0 && x + w > x1 {
                x = x0;
                y += 30.0;
            }
            let chip = Rect::from_min_size(pos2(x, y - 13.0), vec2(w, 26.0));
            x += w + 4.0;
            let cr = ui.interact(chip, ui.id().with(("conv-chip", row, i)), Sense::click());
            let fill = if *on { v.selection.bg_fill } else if cr.hovered() { v.widgets.hovered.bg_fill } else { v.widgets.inactive.bg_fill };
            ui.painter().rect(chip, 6.0, fill, Stroke::NONE, StrokeKind::Inside);
            ui.painter().text(chip.center(), Align2::CENTER_CENTER, name, FontId::proportional(13.0), if *on { v.selection.stroke.color } else { v.text_color() });
            if cr.clicked() {
                *ev = Some((row, 100 + i as isize));
            }
        }
    };
    for (i, g) in d.groups.iter().enumerate() {
        // Tall enough for its lines of formats (video has ten).
        let names: Vec<String> = g.targets.iter().map(|t| t.label().to_string()).collect();
        let h = 24.0 + 30.0 * lines(ui, &names, crate::view::SETTINGS_W - CHIPS_X - 10.0) as f32;
        let (rect, r) = crate::view::settings_row(ui, h.max(54.0), d.row == i, ("conv", i));
        if r.clicked() {
            ev = Some((i, 0));
        }
        ui.painter().text(pos2(rect.min.x + 14.0, rect.min.y + 18.0), Align2::LEFT_CENTER, format!("{} · {}", group_name(g.group), g.items.len()), font.clone(), v.text_color());
        let already = g.already();
        if already > 0 {
            ui.painter().text(pos2(rect.min.x + 14.0, rect.min.y + 38.0), Align2::LEFT_CENTER, trn("Already in this format: %n", already), FontId::proportional(12.0), v.weak_text_color());
        }
        chips(ui, rect, i, g.targets.iter().enumerate().map(|(k, t)| (t.label().to_string(), k == g.pick)).collect(), &mut ev);
    }
    // Quality (for the lossy formats).
    let row = d.quality_row();
    let (rect, r) = crate::view::settings_row(ui, 44.0, d.row == row, ("conv", row));
    if r.clicked() {
        ev = Some((row, 0));
    }
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Quality"), font.clone(), v.text_color());
    chips(ui, Rect::from_min_max(pos2(rect.min.x, rect.center().y - 22.0), rect.max), row, QUALITIES.iter().map(|q| (quality_name(*q).to_string(), *q == d.quality)).collect(), &mut ev);
    // The originals.
    let row = d.trash_row();
    let (rect, r) = crate::view::settings_row(ui, 44.0, d.row == row, ("conv", row));
    if r.clicked() {
        ev = Some((row, 0));
    }
    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Move the originals to the trash"), font.clone(), v.text_color());
    let b = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(20.0));
    ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
    if d.trash {
        ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), v.strong_text_color());
    }
    // What's left out.
    let mut notes = Vec::new();
    if !d.refused.is_empty() {
        let first = &d.refused[0];
        notes.push(if d.refused.len() == 1 { format!("{}: {}", first.0, first.1) } else { format!("{} ({}: {}, …)", trn("Left out: %n", d.refused.len()), first.0, first.1) });
    }
    if d.other > 0 {
        notes.push(trn("Not convertible: %n", d.other));
    }
    for n in notes {
        ui.label(egui::RichText::new(n).color(v.warn_fg_color));
    }
    // The buttons (the pad: ← → and ✕ on their row, or Start).
    let row = d.buttons_row();
    let (rect, _) = crate::view::settings_row(ui, 44.0, d.row == row, ("conv", row));
    for (i, n) in [tr("Convert"), tr("Cancel")].iter().enumerate() {
        let w = 150.0;
        let b = Rect::from_min_size(pos2(rect.max.x - (2 - i) as f32 * (w + 8.0), rect.center().y - 15.0), vec2(w, 30.0));
        let br = ui.interact(b, ui.id().with(("conv-btn", i)), Sense::click());
        let on = (d.row == row && d.button == i) || (i == 0 && d.row != row);
        let fill = if on { v.selection.bg_fill } else if br.hovered() { v.widgets.hovered.bg_fill } else { v.widgets.inactive.bg_fill };
        ui.painter().rect(b, 6.0, fill, Stroke::NONE, StrokeKind::Inside);
        ui.painter().text(b.center(), Align2::CENTER_CENTER, *n, font.clone(), if on { v.selection.stroke.color } else { v.text_color() });
        if br.clicked() {
            ev = Some((row, 10 + i as isize));
        }
    }
    ev
}

/// A click in the dialog.
pub fn clicked(app: &mut App, row: usize, step: isize) {
    let Some(Overlay::Convert(d)) = app.overlay.as_mut() else { return };
    d.row = row;
    match step {
        0 => {
            if row == d.trash_row() {
                d.trash = !d.trash;
            }
        }
        10 => app.convert_start(),
        11 => app.overlay = None,
        _ => {
            let i = (step - 100) as usize;
            if let Some(g) = d.groups.get_mut(row) {
                g.pick = i.min(g.targets.len() - 1);
            } else if row == d.quality_row() {
                d.quality = QUALITIES[i.min(2)];
            }
        }
    }
}
