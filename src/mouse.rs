//! Mouse extras: dragging items onto a folder, an archive, the other pane
//! or the sidebar, and selecting with a rectangle.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use egui::{Order, Vec2, pos2, vec2};

use crate::app::{App, Cmd, Focus, Menu, MenuItem, Overlay, item};
use crate::i18n::{tr, trn};
use crate::vfs::{Dest, Location};

/// Items being dragged.
pub struct Drag {
    pub sources: Vec<Location>,
    pub label: String,
    /// Up to three of the items, for the icons following the pointer.
    pub shown: Vec<(crate::vfs::Entry, Option<PathBuf>)>,
    /// Where they lie (tab, pane), drawn faded there meanwhile.
    pub from: (usize, usize),
    pub names: HashSet<String>,
    /// The stack's place (trailing the pointer) and its sway: angle, spin.
    pub pos: Option<egui::Pos2>,
    pub angle: f32,
    pub spin: f32,
    /// Pointer last frame and its smoothed sideways speed.
    pub last: Option<egui::Pos2>,
    pub vx: f32,
}

/// `Drag::from` of files dragged in from another program.
pub const EXTERNAL: (usize, usize) = (usize::MAX, usize::MAX);

impl Drag {
    /// Files coming from another program.
    pub fn external(files: Vec<PathBuf>) -> Self {
        use std::os::unix::fs::MetadataExt;
        let entry = |p: &PathBuf| {
            let m = std::fs::metadata(p).ok();
            crate::vfs::Entry {
                name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                is_dir: m.as_ref().is_some_and(|m| m.is_dir()),
                is_link: false,
                size: m.as_ref().map_or(0, |m| m.len()),
                mtime: m.as_ref().map(|m| m.mtime()),
                mode: m.as_ref().map_or(0o644, |m| m.mode()),
            }
        };
        let label = match files.len() {
            1 => entry(&files[0]).name,
            n => trn("%n items", n),
        };
        let shown = files.iter().take(3).map(|p| (entry(p), Some(p.clone()))).collect();
        let sources = files.into_iter().map(Location::Local).collect();
        Drag { sources, label, shown, from: EXTERNAL, names: HashSet::new(), pos: None, angle: 0.0, spin: 0.0, last: None, vx: 0.0 }
    }
}

/// Size of the icons following the pointer.
const ICON: f32 = 48.0;

/// A selection rectangle being drawn in a pane, in content coordinates
/// (so it stays put while the list scrolls).
pub struct Band {
    pub start: Vec2,
    /// What was selected before (kept with Ctrl).
    pub base: HashSet<String>,
}

/// Can items be dropped onto this entry? Folders, and archives Polka can
/// write into.
pub fn drop_target(e: &crate::vfs::Entry, local: bool) -> bool {
    e.is_dir || (local && (crate::archive_rw::writable_name(&e.name) || e.name.ends_with(".desktop") || e.mode & 0o111 != 0))
}

impl App {
    pub fn drag_start(&mut self, pane: usize, i: usize) {
        self.set_side(pane);
        let tab = self.tab_of(pane);
        self.focus = Focus::Panes;
        let p = &mut self.tabs[tab].pane;
        let Some(name) = p.entries.get(i).map(|e| e.name.clone()) else { return };
        // Dragging something unselected drags just that.
        if !p.selected.contains(&name) {
            p.selected.clear();
            p.set_cursor(i);
        }
        let targets = p.targets();
        if targets.is_empty() {
            return;
        }
        let sources: Vec<Location> = targets.iter().map(|e| p.entry_loc(e)).collect();
        let label = match targets.len() {
            1 => targets[0].name.clone(),
            n => trn("%n items", n),
        };
        let shown = targets.iter().take(3).map(|e| (e.clone(), p.entry_path(e))).collect();
        let names = targets.iter().map(|e| e.name.clone()).collect();
        self.drag = Some(Drag { sources, label, shown, from: (tab, pane), names, pos: None, angle: 0.0, spin: 0.0, last: None, vx: 0.0 });
    }

