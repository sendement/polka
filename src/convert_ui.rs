//! The "Convert…" dialog: the chosen files sorted into pictures, sound,
//! video, archives and tables. Each kind shows its files, then what they
//! become (archives with the archive dialog's own settings; tables with a
//! CSV separator, previewed, rows to cut at, and gluing); what can't be
//! converted is listed last, with why. Then what becomes of the originals.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::app::{App, Osk, OskPurpose, Overlay, SPLIT_PRESETS, split_label};
use crate::convert::{Group, QUALITIES, Quality, Target};
use crate::i18n::{tr, trn};
use crate::input::Action;
use crate::ops::{ArchiveFormat, CompressOpts, Op};

pub struct GroupRow {
    pub group: Group,
    pub items: Vec<PathBuf>,
    pub targets: Vec<Target>,
    pub pick: usize,
    /// Its own quality (pictures, sound and video).
    pub quality: Quality,
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

/// A row the pad's cursor goes over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CRow {
    /// A kind's formats (by group index).
    Target(usize),
    /// The archives': as in "Compress…".
    Format,
    Method,
    Level,
    Split,
    Password,
    Names,
    /// A kind's quality (by group index).
    Quality(usize),
    /// Tables: the separator CSV is read with (a preview under it), the one
    /// it's written with, rows to cut at, gluing them into one.
    DelimIn,
    /// What CSV's columns become: all at once, then each (by index).
    ColsAll,
    Col(usize),
    DelimOut,
    SplitRows,
    Merge,
    Trash,
    Buttons,
}

pub struct ConvertDlg {
    pub groups: Vec<GroupRow>,
    /// How the archives are packed again.
    pub arch: CompressOpts,
    /// The originals that converted go to the trash.
    pub trash: bool,
    /// Left out, and why (password-protected, a tool missing, not a kind
    /// that converts).
    pub refused: Vec<(PathBuf, &'static str)>,
    pub row: CRow,
    /// On the buttons' row: Convert, Cancel.
    pub button: usize,
    /// How tables convert.
    pub table: crate::table::Opts,
    /// The first CSV's first rows as read now (or why not), and the
    /// separator found for it.
    pub preview: Result<crate::table::Sheet, String>,
    pub found: Option<crate::table::Delim>,
    /// Whether the tables can be glued (else why not).
    pub mergeable: Result<(), String>,
}

const MIB: u64 = 1024 * 1024;

impl ConvertDlg {
    /// The rows there are, in order.
    pub fn rows(&self) -> Vec<CRow> {
        let mut v = Vec::new();
        for (i, g) in self.groups.iter().enumerate() {
            if g.group == Group::Archive {
                let f = self.arch.format;
                v.push(CRow::Format);
                if !f.methods().is_empty() {
                    v.push(CRow::Method);
                }
                v.push(CRow::Level);
                v.push(CRow::Split);
                if f.encrypts() {
                    v.push(CRow::Password);
                }
                if f == ArchiveFormat::SevenZ && self.arch.password.is_some() {
                    v.push(CRow::Names);
                }
            } else if g.group == Group::Table {
                v.push(CRow::Target(i));
                if self.csv_in().is_some() {
                    v.push(CRow::DelimIn);
                    if !self.table.cols.is_empty() {
                        v.push(CRow::ColsAll);
                        v.extend((0..self.table.cols.len()).map(CRow::Col));
                    }
                }
                if g.target() == Target::Table(crate::table::Format::Csv) {
                    v.push(CRow::DelimOut);
                }
                v.push(CRow::SplitRows);
                if g.items.len() > 1 {
                    v.push(CRow::Merge);
                }
            } else {
                v.push(CRow::Target(i));
                v.push(CRow::Quality(i));
            }
        }
        if self.can_trash() {
            v.push(CRow::Trash);
        }
        v.push(CRow::Buttons);
        v
    }

    /// The first CSV among the tables (the preview shows it).
    pub fn csv_in(&self) -> Option<&Path> {
        let g = self.groups.iter().find(|g| g.group == Group::Table)?;
        g.items.iter().find(|p| p.file_name().is_some_and(|n| crate::table::is_csv(&n.to_string_lossy()))).map(|p| p.as_path())
    }

    /// Read again as the separator now says: the preview, whether they glue.
    pub fn refresh_tables(&mut self) {
        let Some(g) = self.groups.iter().find(|g| g.group == Group::Table) else { return };
        let delim = self.table.delim_in;
        let items = g.items.clone();
        if let Some(p) = self.csv_in().map(|p| p.to_path_buf()) {
            self.preview = crate::table::preview(&p, delim, 6);
            self.found = crate::table::delim_for(&p, delim).ok().map(crate::table::Delim::of);
            // A column each (kept while their number stays).
            let n = self.preview.as_ref().map(|s| s.rows.iter().map(|r| r.len()).max().unwrap_or(0)).unwrap_or(0);
            if self.table.cols.len() != n {
                self.table.cols = vec![crate::table::ColType::Auto; n];
            }
        }
        self.mergeable = if items.len() > 1 { crate::table::mergeable(&items, delim) } else { Err(String::new()) };
        if self.mergeable.is_err() {
            self.table.merge = false;
        }
    }

