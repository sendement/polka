//! The Properties window: a file's or folder's facts, and for folders
//! what's inside (counted on another thread: the size, the space it takes
//! on the disk, its files and folders). Several chosen: their sum on top,
//! then each one's own.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use egui::{Ui, Vec2};

use crate::app::human_size;
use crate::i18n::{tr, trn};

/// What's counted so far (it grows while the thread walks).
#[derive(Default)]
pub struct Measure {
    pub size: AtomicU64,
    pub disk: AtomicU64,
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub done: AtomicBool,
    /// The space on the disk is known (not inside an archive).
    pub on_disk: AtomicBool,
}

impl Measure {
    fn add(&self, size: u64, disk: u64, dir: bool) {
        self.size.fetch_add(size, Relaxed);
        self.disk.fetch_add(disk, Relaxed);
        if dir {
            self.dirs.fetch_add(1, Relaxed);
        } else {
            self.files.fetch_add(1, Relaxed);
        }
    }

    fn done(&self) -> bool {
        self.done.load(Relaxed)
    }
}

/// One of several chosen.
pub struct Item {
    pub name: String,
    pub icon: Option<String>,
    pub kind: String,
    pub is_dir: bool,
    pub mtime: Option<i64>,
    pub m: Arc<Measure>,
}

pub struct Props {
    /// Facts that don't change (name, type, place, dates, rights).
    pub rows: Vec<(String, String)>,
    pub icon: Option<String>,
    /// One item that's a folder (its contents counted), or a file.
    pub is_dir: bool,
    pub total: Arc<Measure>,
    /// Several chosen: each one.
    pub items: Vec<Item>,
}

/// What to measure: a path on the disk, or (inside an archive) the size
/// as listed.
pub struct Target {
    pub path: Option<PathBuf>,
    /// Where it's listed (a folder inside an archive is counted by that).
    pub loc: Option<crate::vfs::Location>,
    pub size: u64,
    pub is_dir: bool,
}

/// Counts each target into its own measure and all into `total`, on a
/// thread; stops when the window's gone.
pub fn measure(targets: Vec<(Target, Arc<Measure>)>, total: Arc<Measure>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let mut seen = HashSet::new();
        let mut last = std::time::Instant::now();
        for (t, m) in &targets {
            match &t.path {
                Some(p) => {
                    m.on_disk.store(true, Relaxed);
                    total.on_disk.store(true, Relaxed);
                    walk(p, m, &total, &mut seen, &mut last, &ctx, true);
                }
                None if t.is_dir && t.loc.is_some() => listed(t.loc.as_ref().unwrap(), m, &total, 0),
                None => {
                    m.size.store(t.size, Relaxed);
                    total.size.fetch_add(t.size, Relaxed);
                    let c = if t.is_dir { &total.dirs } else { &total.files };
                    c.fetch_add(1, Relaxed);
                }
            }
            m.done.store(true, Relaxed);
            // (The window's closed: nobody's looking.)
            if Arc::strong_count(&total) == 1 {
                return;
            }
        }
        total.done.store(true, Relaxed);
        ctx.request_repaint();
    });
}

/// A file counts once however many hard links it has; links aren't
/// followed. The top itself isn't counted among a folder's contents.
fn walk(p: &Path, m: &Measure, total: &Arc<Measure>, seen: &mut HashSet<(u64, u64)>, last: &mut std::time::Instant, ctx: &egui::Context, top: bool) {
    let Ok(md) = std::fs::symlink_metadata(p) else { return };
    let dir = md.is_dir();
    if !(md.nlink() > 1 && !dir && !seen.insert((md.dev(), md.ino()))) {
        let (size, disk) = (if dir { 0 } else { md.len() }, md.blocks() * 512);
        if top && dir {
            // Its own blocks count, not as one of its folders.
            m.disk.fetch_add(disk, Relaxed);
            total.disk.fetch_add(disk, Relaxed);
        } else {
            m.add(size, disk, dir);
            total.add(size, disk, dir);
        }
    }
    if last.elapsed().as_millis() > 100 {
        *last = std::time::Instant::now();
        ctx.request_repaint();
    }
    if dir {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for e in rd.flatten() {
            // (The window's closed: nobody's looking.)
            if Arc::strong_count(total) == 1 {
                return;
            }
            walk(&e.path(), m, total, seen, last, ctx, false);
        }
    }
}