    /// Dropped on a pane: onto one of its entries, or anywhere else in it.
    pub fn drop_on_pane(&mut self, pane: usize, entry: Option<usize>) {
        let p = self.pane_at(pane);
        let local = p.loc.local_path().is_some();
        let target = entry.and_then(|i| p.entries.get(i)).filter(|e| drop_target(e, local));
        // A program, script or launcher takes the files as they are; the
        // folder it lies in still offers moving and copying.
        let app = target.and_then(|e| p.entry_path(e)).filter(|a| !a.is_dir() && crate::launch::runs(a));
        let (dest, name) = match target {
            Some(e) if e.is_dir => (p.entry_loc(e).dest(), e.name.clone()),
            Some(e) if app.is_none() => (p.entry_path(e).and_then(|a| Location::Archive { archive: a, inner: String::new() }.dest()), e.name.clone()),
            _ => (p.loc.dest(), p.loc.title()),
        };
        self.set_side(pane);
        match (dest, app) {
            (None, None) => {
                self.drag = None;
                self.toast(tr("Can't put anything here"), true);
            }
            (dest, app) => self.drop_to(dest, app, name),
        }
    }

    /// Dropped on a sidebar entry: the trash throws away, places receive.
    pub fn drop_on_side(&mut self, item: usize) {
        let Some(it) = self.side_items.get(item).cloned() else { return };
        let trash = dirs::data_dir().map(|d| d.join("Trash/files"));
        if it.loc.local_path().is_some_and(|p| Some(p) == trash.as_deref()) {
            let Some(d) = self.drag.take() else { return };
            let paths: Vec<PathBuf> = d.sources.iter().filter_map(|l| l.local_path().map(|p| p.to_path_buf())).collect();
            if paths.len() == d.sources.len() {
                self.drop_done();
                self.run(Cmd::TrashNow(paths));
            } else {
                self.toast(tr("Only files on disk can go to the trash"), true);
            }
            return;
        }
        match it.loc.dest().filter(|_| !it.usage && it.drive.as_ref().is_none_or(|d| d.mount.is_some())) {
            Some(d) => self.drop_to(Some(d), None, it.label.clone()),
            None => self.drag = None,
        }
    }