    /// Anything whose original could go to the trash (Steam's recordings
    /// never do).
    pub fn can_trash(&self) -> bool {
        self.groups.iter().flat_map(|g| &g.items).any(|p| !crate::steam::is_recording(p))
    }

    fn move_row(&mut self, d: isize) {
        let rows = self.rows();
        let at = rows.iter().position(|r| *r == self.row).unwrap_or(0) as isize;
        self.row = rows[(at + d).clamp(0, rows.len() as isize - 1) as usize];
    }

    /// The archives' format, kept with their group's target; a new format
    /// starts on its own method (LZMA2 for 7z, not zip's Deflate).
    fn sync_archive(&mut self) {
        if let Some(m) = self.arch.format.methods().first() {
            self.arch.method = *m;
        }
        self.arch.fit();
        let f = self.arch.format;
        if let Some(g) = self.groups.iter_mut().find(|g| g.group == Group::Archive) {
            g.pick = g.targets.iter().position(|t| *t == Target::Archive(f)).unwrap_or(0);
        }
    }

    fn step(&mut self, row: CRow, d: isize) {
        fn cycle<T: PartialEq + Copy>(all: &[T], cur: T, d: isize) -> T {
            let i = all.iter().position(|x| *x == cur).unwrap_or(0) as isize;
            all[(i + d).rem_euclid(all.len() as isize) as usize]
        }
        let o = &mut self.arch;
        match row {
            CRow::Target(i) => {
                let g = &mut self.groups[i];
                g.pick = (g.pick as isize + d).rem_euclid(g.targets.len() as isize) as usize;
            }
            CRow::Format => {
                o.format = cycle(&crate::app::CompressDlg::formats(), o.format, d);
                self.sync_archive();
            }
            CRow::Method if !o.format.methods().is_empty() => o.method = cycle(o.format.methods(), o.method, d),
            CRow::Level => o.level = cycle(o.format.levels(), o.level, d),
            CRow::Split => {
                let mut all: Vec<Option<u64>> = vec![None];
                all.extend(SPLIT_PRESETS.iter().map(|m| Some(m * MIB)));
                o.split = cycle(&all, o.split, d);
            }
            CRow::Names => o.encrypt_names = !o.encrypt_names,
            CRow::Quality(i) => {
                let g = &mut self.groups[i];
                let at = QUALITIES.iter().position(|q| *q == g.quality).unwrap_or(1) as isize;
                g.quality = QUALITIES[(at + d).clamp(0, 2) as usize];
            }
            CRow::DelimIn => {
                self.table.delim_in = cycle(&crate::table::DELIMS, self.table.delim_in, d);
                self.refresh_tables();
            }
            CRow::ColsAll => {
                let cols = &mut self.table.cols;
                let now = if cols.iter().all(|c| *c == cols[0]) { cols[0] } else { crate::table::ColType::Auto };
                let t = cycle(&crate::table::COL_TYPES, now, d);
                cols.iter_mut().for_each(|c| *c = t);
            }
            CRow::Col(k) => {
                if let Some(c) = self.table.cols.get_mut(k) {
                    *c = cycle(&crate::table::COL_TYPES, *c, d);
                }
            }
            CRow::DelimOut => self.table.delim_out = cycle(&crate::table::DELIMS[1..], self.table.delim_out, d),
            CRow::SplitRows => {
                let mut all: Vec<Option<usize>> = vec![None];
                all.extend(crate::table::SPLITS.iter().map(|n| Some(*n)));
                self.table.split = cycle(&all, self.table.split, d);
            }
            CRow::Merge => self.table.merge = !self.table.merge && self.mergeable.is_ok(),
            CRow::Trash => self.trash = !self.trash,
            CRow::Buttons => self.button = (self.button as isize + d).clamp(0, 1) as usize,
            _ => {}
        }
    }
}

fn group_name(g: Group) -> &'static str {
    match g {
        Group::Image => tr("Image"),
        Group::Audio => tr("Audio"),
        Group::Video => tr("Video"),
        Group::Archive => tr("Archive"),
        Group::Table => tr("Tables"),
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
        let mut sets = HashSet::new();
        for p in paths {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(g) = crate::convert::group_of(&name).filter(|_| !p.is_dir()) else {
                refused.push((p, if crate::convert::group_of(&name).is_some() { tr("Folder") } else { tr("doesn't convert") }));
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
                refused.push((p, why));
                continue;
            }
            match groups.iter_mut().find(|r| r.group == g) {
                Some(r) => r.items.push(p),
                None => {
                    let targets = crate::convert::targets(g);
                    if targets.is_empty() {
                        refused.push((p, tr("needs ffmpeg")));
                        continue;
                    }
                    // The last choice for this kind, else a usual one: HEIC
                    // from a phone mostly becomes JPEG.
                    let usual = match g {
                        Group::Image if name.to_lowercase().ends_with(".heic") => Target::Jpg,
                        Group::Image => Target::Png,
                        Group::Audio => Target::Mp3,
                        Group::Video => Target::Mp4,
                        Group::Archive => Target::Archive(ArchiveFormat::Zip),
                        // A spreadsheet mostly wants to be a CSV, a CSV a spreadsheet.
                        Group::Table if crate::table::is_csv(&name) => Target::Table(crate::table::Format::Xlsx),
                        Group::Table => Target::Table(crate::table::Format::Csv),
                    };
                    let want = self.convert_last.iter().find(|(k, _)| *k == g).map(|(_, t)| *t).unwrap_or(usual);
                    let pick = targets.iter().position(|t| *t == want).unwrap_or(0);
                    groups.push(GroupRow { group: g, items: vec![p], targets, pick, quality: Quality::Balanced });
                }
            }
        }
        groups.sort_by_key(|r| crate::convert::GROUPS.iter().position(|g| *g == r.group));
        if groups.is_empty() {
            let why = refused.first().map(|(p, w)| format!("{}: {w}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())).unwrap_or_else(|| tr("Nothing here can be converted").to_string());
            return self.toast(why, true);
        }
        let format = groups.iter().find(|g| g.group == Group::Archive).and_then(|g| match g.target() {
            Target::Archive(f) => Some(f),
            _ => None,
        });
        let arch = CompressOpts::new(format.unwrap_or(ArchiveFormat::Zip));
        let mut d = ConvertDlg {
            groups,
            arch,
            trash: false,
            refused,
            row: CRow::Buttons,
            button: 0,
            table: Default::default(),
            preview: Ok(Default::default()),
            found: None,
            mergeable: Err(String::new()),
        };
        d.row = d.rows()[0];
        d.sync_archive();
        d.refresh_tables();
        self.overlay = Some(Overlay::Convert(Box::new(d)));
    }

    pub fn convert_input(&mut self, a: Action) {
        let Some(Overlay::Convert(d)) = self.overlay.as_mut() else { return };
        match a {
            Action::Up | Action::Down => {
                d.move_row(if a == Action::Up { -1 } else { 1 });
                let bottom = matches!(d.row, CRow::Trash | CRow::Buttons);
                self.ctx.data_mut(|m| {
                    m.insert_temp(egui::Id::new("settings-reveal"), true);
                    // At the end: down to what's left out, under it.
                    m.insert_temp(egui::Id::new("convert-bottom"), bottom);
                });
            }
            Action::Left | Action::Right => {
                let row = d.row;
                d.step(row, if a == Action::Left { -1 } else { 1 });
            }
            Action::Accept => {
                let row = d.row;
                self.convert_activate(row);
            }
            Action::Menu => self.convert_start(),
            Action::Back => self.overlay = None,
            _ => {}
        }
    }

    /// ✕ (or a click) on a row.
    pub fn convert_activate(&mut self, row: CRow) {
        let Some(Overlay::Convert(d)) = self.overlay.as_mut() else { return };
        d.row = row;
        match row {
            CRow::Buttons => {
                if d.button == 0 {
                    self.convert_start();
                } else {
                    self.overlay = None;
                }
            }
            CRow::Password => {
                let Some(Overlay::Convert(d)) = self.overlay.take() else { return };
                let now = d.arch.password.clone().unwrap_or_default();
                let mut k = Osk::new(tr("Archive password"), &now, OskPurpose::ConvertPassword { dlg: d });
                k.secret = true;
                self.overlay = Some(Overlay::Osk(k));
            }
            _ => d.step(row, 1),
        }
    }

    /// The archives' password typed (empty: none).
    pub fn convert_password(&mut self, mut d: Box<ConvertDlg>, pw: String) {
        d.arch.password = (!pw.is_empty()).then_some(pw);
        if d.arch.password.is_none() {
            d.arch.encrypt_names = false;
        }
        self.overlay = Some(Overlay::Convert(d));
    }

    /// Off it goes, as a job.
    pub fn convert_start(&mut self) {
        let Some(Overlay::Convert(d)) = self.overlay.take() else { return };
        let mut items = Vec::new();
        for g in &d.groups {
            // Archives: as their settings say.
            let t = if g.group == Group::Archive { Target::Archive(d.arch.format) } else { g.target() };
            self.convert_last.retain(|(k, _)| *k != g.group);
            self.convert_last.push((g.group, t));
            let reshaped = (g.group == Group::Archive && (d.arch.split.is_some() || d.arch.password.is_some())) || (g.group == Group::Table && (d.table.split.is_some() || d.table.merge));
            items.extend(g.items.iter().filter(|p| !crate::convert::needless(p, t) || reshaped).map(|p| crate::convert::Item { src: p.clone(), target: t, quality: g.quality }));
        }
        if items.is_empty() {
            return self.toast(tr("Already in that format"), false);
        }
        self.pane_mut().selected.clear();
        self.start(Op::Convert { items, trash: d.trash, archive: d.arch.clone(), table: d.table.clone() });
    }
}

// ---------------------------------------------------------------- drawing

/// Up to this many lines of files a kind shows (the rest: "+n").
const TILE_LINES: usize = 2;
const TILE: Vec2 = Vec2::new(92.0, 74.0);
/// Left out: room for why, on two lines.
const TILE_WHY: f32 = 96.0;

/// The files as tiles: their thumbnail or icon, the name, maybe why not.
fn tiles(ui: &mut Ui, items: &[(&Path, Option<&str>)], icons: &mut crate::icons::Icons, dim: bool) {
    let v = ui.visuals().clone();
    let per_line = ((crate::view::SETTINGS_W - 8.0) / TILE.x).floor().max(1.0) as usize;
    let max = per_line * TILE_LINES;
    let shown = if items.len() > max { max - 1 } else { items.len() };
    let lines = (shown + usize::from(items.len() > shown)).div_ceil(per_line).max(1);
    let tile = vec2(TILE.x, if items.iter().any(|i| i.1.is_some()) { TILE_WHY } else { TILE.y });
    let (room, _) = ui.allocate_exact_size(vec2(crate::view::SETTINGS_W, lines as f32 * tile.y), Sense::hover());
    let tint = if dim { Color32::from_white_alpha(110) } else { Color32::WHITE };
    for (i, (path, why)) in items.iter().take(shown).enumerate() {
        let at = room.min + vec2((i % per_line) as f32 * tile.x, (i / per_line) as f32 * tile.y);
        let cell = Rect::from_min_size(at, tile).shrink(3.0);
        // A multi-volume set by its whole name.
        let name = match crate::volumes::set_of(path) {
            Some(set) => format!("{} ({})", set.archive, crate::i18n::volumes(set.files.len())),
            // A Steam recording by its game and time.
            None => crate::steam::recording_title(path).unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        };
        let meta = std::fs::metadata(path).ok();
        let mtime = meta.as_ref().map(|m| std::os::unix::fs::MetadataExt::mtime(m));
        let ir = Rect::from_center_size(pos2(cell.center().x, cell.min.y + 20.0), Vec2::splat(36.0));
        let thumb = mtime.and_then(|t| icons.thumb(path, t));
        let uri = thumb.map(|t| t.0).or_else(|| {
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let e = crate::vfs::Entry { name: file, is_dir: false, is_link: false, size: meta.as_ref().map_or(0, |m| m.len()), mtime, mode: 0o644, tags: Vec::new() };
            icons.for_entry(&e, Some(path), 48)
        });
        if let Some(u) = uri {
            egui::Image::new(u).tint(tint).maintain_aspect_ratio(true).paint_at(ui, ir);
        }
        let color = if dim { v.weak_text_color() } else { v.text_color() };
        let g = crate::view::elided_centered(ui, &name, FontId::proportional(12.0), color, cell.width(), 1);
        ui.painter().galley(pos2(cell.center().x, ir.max.y + 4.0), g, color);
        if let Some(w) = why {
            let g = crate::view::elided_centered(ui, w, FontId::proportional(11.0), v.warn_fg_color, cell.width(), 2);
            ui.painter().galley(pos2(cell.center().x, ir.max.y + 19.0), g, v.warn_fg_color);
        }
        let r = ui.interact(cell, ui.id().with(("conv-tile", path)), Sense::hover());
        r.on_hover_text(match why {
            Some(w) => format!("{name}\n{w}"),
            None => name,
        });
    }
    if items.len() > shown {
        let i = shown;
        let at = room.min + vec2((i % per_line) as f32 * tile.x, (i / per_line) as f32 * tile.y);
        let cell = Rect::from_min_size(at, tile).shrink(3.0);
        ui.painter().rect_filled(cell, 8.0, v.widgets.inactive.weak_bg_fill);
        ui.painter().text(cell.center(), Align2::CENTER_CENTER, format!("+{}", items.len() - shown), FontId::proportional(16.0), v.text_color());
    }
}

/// The dialog. A click: the row, and what was hit (0 the row itself,
/// 10+ a button, 100+ a chip, ±1 an arrow).
pub fn draw(ui: &mut Ui, d: &ConvertDlg, icons: &mut crate::icons::Icons) -> Option<(CRow, isize)> {
    let mut ev = None;
    let v = ui.visuals().clone();
    let font = FontId::proportional(15.0);
    ui.label(egui::RichText::new(tr("Convert")).strong().size(18.0));
    ui.add_space(6.0);
    const CHIPS_X: f32 = 150.0;
    // Chips from the left of a column on the right, wrapping as needed.
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
    let chips = |ui: &mut Ui, rect: Rect, row: CRow, names: Vec<(String, bool)>, ev: &mut Option<(CRow, isize)>| {
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
            let cr = ui.interact(chip, ui.id().with(("conv-chip", format!("{row:?}"), i)), Sense::click());
            let fill = if *on { v.selection.bg_fill } else if cr.hovered() { v.widgets.hovered.bg_fill } else { v.widgets.inactive.bg_fill };
            ui.painter().rect(chip, 6.0, fill, Stroke::NONE, StrokeKind::Inside);
            ui.painter().text(chip.center(), Align2::CENTER_CENTER, name, FontId::proportional(13.0), if *on { v.selection.stroke.color } else { v.text_color() });
            if cr.clicked() {
                *ev = Some((row, 100 + i as isize));
            }
        }
    };
    // A setting with ‹ value ›.
    let stepper = |ui: &mut Ui, row: CRow, label: &str, value: String, ev: &mut Option<(CRow, isize)>| -> Rect {
        let (rect, r) = crate::view::settings_row(ui, 40.0, d.row == row, ("conv", format!("{row:?}")));
        if r.clicked() {
            *ev = Some((row, 0));
        }
        ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(14.0), v.text_color());
        let right = Rect::from_center_size(pos2(rect.max.x - 24.0, rect.center().y), Vec2::splat(26.0));
        let left = Rect::from_center_size(pos2(rect.max.x - 214.0, rect.center().y), Vec2::splat(26.0));
        for (b, t, step) in [(left, "‹", -1), (right, "›", 1)] {
            let br = ui.interact(b, ui.id().with(("conv-step", format!("{row:?}"), step)), Sense::click());
            ui.painter().rect(b, 6.0, if br.hovered() { v.widgets.hovered.bg_fill } else { v.widgets.inactive.bg_fill }, Stroke::NONE, StrokeKind::Inside);
            ui.painter().text(b.center(), Align2::CENTER_CENTER, t, FontId::proportional(16.0), v.text_color());
            if br.clicked() {
                *ev = Some((row, step));
            }
        }
        ui.painter().text(pos2((left.max.x + right.min.x) / 2.0, rect.center().y), Align2::CENTER_CENTER, value, FontId::proportional(14.0), v.strong_text_color());
        rect
    };
    let max_h = (ui.ctx().content_rect().height() - 200.0).max(240.0);
    egui::ScrollArea::vertical().id_salt("convert").max_height(max_h).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        for (i, g) in d.groups.iter().enumerate() {
            // The kind's files first…
            ui.label(egui::RichText::new(format!("{} · {}", group_name(g.group), g.items.len())).strong());
            let items: Vec<(&Path, Option<&str>)> = g.items.iter().map(|p| (p.as_path(), None)).collect();
            tiles(ui, &items, icons, false);
            // …then what they become.
            if g.group == Group::Archive {
                let o = &d.arch;
                let names: Vec<String> = g.targets.iter().map(|t| t.label().to_string()).collect();
                let h = 24.0 + 30.0 * lines(ui, &names, crate::view::SETTINGS_W - CHIPS_X - 10.0) as f32;
                let (rect, r) = crate::view::settings_row(ui, h.max(44.0), d.row == CRow::Format, ("conv", "format"));
                if r.clicked() {
                    ev = Some((CRow::Format, 0));
                }
                ui.painter().text(pos2(rect.min.x + 14.0, rect.min.y + 22.0), Align2::LEFT_CENTER, tr("Format"), font.clone(), v.text_color());
                chips(ui, rect, CRow::Format, g.targets.iter().enumerate().map(|(k, t)| (t.label().to_string(), k == g.pick)).collect(), &mut ev);
                if !o.format.methods().is_empty() {
                    stepper(ui, CRow::Method, tr("Method"), o.method.label().to_string(), &mut ev);
                }
                stepper(ui, CRow::Level, tr("Compression"), o.level.label().to_string(), &mut ev);
                stepper(ui, CRow::Split, tr("Split into volumes"), split_label(o.split), &mut ev);
                if o.format.encrypts() {
                    let dots = o.password.as_ref().map_or_else(|| tr("None").to_string(), |p| "•".repeat(p.chars().count().min(12)));
                    let (rect, r) = crate::view::settings_row(ui, 40.0, d.row == CRow::Password, ("conv", "password"));
                    if r.clicked() {
                        ev = Some((CRow::Password, 0));
                    }
                    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Password"), FontId::proportional(14.0), v.text_color());
                    ui.painter().text(pos2(rect.max.x - 14.0, rect.center().y), Align2::RIGHT_CENTER, dots, FontId::proportional(14.0), v.strong_text_color());
                }
                if o.format == ArchiveFormat::SevenZ && o.password.is_some() {
                    let (rect, r) = crate::view::settings_row(ui, 40.0, d.row == CRow::Names, ("conv", "names"));
                    if r.clicked() {
                        ev = Some((CRow::Names, 0));
                    }
                    ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Encrypt file names"), FontId::proportional(14.0), v.text_color());
                    let b = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(18.0));
                    ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
                    if o.encrypt_names {
                        ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(14.0), v.strong_text_color());
                    }
                }
            } else {
                let names: Vec<String> = g.targets.iter().map(|t| t.label().to_string()).collect();
                let h = 24.0 + 30.0 * lines(ui, &names, crate::view::SETTINGS_W - CHIPS_X - 10.0) as f32;
                let row = CRow::Target(i);
                let (rect, r) = crate::view::settings_row(ui, h.max(44.0), d.row == row, ("conv", i));
                if r.clicked() {
                    ev = Some((row, 0));
                }
                ui.painter().text(pos2(rect.min.x + 14.0, rect.min.y + 22.0), Align2::LEFT_CENTER, tr("Convert to"), font.clone(), v.text_color());
                let already = g.already();
                if already > 0 {
                    ui.painter().text(pos2(rect.min.x + 14.0, rect.min.y + 42.0), Align2::LEFT_CENTER, trn("Already in this format: %n", already), FontId::proportional(12.0), v.weak_text_color());
                }
                chips(ui, rect, row, g.targets.iter().enumerate().map(|(k, t)| (t.label().to_string(), k == g.pick)).collect(), &mut ev);
                if g.group == Group::Table {
                    table_rows(ui, d, g, &stepper, &mut ev);
                    ui.add_space(8.0);
                    continue;
                }
                // Its own quality (faded for a lossless format, where it
                // means nothing).
                let qrow = CRow::Quality(i);
                let (rect, r) = crate::view::settings_row(ui, 40.0, d.row == qrow, ("conv-q", i));
                if r.clicked() {
                    ev = Some((qrow, 0));
                }
                let c = if g.target().lossy() { v.text_color() } else { v.weak_text_color() };
                ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Quality"), FontId::proportional(14.0), c);
                chips(ui, Rect::from_min_max(pos2(rect.min.x, rect.center().y - 22.0), rect.max), qrow, QUALITIES.iter().map(|q| (quality_name(*q).to_string(), *q == g.quality)).collect(), &mut ev);
            }
            ui.add_space(8.0);
        }
        if d.can_trash() {
            let (rect, r) = crate::view::settings_row(ui, 44.0, d.row == CRow::Trash, ("conv", "trash"));
            if r.clicked() {
                ev = Some((CRow::Trash, 0));
            }
            ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, tr("Move the originals to the trash"), font.clone(), v.text_color());
            let b = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(20.0));
            ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
            if d.trash {
                ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), v.strong_text_color());
            }
        }
        ui.add_space(8.0);
        // What can't be converted, and why.
        if !d.refused.is_empty() {
            ui.label(egui::RichText::new(format!("{} · {}", tr("Left out"), d.refused.len())).strong().color(v.warn_fg_color));
            // A line each: the icon, the name, why.
            for (p, why) in &d.refused {
                let (rect, _) = ui.allocate_exact_size(vec2(crate::view::SETTINGS_W, 26.0), Sense::hover());
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let e = crate::vfs::Entry { name: name.clone(), is_dir: p.is_dir(), is_link: false, size: 0, mtime: None, mode: 0o644, tags: Vec::new() };
                if let Some(u) = icons.for_entry(&e, Some(p), 32) {
                    egui::Image::new(u).tint(Color32::from_white_alpha(110)).paint_at(ui, Rect::from_center_size(pos2(rect.min.x + 18.0, rect.center().y), Vec2::splat(20.0)));
                }
                let g = crate::view::elided(ui, &name, FontId::proportional(13.0), v.weak_text_color(), rect.width() * 0.55, 1);
                let w = g.size().x;
                ui.painter().galley(pos2(rect.min.x + 36.0, rect.center().y - g.size().y / 2.0), g, v.weak_text_color());
                ui.painter().text(pos2(rect.min.x + 44.0 + w, rect.center().y), Align2::LEFT_CENTER, format!("— {why}"), FontId::proportional(13.0), v.warn_fg_color);
            }
            ui.add_space(8.0);
        }
        if ui.ctx().data_mut(|m| m.remove_temp::<bool>(egui::Id::new("convert-bottom"))).unwrap_or(false) {
            ui.scroll_to_cursor(Some(egui::Align::Max));
        }
    });
    // The buttons (the pad: ← → and ✕ on their row, or Start).
    let (rect, _) = crate::view::settings_row(ui, 44.0, d.row == CRow::Buttons, ("conv", "buttons"));
    for (i, n) in [tr("Convert"), tr("Cancel")].iter().enumerate() {
        let w = 150.0;
        let b = Rect::from_min_size(pos2(rect.max.x - (2 - i) as f32 * (w + 8.0), rect.center().y - 15.0), vec2(w, 30.0));
        let br = ui.interact(b, ui.id().with(("conv-btn", i)), Sense::click());
        let on = (d.row == CRow::Buttons && d.button == i) || (i == 0 && d.row != CRow::Buttons);
        let fill = if on { v.selection.bg_fill } else if br.hovered() { v.widgets.hovered.bg_fill } else { v.widgets.inactive.bg_fill };
        ui.painter().rect(b, 6.0, fill, Stroke::NONE, StrokeKind::Inside);
        ui.painter().text(b.center(), Align2::CENTER_CENTER, *n, font.clone(), if on { v.selection.stroke.color } else { v.text_color() });
        if br.clicked() {
            ev = Some((CRow::Buttons, 10 + i as isize));
        }
    }
    ev
}