/// A folder inside an archive, by its listing (sizes as listed).
fn listed(loc: &crate::vfs::Location, m: &Measure, total: &Arc<Measure>, depth: usize) {
    let crate::vfs::Location::Archive { archive, inner } = loc else { return };
    let Ok(es) = crate::vfs::list(loc) else { return };
    for e in es {
        if Arc::strong_count(total) == 1 || depth > 64 {
            return;
        }
        m.add(e.size, 0, e.is_dir);
        total.add(e.size, 0, e.is_dir);
        if e.is_dir {
            let inner = if inner.is_empty() { e.name.clone() } else { format!("{inner}/{}", e.name) };
            listed(&crate::vfs::Location::Archive { archive: archive.clone(), inner }, m, total, depth + 1);
        }
    }
}

/// "12 files · 3 folders".
fn contents(m: &Measure) -> String {
    let (f, d) = (m.files.load(Relaxed) as usize, m.dirs.load(Relaxed) as usize);
    format!("{} · {}", trn("%n files", f), trn("%n folders", d))
}

/// "1.2 GiB (1 234 567 B)", with … while still counting.
fn size_text(m: &Measure, bytes: bool) -> String {
    let s = m.size.load(Relaxed);
    let more = if m.done() { "" } else { " …" };
    if bytes { format!("{} ({s} B){more}", human_size(s)) } else { format!("{}{more}", human_size(s)) }
}

fn disk_text(m: &Measure) -> Option<String> {
    m.on_disk.load(Relaxed).then(|| format!("{}{}", human_size(m.disk.load(Relaxed)), if m.done() { "" } else { " …" }))
}

pub fn draw(ui: &mut Ui, p: &Props) -> bool {
    if !p.total.done() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
    }
    let w = crate::view::dialog_w().min(if p.items.is_empty() { 420.0 } else { 520.0 });
    ui.set_min_width(w);
    ui.horizontal(|ui| {
        if let Some(u) = &p.icon {
            ui.add(egui::Image::new(u.clone()).fit_to_exact_size(Vec2::splat(64.0)));
        }
        ui.label(egui::RichText::new(tr("Properties")).strong().size(18.0));
    });
    ui.add_space(8.0);
    let t = &p.total;
    let mut rows = p.rows.clone();
    // (After the name, type and place.)
    let at = rows.len().min(3);
    let mut live = vec![(tr("Size").to_string(), size_text(t, true))];
    if let Some(d) = disk_text(t) {
        live.push((tr("On disk").to_string(), d));
    }
    if p.is_dir || !p.items.is_empty() {
        live.push((tr("Contents").to_string(), contents(t)));
    }
    rows.splice(at..at, live);
    egui::Grid::new("props").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
        for (k, v) in &rows {
            ui.label(egui::RichText::new(k).weak());
            ui.add(egui::Label::new(v).wrap());
            ui.end_row();
        }
    });
    if !p.items.is_empty() {
        ui.add_space(10.0);
        ui.separator();
        let max_h = (ui.ctx().content_rect().height() - 380.0).max(160.0);
        egui::ScrollArea::vertical().id_salt("props-items").max_height(max_h).show(ui, |ui| {
            ui.set_min_width(w);
            for (i, it) in p.items.iter().enumerate() {
                ui.push_id(i, |ui| item(ui, it));
                ui.add_space(4.0);
            }
        });
    }
    ui.add_space(8.0);
    ui.button(tr("Close")).clicked()
}

/// One of several: its icon, name, and own figures.
fn item(ui: &mut Ui, it: &Item) {
    let vis = ui.visuals().clone();
    egui::Frame::new().fill(vis.faint_bg_color).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            match &it.icon {
                Some(u) => {
                    ui.add(egui::Image::new(u.clone()).fit_to_exact_size(Vec2::splat(32.0)));
                }
                None => {
                    ui.add_space(32.0);
                }
            }
            ui.vertical(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(&it.name).strong()).truncate());
                let mut line = vec![it.kind.clone(), size_text(&it.m, false)];
                if let Some(d) = disk_text(&it.m) {
                    line.push(format!("{} {d}", tr("On disk").to_lowercase()));
                }
                ui.add(egui::Label::new(egui::RichText::new(line.join(" · ")).weak()).truncate());
                let mut line2 = Vec::new();
                if it.is_dir {
                    line2.push(contents(&it.m));
                }
                if let Some(t) = it.mtime {
                    line2.push(crate::app::fmt_time(t));
                }
                if !line2.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(line2.join(" · ")).weak()).truncate());
                }
            });
        });
    });
}