    /// Let go over a place: ask what to do (move, copy, or run the program
    /// dropped on), unless Ctrl (copy) or Shift (move) already says.
    fn drop_to(&mut self, dest: Option<Dest>, app: Option<PathBuf>, name: String) {
        let Some(d) = self.drag.take() else { return };
        let local: Vec<&std::path::Path> = d.sources.iter().filter_map(|l| l.local_path()).collect();
        let all_local = local.len() == d.sources.len();
        let mut dest = dest;
        if let Some(Dest::Dir(dir)) = &dest {
            // Back where they lie: nothing to move or copy there.
            if all_local && local.iter().all(|s| s.parent() == Some(dir.as_path())) {
                dest = None;
            } else if local.iter().any(|s| dir.starts_with(s)) {
                if app.is_none() {
                    return self.toast(tr("A folder can't go inside itself"), true);
                }
                dest = None;
            }
        }
        if dest.is_none() && app.is_none() {
            return;
        }
        let m = self.ctx.input(|i| i.modifiers);
        if let Some(dest) = dest.clone().filter(|_| m.command || m.shift) {
            self.drop_done();
            return self.request_transfer(d.sources, dest, m.shift);
        }
        // The usual choice comes first: a move within one disk, else a copy.
        let same_disk = match &dest {
            Some(Dest::Dir(dir)) => {
                let dev = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.dev()).ok();
                all_local && local.first().and_then(|s| dev(s)).is_some_and(|a| dev(dir) == Some(a))
            }
            _ => false,
        };
        let mut items = Vec::new();
        if let Some(dest) = &dest {
            let go = |mv| Cmd::DropTransfer { sources: d.sources.clone(), dest: dest.clone(), mv };
            items.push(item(tr("Move here"), go(true)));
            items.push(item(tr("Copy here"), go(false)));
            if let (Dest::Dir(dir), true) = (dest, all_local) {
                let sources = local.iter().map(|p| p.to_path_buf()).collect();
                items.push(item(tr("Create link here"), Cmd::DropLink { sources, dir: dir.clone() }));
            }
        }
        if let Some(a) = &app {
            let files: Vec<PathBuf> = local.iter().map(|p| p.to_path_buf()).collect();
            let label = format!("{} {}", tr("Open with"), crate::launch::app_name(a));
            items.push(MenuItem { enabled: all_local, ..item(&label, Cmd::OpenWith { app: a.clone(), files }) });
        }
        items.push(item(tr("Cancel"), Cmd::Nothing));
        let mut menu = Menu::new(format!("{} → {name}", d.label), items);
        menu.cursor = if dest.is_some() && !same_disk { 1 } else { 0 };
        self.overlay = Some(Overlay::Menu(menu));
    }

    /// Dropped: the selection is done with, and the cursor stays hidden
    /// instead of jumping to the top of the list in the items' place.
    pub(crate) fn drop_done(&mut self) {
        for side in [0, 1] {
            let p = self.pane_at_mut(side);
            p.selected.clear();
            p.cursor_hidden = true;
        }
    }

    /// Drags from and to other programs, turned into the pointer moves and
    /// release that drags inside the window are made of.
    pub fn dnd_tick(&mut self) {
        let Some(d) = &self.dnd else { return };
        let evs: Vec<crate::dnd::Ev> = d.events.try_iter().collect();
        let zoom = self.ctx.zoom_factor();
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        for e in evs {
            use crate::dnd::Ev;
            match e {
                Ev::Enter { x, y, files } => {
                    let p = pos2(x / zoom, y / zoom);
                    self.dnd_pos = Some(p);
                    self.dnd_inject.push(egui::Event::PointerMoved(p));
                    // No press for egui (it would start a selection where the
                    // pointer came in); the release alone drops.
                    if let Some(files) = files {
                        self.drag = Some(Drag::external(files));
                        self.dnd_in = true;
                    }
                }
                Ev::Motion { x, y } => {
                    let p = pos2(x / zoom, y / zoom);
                    self.dnd_pos = Some(p);
                    self.dnd_inject.push(egui::Event::PointerMoved(p));
                }
                Ev::Leave => {
                    if std::mem::take(&mut self.dnd_in) {
                        self.drag = None;
                        self.release_far();
                    }
                }
                Ev::Drop => {
                    let p = self.dnd_pos.unwrap_or_default();
                    self.dnd_inject.push(egui::Event::PointerMoved(p));
                    // From elsewhere egui saw no press: a press and release
                    // right where it was let go. Ours has its press still on.
                    if self.dnd_in {
                        self.dnd_inject.push(button(p, true));
                    }
                    self.dnd_inject.push(button(p, false));
                    // Our own drag, dropped back here (its end follows).
                    self.dnd_dropped = !self.dnd_in;
                    self.dnd_in = false;
                    self.dnd_release = Some(std::time::Instant::now());
                }
                Ev::OutEnded => {
                    self.dnd_out = false;
                    // Dropped here: the release above does it. Elsewhere: the
                    // button egui still thinks is down goes up, out of the way.
                    if !std::mem::take(&mut self.dnd_dropped) {
                        self.drag = None;
                        self.release_far();
                    }
                }
            }
        }
    }

    fn release_far(&mut self) {
        let far = pos2(-10000.0, -10000.0);
        self.dnd_inject.push(egui::Event::PointerMoved(far));
        self.dnd_inject.push(egui::Event::PointerButton { pos: far, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        self.dnd_inject.push(egui::Event::PointerGone);
    }

    /// Files dropped with no place to point at (X11): into the panel.
    pub fn drop_external(&mut self, files: Vec<PathBuf>) {
        if files.is_empty() {
            return;
        }
        self.drag = Some(Drag::external(files));
        let pane = self.side();
        self.drop_on_pane(pane, None);
    }

    /// The dragged items follow the pointer as a little stack that trails
    /// behind and sways on a spring; letting go anywhere else drops nothing.
    pub fn draw_drag(&mut self, ctx: &egui::Context) {
        if self.drag.is_none() {
            return;
        }
        // (One from elsewhere has its press handed to egui a frame later.)
        // A drop's release handed to egui: wait till it's seen (a second at most).
        if ctx.input(|i| i.pointer.primary_released()) || self.dnd_release.is_some_and(|t| t.elapsed().as_secs_f32() > 1.0) {
            self.dnd_release = None;
        }
        if !ctx.input(|i| i.pointer.primary_down()) && !self.dnd_in && self.dnd_release.is_none() {
            self.drag = None;
            return;
        }
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let Some(p) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let accent = ctx.style_of(ctx.theme()).visuals.selection.bg_fill;
        // The stack's pictures (paths), for the system's drag.
        let pics_for_icon: Vec<String> = if self.dnd_out || ctx.content_rect().contains(p) {
            Vec::new()
        } else {
            let shown = self.drag.as_ref().map(|d| d.shown.clone()).unwrap_or_default();
            let thumbs = self.cfg.thumbnails;
            shown
                .iter()
                .filter_map(|(e, path)| {
                    let thumb = match (path, e.mtime) {
                        (Some(p), Some(t)) if thumbs && !e.is_dir => self.icons.thumb(p, t).map(|(u, _)| u),
                        _ => None,
                    };
                    thumb.or_else(|| self.icons.for_entry(e, path.as_deref(), 64))
                })
                .collect()
        };
        // Pulled out of the window: the system's drag takes the files on, to
        // other programs (and back here, if it returns).
        if !self.dnd_out && !ctx.content_rect().contains(p) {
            if let (Some(dnd), Some(d)) = (&self.dnd, &self.drag) {
                let files: Vec<PathBuf> = d.sources.iter().filter_map(|l| l.local_path().map(|p| p.to_path_buf())).collect();
                if d.from != EXTERNAL && files.len() == d.sources.len() {
                    let icon = drag_icon(&pics_for_icon, d.sources.len(), accent, ctx.pixels_per_point());
                    dnd.start(files, icon);
                    self.dnd_out = true;
                    self.dnd_dropped = false;
                }
            }
        }
        let (dt, m) = ctx.input(|i| (i.stable_dt.clamp(0.001, 0.05), i.modifiers));
        // Icons first (they need `self.icons`), then the motion.
        let shown = self.drag.as_ref().map(|d| d.shown.clone()).unwrap_or_default();
        let thumbs = self.cfg.thumbnails;
        let px = ((ICON * ctx.pixels_per_point()).ceil() as u16).max(32);
        let pics: Vec<(Option<String>, Option<(String, f32)>)> = shown
            .iter()
            .map(|(e, path)| {
                let thumb = match (path, e.mtime) {
                    (Some(p), Some(t)) if thumbs && !e.is_dir => self.icons.thumb(p, t),
                    _ => None,
                };
                let icon = if thumb.is_some() { None } else { self.icons.for_entry(e, path.as_deref(), px) };
                (icon, thumb)
            })
            .collect();
        let Some(d) = self.drag.as_mut() else { return };
        // The stack trails the pointer a little…
        let target = p + vec2(10.0, 14.0);
        let pos = d.pos.map_or(target, |q| q + (target - q) * (1.0 - (-dt * 20.0).exp()));
        d.pos = Some(pos);
        // …and tilts against the motion, swinging back past upright when it
        // stops: a damped spring.
        // The pointer's own speed, smoothed (egui's needs a dense history).
        let vx = d.last.map_or(0.0, |l| (p.x - l.x) / dt);
        d.last = Some(p);
        d.vx += (vx - d.vx) * (1.0 - (-dt * 30.0).exp());
        let want = (-d.vx * 0.0012).clamp(-0.5, 0.5);
        let acc = 170.0 * (want - d.angle) - 9.0 * d.spin;
        d.spin += acc * dt;
        d.angle = (d.angle + d.spin * dt).clamp(-0.8, 0.8);
        let (angle, count, label) = (d.angle, d.sources.len(), d.label.clone());
        let how = if m.command {
            tr("Copy")
        } else if m.shift {
            tr("Move")
        } else {
            ""
        };
        egui::Area::new("drag-ghost".into()).order(Order::Tooltip).fixed_pos(pos).interactable(false).show(ctx, |ui| {
            let (area, _) = ui.allocate_exact_size(vec2(ICON + 40.0, ICON + 46.0), egui::Sense::hover());
            let n = pics.len();
            // Back to front, each a bit lower right and swinging a bit less.
            for (k, (icon, thumb)) in pics.into_iter().enumerate().rev() {
                let r = egui::Rect::from_min_size(area.min + vec2(k as f32 * 7.0, k as f32 * 5.0), egui::Vec2::splat(ICON));
                let a = angle * (1.0 - k as f32 * 0.18);
                let tint = egui::Color32::WHITE.gamma_multiply(if k == 0 { 0.95 } else { 0.8 });
                // Hung from the top middle, like from the pointer.
                let pivot = vec2(0.5, 0.0);
                if let Some((uri, aspect)) = thumb {
                    let size = if aspect >= 1.0 { vec2(ICON, ICON / aspect) } else { vec2(ICON * aspect, ICON) };
                    let r = egui::Rect::from_center_size(r.center(), size);
                    egui::Image::new(uri).corner_radius(3).rotate(a, pivot).tint(tint).paint_at(ui, r);
                } else if let Some(u) = icon {
                    egui::Image::new(u).rotate(a, pivot).tint(tint).paint_at(ui, r);
                }
            }
            if count > 1 {
                let c = area.min + vec2(ICON + (n.saturating_sub(1)) as f32 * 7.0 - 2.0, 4.0);
                ui.painter().circle_filled(c, 11.0, accent);
                ui.painter().text(c, egui::Align2::CENTER_CENTER, count.to_string(), egui::FontId::proportional(12.0), egui::Color32::WHITE);
            }
            let text = if how.is_empty() { label } else { format!("{label} · {how}") };
            let font = egui::FontId::proportional(12.5);
            let g = ui.painter().layout_no_wrap(text, font, ui.visuals().strong_text_color());
            let chip = egui::Rect::from_min_size(pos2(area.min.x, area.min.y + ICON + 18.0), g.size() + vec2(12.0, 6.0));
            ui.painter().rect(chip, 6.0, ui.visuals().window_fill, ui.visuals().window_stroke, egui::StrokeKind::Inside);
            ui.painter().galley(chip.min + vec2(6.0, 3.0), g, ui.visuals().strong_text_color());
        });
        ctx.request_repaint();
    }
}

/// One picture file as RGBA at `px` pixels (SVG drawn, others scaled).
fn load_rgba(uri: &str, px: u32) -> Option<image::RgbaImage> {
    let path = std::path::Path::new(uri.strip_prefix("file://")?);
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
        let data = std::fs::read(path).ok()?;
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let mut pm = resvg::tiny_skia::Pixmap::new(px, px)?;
        let s = tree.size();
        let k = px as f32 / s.width().max(s.height());
        let t = resvg::tiny_skia::Transform::from_scale(k, k).post_translate((px as f32 - s.width() * k) / 2.0, (px as f32 - s.height() * k) / 2.0);
        resvg::render(&tree, t, &mut pm.as_mut());
        // Premultiplied already; kept that way below.
        return image::RgbaImage::from_raw(px, px, pm.take());
    }
    let img = image::open(path).ok()?;
    let img = img.resize(px, px, image::imageops::FilterType::Triangle).to_rgba8();
    // Premultiply, centred in a square.
    let mut out = image::RgbaImage::new(px, px);
    let (ox, oy) = ((px - img.width()) / 2, (px - img.height()) / 2);
    for (x, y, p) in img.enumerate_pixels() {
        let a = p[3] as u32;
        out.put_pixel(ox + x, oy + y, image::Rgba([(p[0] as u32 * a / 255) as u8, (p[1] as u32 * a / 255) as u8, (p[2] as u32 * a / 255) as u8, p[3]]));
    }
    Some(out)
}