/// Tables' settings: CSV's separator in (with what it reads as, and a
/// preview) and its columns' types, the separator out, rows to cut at,
/// gluing.
fn table_rows(ui: &mut Ui, d: &ConvertDlg, g: &GroupRow, stepper: &dyn Fn(&mut Ui, CRow, &str, String, &mut Option<(CRow, isize)>) -> Rect, ev: &mut Option<(CRow, isize)>) {
    use crate::table::{Cell, ColType, Delim};
    let v = ui.visuals().clone();
    if d.csv_in().is_some() {
        let value = match (d.table.delim_in, d.found) {
            (Delim::Auto, Some(f)) => format!("{} ({})", Delim::Auto.label(), f.label().trim()),
            (x, _) => x.label().trim().to_string(),
        };
        stepper(ui, CRow::DelimIn, tr("CSV separator"), value, ev);
        // How the first CSV reads with it: a few rows, a few columns (the
        // column being set among them), as their types make them.
        const ROW_H: f32 = 20.0;
        let (rect, _) = ui.allocate_exact_size(vec2(crate::view::SETTINGS_W, 18.0 + 6.0 * ROW_H + 4.0), Sense::hover());
        let caption = Rect::from_min_max(pos2(rect.min.x + 18.0, rect.min.y), pos2(rect.max.x - 18.0, rect.min.y + 16.0));
        let area = Rect::from_min_max(pos2(rect.min.x + 14.0, rect.min.y + 18.0), pos2(rect.max.x - 14.0, rect.max.y - 2.0));
        ui.painter().rect_filled(area, 6.0, v.extreme_bg_color);
        let focus = match d.row {
            CRow::Col(k) => Some(k),
            _ => None,
        };
        match &d.preview {
            Ok(sheet) => {
                let n = sheet.rows.iter().map(|r| r.len()).max().unwrap_or(0);
                let shown = n.clamp(1, 6);
                let first = focus.map_or(0, |k| k.saturating_sub(shown - 1)).min(n.saturating_sub(shown));
                let header = sheet.header().is_some();
                let w = area.width() / shown as f32;
                let font = FontId::monospace(12.0);
                // Which file it is, and which columns show.
                let mut room = caption.width();
                if n > shown {
                    let span = format!("{}–{} / {}", first + 1, first + shown, n);
                    let gal = ui.painter().layout_no_wrap(span, FontId::proportional(11.0), v.weak_text_color());
                    room -= gal.size().x + 12.0;
                    ui.painter().galley(pos2(caption.max.x - gal.size().x, caption.center().y - gal.size().y / 2.0), gal, v.weak_text_color());
                }
                if let Some(name) = d.csv_in().and_then(|p| p.file_name()) {
                    let gal = crate::view::elided(ui, &name.to_string_lossy(), FontId::proportional(11.0), v.weak_text_color(), room, 1);
                    ui.painter().galley(pos2(caption.min.x, caption.center().y - gal.size().y / 2.0), gal, v.weak_text_color());
                }
                if let Some(k) = focus.filter(|k| (first..first + shown).contains(k)) {
                    let col = Rect::from_min_size(pos2(area.min.x + (k - first) as f32 * w, area.min.y), vec2(w, area.height()));
                    ui.painter().rect_filled(col.shrink(1.0), 4.0, v.selection.bg_fill.gamma_multiply(0.3));
                }
                for (r, row) in sheet.rows.iter().take(6).enumerate() {
                    let line = Rect::from_min_size(pos2(area.min.x, area.min.y + 2.0 + r as f32 * ROW_H), vec2(area.width(), ROW_H));
                    if r % 2 == 1 {
                        ui.painter().rect_filled(line.shrink2(vec2(2.0, 0.0)), 3.0, crate::view::stripe(&v));
                    }
                    for c in first..first + shown {
                        let cell = Rect::from_min_size(pos2(area.min.x + (c - first) as f32 * w, line.min.y), vec2(w, ROW_H));
                        let raw = row.get(c).cloned().unwrap_or(Cell::Empty);
                        let t = d.table.cols.get(c).copied().unwrap_or(ColType::Auto);
                        let (shown_cell, ok) = if r == 0 && header { (raw, true) } else { (t.shown(&raw), t.fits(&raw)) };
                        let color = if r == 0 && header {
                            v.strong_text_color()
                        } else if !ok {
                            v.warn_fg_color
                        } else {
                            v.text_color()
                        };
                        let gal = crate::view::elided(ui, &shown_cell.text(), font.clone(), color, w - 10.0, 1);
                        // Numbers to the right, as spreadsheets have them.
                        let x = if matches!(shown_cell, Cell::Num(_)) { cell.max.x - 5.0 - gal.size().x } else { cell.min.x + 5.0 };
                        ui.painter().galley(pos2(x, cell.center().y - gal.size().y / 2.0), gal, color);
                        if c > first {
                            ui.painter().line_segment([cell.left_top(), cell.left_bottom()], Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color));
                        }
                    }
                }
                // The columns' types: all at once, then each, with its first
                // values as they'd become.
                let cols = &d.table.cols;
                if !cols.is_empty() {
                    let all = if cols.iter().all(|c| *c == cols[0]) { cols[0].label().to_string() } else { tr("Mixed").to_string() };
                    stepper(ui, CRow::ColsAll, tr("All columns"), all, ev);
                    let names = sheet.header();
                    let label_w = crate::view::SETTINGS_W - 250.0;
                    for (k, t) in cols.iter().enumerate() {
                        let rect = stepper(ui, CRow::Col(k), "", t.label().to_string(), ev);
                        let name = match names.as_ref().and_then(|h| h.get(k)) {
                            Some(h) => format!("{} · {h}", k + 1),
                            None => trn("Column %n", k + 1),
                        };
                        let gal = crate::view::elided(ui, &name, FontId::proportional(14.0), v.text_color(), label_w, 1);
                        ui.painter().galley(pos2(rect.min.x + 14.0, rect.center().y - 7.0 - gal.size().y / 2.0), gal, v.text_color());
                        let data = sheet.rows.iter().skip(usize::from(names.is_some()));
                        let cells: Vec<Cell> = data.filter_map(|r| r.get(k)).filter(|c| **c != Cell::Empty).take(4).cloned().collect();
                        let bad = cells.iter().any(|c| !t.fits(c));
                        let sample = cells.iter().map(|c| t.shown(c).text()).collect::<Vec<_>>().join(", ");
                        let color = if bad { v.warn_fg_color } else { v.weak_text_color() };
                        let gal = crate::view::elided(ui, &sample, FontId::proportional(11.5), color, label_w, 1);
                        ui.painter().galley(pos2(rect.min.x + 14.0, rect.center().y + 8.0 - gal.size().y / 2.0), gal, color);
                    }
                }
            }
            Err(e) => {
                ui.painter().text(area.center(), Align2::CENTER_CENTER, e, FontId::proportional(13.0), v.warn_fg_color);
            }
        }
    }
    if g.target() == Target::Table(crate::table::Format::Csv) {
        stepper(ui, CRow::DelimOut, tr("Write CSV with"), d.table.delim_out.label().trim().to_string(), ev);
    }
    let split = match d.table.split {
        None => tr("No").to_string(),
        Some(n) => trn("Every %n rows", n),
    };
    stepper(ui, CRow::SplitRows, tr("Cut into parts"), split, ev);
    if g.items.len() > 1 {
        let (rect, r) = crate::view::settings_row(ui, 44.0, d.row == CRow::Merge, ("conv", "merge"));
        if r.clicked() {
            *ev = Some((CRow::Merge, 0));
        }
        let ok = d.mergeable.is_ok();
        let c = if ok { v.text_color() } else { v.weak_text_color() };
        ui.painter().text(pos2(rect.min.x + 14.0, rect.min.y + 15.0), Align2::LEFT_CENTER, tr("Glue into one table"), FontId::proportional(14.0), c);
        let note = match &d.mergeable {
            Ok(()) => tr("same columns, case aside").to_string(),
            Err(e) => e.clone(),
        };
        let gal = crate::view::elided(ui, &note, FontId::proportional(11.5), if ok { v.weak_text_color() } else { v.warn_fg_color }, rect.width() - 80.0, 1);
        ui.painter().galley(pos2(rect.min.x + 14.0, rect.min.y + 26.0), gal, v.weak_text_color());
        let b = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(20.0));
        ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, c), StrokeKind::Inside);
        if d.table.merge {
            ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), v.strong_text_color());
        }
    }
}

/// A click in the dialog.
pub fn clicked(app: &mut App, row: CRow, step: isize) {
    let Some(Overlay::Convert(d)) = app.overlay.as_mut() else { return };
    d.row = row;
    match step {
        0 => match row {
            CRow::Trash | CRow::Names | CRow::Password | CRow::Merge => app.convert_activate(row),
            _ => {}
        },
        -1 | 1 => d.step(row, step),
        10 => app.convert_start(),
        11 => app.overlay = None,
        _ => {
            let i = (step - 100) as usize;
            match row {
                CRow::Target(g) => {
                    let g = &mut d.groups[g];
                    g.pick = i.min(g.targets.len() - 1);
                }
                CRow::Format => {
                    let formats: Vec<ArchiveFormat> = d.groups.iter().find(|g| g.group == Group::Archive).map(|g| g.targets.iter().filter_map(|t| if let Target::Archive(f) = t { Some(*f) } else { None }).collect()).unwrap_or_default();
                    if let Some(f) = formats.get(i) {
                        d.arch.format = *f;
                        d.sync_archive();
                    }
                }
                CRow::Quality(g) => d.groups[g].quality = QUALITIES[i.min(2)],
                _ => {}
            }
        }
    }
}