/// Digits 0-9, 3×5 dots each (rows top down, 3 bits a row).
const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 2, 2, 2],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

/// The stack following the pointer, for the system's drag: up to three
/// pictures, each a bit lower right, and a count when there are more.
pub fn drag_icon(pics: &[String], count: usize, accent: egui::Color32, ppp: f32) -> Option<crate::dnd::Icon> {
    let scale = ppp.ceil().max(1.0) as u32;
    let (w, h) = (96 * scale, 84 * scale);
    let size = 48 * scale;
    let mut canvas = image::RgbaImage::new(w, h);
    let blend = |c: &mut image::RgbaImage, x: i64, y: i64, p: [u8; 4]| {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            return;
        }
        let d = c.get_pixel_mut(x as u32, y as u32);
        let a = p[3] as u32;
        for i in 0..4 {
            d[i] = (p[i] as u32 + d[i] as u32 * (255 - a) / 255).min(255) as u8;
        }
    };
    let imgs: Vec<image::RgbaImage> = pics.iter().take(3).filter_map(|u| load_rgba(u, size)).collect();
    if imgs.is_empty() {
        return None;
    }
    // Back to front, like the stack in the window.
    for (k, img) in imgs.iter().enumerate().rev() {
        let (ox, oy) = ((10 + k as u32 * 7) * scale, (14 + k as u32 * 5) * scale);
        for (x, y, p) in img.enumerate_pixels() {
            blend(&mut canvas, (ox + x) as i64, (oy + y) as i64, p.0);
        }
    }
    if count > 1 {
        let (cx, cy, r) = (((10 + 48 + (imgs.len() as u32 - 1) * 7) as f32 - 2.0) * scale as f32, 18.0 * scale as f32, 11.0 * scale as f32);
        for y in (cy - r) as i64..=(cy + r) as i64 {
            for x in (cx - r) as i64..=(cx + r) as i64 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let cov = (r - d + 0.5).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let a = (255.0 * cov) as u32;
                    blend(&mut canvas, x, y, [(accent.r() as u32 * a / 255) as u8, (accent.g() as u32 * a / 255) as u8, (accent.b() as u32 * a / 255) as u8, a as u8]);
                }
            }
        }
        let text = count.min(999).to_string();
        let dot = 2 * scale as i64;
        let tw = text.len() as i64 * 4 * dot - dot;
        let (x0, y0) = (cx as i64 - tw / 2, cy as i64 - 5 * dot / 2);
        for (i, ch) in text.bytes().enumerate() {
            let g = DIGITS[(ch - b'0') as usize];
            for (row, bits) in g.iter().enumerate() {
                for col in 0..3 {
                    if bits & (4 >> col) != 0 {
                        for dy in 0..dot {
                            for dx in 0..dot {
                                blend(&mut canvas, x0 + i as i64 * 4 * dot + col * dot + dx, y0 + row as i64 * dot + dy, [255, 255, 255, 255]);
                            }
                        }
                    }
                }
            }
        }
    }
    // RGBA premultiplied → Wayland's ARGB8888 (BGRA in memory).
    let mut pixels = canvas.into_raw();
    for p in pixels.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    Some(crate::dnd::Icon { w, h, pixels, scale: scale as i32 })
}

#[cfg(test)]
mod tests {
    #[test]
    fn drag_picture() {
        let dir = std::env::temp_dir().join(format!("polka-drag-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        image::RgbaImage::from_fn(32, 32, |x, y| image::Rgba([200, (x * 8) as u8, (y * 8) as u8, 255])).save(&png).unwrap();
        let svg = dir.join("b.svg");
        std::fs::write(&svg, r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect x="1" y="1" width="14" height="14" rx="3" fill="#3080f0"/></svg>"##).unwrap();
        let pics = vec![crate::icons::uri(&png), crate::icons::uri(&svg), crate::icons::uri(&png)];
        let icon = super::drag_icon(&pics, 12, egui::Color32::from_rgb(40, 120, 220), 2.0).expect("no picture");
        assert_eq!((icon.w, icon.h, icon.scale), (192, 168, 2));
        assert_eq!(icon.pixels.len(), 192 * 168 * 4);
        assert!(icon.pixels.chunks(4).filter(|p| p[3] > 0).count() > 3000);
        if let Some(d) = std::env::var_os("POLKA_SHOT_DIR") {
            let mut rgba = icon.pixels.clone();
            for p in rgba.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
            image::RgbaImage::from_raw(icon.w, icon.h, rgba).unwrap().save(std::path::Path::new(&d).join("drag-icon.png")).unwrap();
        }
    }
}
