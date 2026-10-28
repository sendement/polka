//! Documents, to look at: PDF (pages drawn by poppler's pdftoppm, their
//! text from pdftotext laid over them, unseen, for the mouse to select and
//! copy), word processing (docx, odt: paragraphs, headings, lists, tables,
//! pictures), presentations (pptx, odp: each slide's boxes where they
//! stand) and spreadsheets (xlsx, xls, ods: a grid a sheet). The old binary
//! ones (doc, ppt, rtf) go through LibreOffice to PDF when there's one.
//! The gamepad scrolls, pages and zooms; the mouse selects and copies.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::text::LayoutJob;
use egui::{Align2, Color32, FontFamily, FontId, Rect, Sense, Stroke, TextFormat, Ui, Vec2, pos2, vec2};

use crate::i18n::tr;
use crate::input::Action;

/// What a document is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    Pdf,
    Word,
    Slides,
    Sheets,
    /// doc, ppt, rtf…: to PDF by LibreOffice first.
    Office,
}

pub fn doc_kind(name: &str) -> Option<DocKind> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    Some(match ext.as_str() {
        "pdf" => DocKind::Pdf,
        "docx" | "docm" | "dotx" | "odt" | "ott" => DocKind::Word,
        "pptx" | "pptm" | "ppsx" | "potx" | "odp" | "otp" => DocKind::Slides,
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" | "ots" | "csv" | "tsv" => DocKind::Sheets,
        "doc" | "dot" | "rtf" | "ppt" | "pps" | "odg" | "wpd" | "pages" | "key" => DocKind::Office,
        _ => return None,
    })
}

/// Which "Open in Polka" switch a document goes by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Pdf,
    Text,
    Sheets,
    Slides,
}

pub fn family(name: &str) -> Option<Family> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    Some(match doc_kind(name)? {
        DocKind::Pdf => Family::Pdf,
        DocKind::Word => Family::Text,
        DocKind::Sheets => Family::Sheets,
        DocKind::Slides => Family::Slides,
        DocKind::Office if matches!(ext.as_str(), "ppt" | "pps" | "key") => Family::Slides,
        DocKind::Office => Family::Text,
    })
}

// ---------------------------------------------------------------- the state

pub enum Body {
    Loading(Receiver<Result<Body, String>>),
    Pdf(Pdf),
    Flow(Flow),
    Slides(Slides),
    Sheets(Sheets),
    Failed(String),
}

pub struct DocState {
    pub body: Body,
    /// 1 = the width of the view (pages, slides); more is closer.
    pub zoom: f32,
    /// Asked for by the gamepad, applied next frame.
    pub scroll_to: Option<Vec2>,
    /// What last showed: offset, size of the view, its content.
    pub offset: Vec2,
    pub view: Vec2,
    pub content: Vec2,
    /// The page (slide) in view, and where each starts.
    pub page: usize,
    pub page_tops: Vec<f32>,
    /// The sheet shown.
    pub sheet: usize,
    /// Its scroll's own (not the document's before it).
    salt: egui::Id,
}

/// Documents opened so far: each one's scroll starts at its top.
static OPENED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl DocState {
    pub fn open(path: &Path, name: &str, ctx: &egui::Context) -> DocState {
        let (tx, rx) = channel();
        let (path, kind, c) = (path.to_path_buf(), doc_kind(name), ctx.clone());
        std::thread::spawn(move || {
            let r = match kind {
                Some(DocKind::Pdf) => Pdf::open(&path, c.clone()).map(Body::Pdf),
                Some(DocKind::Office) => office_pdf(&path).and_then(|p| Pdf::open(&p, c.clone())).map(Body::Pdf),
                Some(DocKind::Word) => Flow::open(&path).map(Body::Flow),
                Some(DocKind::Slides) => Slides::open(&path).map(Body::Slides),
                Some(DocKind::Sheets) => crate::table::read(&path, crate::table::Delim::Auto).map(|s| Body::Sheets(Sheets::new(s))),
                None => Err("not a document".into()),
            };
            let _ = tx.send(r);
            c.request_repaint();
        });
        DocState { body: Body::Loading(rx), zoom: 1.0, scroll_to: None, offset: Vec2::ZERO, view: Vec2::ZERO, content: Vec2::ZERO, page: 0, page_tops: Vec::new(), sheet: 0, salt: egui::Id::new(("doc-scroll", OPENED.fetch_add(1, std::sync::atomic::Ordering::Relaxed))) }
    }

    fn poll(&mut self) {
        if let Body::Loading(rx) = &self.body {
            if let Ok(r) = rx.try_recv() {
                self.body = r.unwrap_or_else(Body::Failed);
            }
        }
    }

    pub fn pages(&self) -> usize {
        match &self.body {
            Body::Pdf(p) => p.sizes.len(),
            Body::Slides(s) => s.slides.len(),
            Body::Sheets(s) => s.sheets.len(),
            _ => 0,
        }
    }

    /// The gamepad: ↑ ↓ scroll, ← → a page (a slide, a sheet's columns),
    /// the triggers zoom (a spreadsheet's: its sheets), ✕ back to the
    /// width, Home / End.
    pub fn input(&mut self, a: Action) {
        let line = 60.0;
        let page = (self.view.y - 40.0).max(line);
        let mut s = self.offset;
        let sheets = matches!(self.body, Body::Sheets(_));
        match a {
            Action::Up => s.y -= line,
            Action::Down => s.y += line,
            Action::Left | Action::Right if sheets => s.x += if a == Action::Left { -200.0 } else { 200.0 },
            Action::Left | Action::Right if !self.page_tops.is_empty() => {
                let to = if a == Action::Left { self.page.saturating_sub(1) } else { (self.page + 1).min(self.page_tops.len() - 1) };
                // (Back on a page's top first, if it's scrolled into.)
                let at = if a == Action::Left && self.offset.y > self.page_tops[self.page] + 4.0 { self.page } else { to };
                s.y = self.page_tops[at];
            }
            Action::Left | Action::Right => s.y += if a == Action::Left { -page } else { page },
            Action::LeftTrigger | Action::RightTrigger if sheets => {
                let n = self.pages().max(1);
                self.sheet = if a == Action::LeftTrigger { (self.sheet + n - 1) % n } else { (self.sheet + 1) % n };
                s = Vec2::ZERO;
            }
            Action::LeftTrigger | Action::RightTrigger => return self.zoom_by(if a == Action::RightTrigger { 1.25 } else { 0.8 }),
            Action::Accept => return self.zoom_to(1.0),
            Action::Home => s = Vec2::ZERO,
            Action::End => s.y = self.content.y,
            _ => return,
        }
        let max = (self.content - self.view).max(Vec2::ZERO);
        self.scroll_to = Some(s.clamp(Vec2::ZERO, max));
    }

    /// Closer (`k` > 1) or farther, about the view's middle.
    pub fn zoom_by(&mut self, k: f32) {
        self.zoom_to(self.zoom * k);
    }

    pub fn zoom_to(&mut self, z: f32) {
        let old = self.zoom;
        self.zoom = z.clamp(0.3, 6.0);
        let s = (self.offset + self.view / 2.0) * (self.zoom / old) - self.view / 2.0;
        self.scroll_to = Some(s.max(Vec2::ZERO));
    }

    /// For the status line: page 3/12 · 120%.
    pub fn info(&self) -> String {
        let mut parts = Vec::new();
        match &self.body {
            Body::Pdf(p) => parts.push(format!("{} {}/{}", tr("page"), self.page + 1, p.sizes.len())),
            Body::Slides(s) => parts.push(format!("{} {}/{}", tr("slide"), self.page + 1, s.slides.len())),
            Body::Sheets(s) => {
                if let Some(sh) = s.sheets.get(self.sheet) {
                    parts.push(format!("{} · {}/{}", sh.name, self.sheet + 1, s.sheets.len()));
                }
            }
            _ => {}
        }
        if (self.zoom - 1.0).abs() > 1e-3 {
            parts.push(format!("{:.0}%", self.zoom * 100.0));
        }
        parts.join(" · ")
    }
}

/// The page's look: paper, its ink, the gap between pages.
const PAPER: Color32 = Color32::from_rgb(255, 255, 255);
const INK: Color32 = Color32::from_rgb(28, 28, 28);
const GAP: f32 = 14.0;

pub fn draw(ui: &mut Ui, d: &mut DocState) {
    d.poll();
    let vis = ui.visuals().clone();
    let msg = match &d.body {
        Body::Loading(_) => Some(None),
        Body::Failed(e) => Some(Some(e.clone())),
        _ => None,
    };
    if let Some(m) = msg {
        ui.centered_and_justified(|ui| match m {
            None => {
                ui.add(egui::Spinner::new().size(28.0));
            }
            Some(e) => {
                ui.label(egui::RichText::new(e).color(vis.error_fg_color));
            }
        });
        if matches!(d.body, Body::Loading(_)) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        return;
    }
    // Ctrl + the wheel (a pinch): zoom.
    let pinch = ui.input(|i| i.zoom_delta());
    if (pinch - 1.0).abs() > 1e-3 && ui.rect_contains_pointer(ui.max_rect()) {
        d.zoom_by(pinch);
    }
    if let Body::Sheets(_) = &d.body {
        let view = draw_sheets(ui, d);
        return zoom_buttons(ui, d, view);
    }
    // (Under the pages: the text on them takes the drags.)
    copy_menu(ui, ui.available_rect_before_wrap());
    let mut area = egui::ScrollArea::both().id_salt(d.salt).auto_shrink([false, false]);
    if let Some(s) = d.scroll_to.take() {
        area = area.scroll_offset(s);
    }
    let zoom = d.zoom;
    let mut tops = Vec::new();
    let out = area.show(ui, |ui| {
        let avail = ui.available_width();
        let width = ((avail - 2.0 * GAP).max(200.0) * zoom).round();
        match &mut d.body {
            Body::Pdf(p) => p.draw(ui, width, avail, &mut tops),
            Body::Slides(s) => s.draw(ui, width, avail, &mut tops),
            Body::Flow(f) => f.draw(ui, ((avail - 2.0 * GAP).min(860.0) * zoom).round(), avail, zoom),
            _ => {}
        }
    });
    d.offset = out.state.offset;
    d.view = out.inner_rect.size();
    d.content = out.content_size;
    // The page in view: the last one starting above its middle.
    d.page = tops.iter().rposition(|t| *t <= d.offset.y + d.view.y * 0.4).unwrap_or(0);
    d.page_tops = tops;
    zoom_buttons(ui, d, out.inner_rect);
}

/// The mouse's zoom at the view's bottom right: − the percentage (back to
/// the width) +. Not while the gamepad's in use: its triggers do it.
fn zoom_buttons(ui: &mut Ui, d: &mut DocState, view: Rect) {
    if ui.ctx().data(|m| m.get_temp::<bool>(egui::Id::new("pad-active"))).unwrap_or(false) || view.width() < 200.0 {
        return;
    }
    let vis = ui.visuals().clone();
    let (bw, pw, h) = (30.0, 58.0, 28.0);
    let size = vec2(2.0 * bw + pw + 8.0, h + 8.0);
    let r = Rect::from_min_size(view.max - size - vec2(16.0, 12.0), size);
    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(r));
    bar.painter().rect(r, 8.0, vis.window_fill, vis.window_stroke, egui::StrokeKind::Inside);
    let pct = format!("{:.0}%", d.zoom * 100.0);
    let mut x = r.min.x + 4.0;
    for (w, label, tip, z) in [(bw, "−", tr("Zoom out"), d.zoom * 0.8), (pw, pct.as_str(), tr("Fit width"), 1.0), (bw, "+", tr("Zoom in"), d.zoom * 1.25)] {
        let b = Rect::from_min_size(pos2(x, r.min.y + 4.0), vec2(w, h));
        x += w;
        let text = egui::RichText::new(label).size(if w == bw { 18.0 } else { 13.0 });
        if bar.put(b, egui::Button::new(text).frame(false)).on_hover_text(tip).clicked() {
            d.zoom_to(z);
        }
    }
}

/// A right click: copy what's selected (the labels' selection hears it).
fn copy_menu(ui: &mut Ui, rect: Rect) {
    let r = ui.interact(rect, ui.id().with("doc-menu"), Sense::click());
    r.context_menu(|ui| {
        if ui.button(tr("Copy")).clicked() {
            ui.ctx().input_mut(|i| i.events.push(egui::Event::Copy));
            ui.close();
        }
    });
}

/// Text laid over a page, unseen: what the mouse selects and copies.
fn ghost_text(ui: &mut Ui, rect: Rect, text: &str) {
    if rect.height() < 2.0 || !ui.is_rect_visible(rect) {
        return;
    }
    // Sized to the line's height, then to its width.
    let size0 = (rect.height() * 0.82).max(4.0);
    let w0 = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(size0), Color32::WHITE).size().x.max(1.0);
    let size = (size0 * rect.width() / w0).clamp(size0 * 0.55, size0 * 1.3);
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat { font_id: FontId::proportional(size), color: Color32::TRANSPARENT, ..Default::default() });
    job.wrap.max_width = f32::INFINITY;
    ui.put(rect, egui::Label::new(job).selectable(true).wrap_mode(egui::TextWrapMode::Extend));
}

// ---------------------------------------------------------------- tools

/// A tool, the host's when in Flatpak.
fn tool(name: &str) -> std::process::Command {
    crate::sandbox::host(std::process::Command::new(name))
}

fn host(p: &Path) -> String {
    crate::sandbox::host_path(p).to_string_lossy().into_owned()
}

/// Where pages are drawn to and documents converted.
fn work_dir() -> Result<PathBuf, String> {
    let d = if cfg!(test) {
        std::env::var_os("POLKA_OFFICE_WORK").map(PathBuf::from).unwrap_or_else(|| crate::vfs::temp_root().join("docs"))
    } else { dirs::cache_dir().ok_or("no cache folder")?.join("polka/docs") };
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

/// An old office file as PDF, by LibreOffice (kept by its size and time).
fn office_pdf(path: &Path) -> Result<PathBuf, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let stamp = format!("{}-{}", meta.len(), meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs()));
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = work_dir()?.join(format!("office-{stem}-{stamp}"));
    let pdf = dir.join(format!("{stem}.pdf"));
    if pdf.exists() {
        return Ok(pdf);
    }
    office_convert(path, "pdf", &dir)
}

/// A document in another format ("docx", "txt:Text (encoded):UTF8"…), by
/// LibreOffice, into this folder; the file it made.
pub fn office_convert(path: &Path, to: &str, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let work = work_dir()?;
    // LibreOffice's Flatpak can't be let into /run or /tmp (where files
    // out of archives wait): a copy, then.
    let hidden = ["/run/", "/tmp/"].iter().any(|r| crate::sandbox::host_path(path).starts_with(r));
    let staged = hidden.then(|| work.join(format!("stage-{}-{:?}", std::process::id(), std::thread::current().id())));
    let path = match &staged {
        Some(stage) => {
            let _ = std::fs::remove_dir_all(stage);
            std::fs::create_dir_all(stage).map_err(|e| e.to_string())?;
            let copy = stage.join(path.file_name().ok_or("no name")?);
            std::fs::copy(path, &copy).map_err(|e| e.to_string())?;
            copy
        }
        None => path.to_path_buf(),
    };
    let r = office_run(&path, to, dir, &work);
    if let Some(stage) = staged {
        let _ = std::fs::remove_dir_all(stage);
    }
    r
}

fn office_run(path: &Path, to: &str, dir: &Path, work: &Path) -> Result<PathBuf, String> {
    let mut cmd = libreoffice(&[work, dir], path).ok_or_else(|| tr("needs LibreOffice").to_string())?;
    // Its own profile: a LibreOffice already open would take the job and
    // drop it.
    let profile = work.join("office-profile");
    let out = cmd
        .arg(format!("-env:UserInstallation={}", crate::icons::uri(&crate::sandbox::host_path(&profile))))
        .args(["--headless", "--convert-to", to, "--outdir"])
        .arg(host(dir))
        .arg(host(path))
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = to.split(':').next().unwrap_or(to);
    let made = dir.join(format!("{stem}.{ext}"));
    if !made.exists() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { "LibreOffice failed".to_string() } else { err });
    }
    Ok(made)
}

/// Where documents being converted wait (LibreOffice may write there).
pub fn office_scratch() -> Result<PathBuf, String> {
    work_dir().map(|w| w.join(format!("convert-{}-{:?}", std::process::id(), std::thread::current().id())))
}

/// There is a LibreOffice to run (asked once).
pub fn have_libreoffice() -> bool {
    if cfg!(test) {
        if let Ok(v) = std::env::var("POLKA_CONVERT_TOOLS") {
            return v.split(',').any(|t| t == "libreoffice");
        }
    }
    static HAVE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAVE.get_or_init(|| libreoffice(&[], Path::new("/")).is_some())
}

/// LibreOffice's Flatpak (on the Steam Deck it's the only way to have it).
const LIBREOFFICE_APP: &str = "org.libreoffice.LibreOffice";

/// LibreOffice to run: the system's, else its Flatpak, let into the
/// folders it writes to and the file's.
fn libreoffice(out: &[&Path], file: &Path) -> Option<std::process::Command> {
    if let Some(t) = ["soffice", "libreoffice"].into_iter().find(|t| !crate::terminal::installed(&[t]).is_empty()) {
        return Some(tool(t));
    }
    let installed = tool("flatpak").args(["info", LIBREOFFICE_APP]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    if !installed {
        return None;
    }
    let mut c = tool("flatpak");
    c.args(["run", "--command=libreoffice"]);
    for o in out {
        c.arg(format!("--filesystem={}", host(o)));
    }
    if let Some(dir) = file.parent() {
        c.arg(format!("--filesystem={}:ro", host(dir)));
    }
    c.arg(LIBREOFFICE_APP);
    Some(c)
}

// ---------------------------------------------------------------- PDF

/// A line of a page's text, where it stands (points from the top left).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub rect: Rect,
    pub text: String,
}

enum Drawn {
    Page { page: usize, dpi: f32, image: egui::ColorImage, lines: Option<Vec<Line>> },
    Failed { page: usize, error: String },
}

pub struct Pdf {
    pub path: PathBuf,
    /// Each page's size in points.
    pub sizes: Vec<Vec2>,
    textures: HashMap<usize, (f32, egui::TextureHandle)>,
    pub lines: HashMap<usize, Vec<Line>>,
    asked: HashSet<usize>,
    failed: HashMap<usize, String>,
    ask: Sender<(usize, f32, bool)>,
    got: Receiver<Drawn>,
}

impl Pdf {
    fn open(path: &Path, ctx: egui::Context) -> Result<Pdf, String> {
        if crate::terminal::installed(&["pdftoppm"]).is_empty() {
            return Err(tr("needs poppler (pdftoppm)").to_string());
        }
        let out = tool("pdfinfo").args(["-f", "1", "-l", "100000"]).arg(host(path)).output().map_err(|e| format!("pdfinfo: {e}"))?;
        let info = String::from_utf8_lossy(&out.stdout);
        let sizes = page_sizes(&info);
        if sizes.is_empty() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if err.is_empty() { tr("No pages").to_string() } else { err });
        }
        // One drawer, pages one after another as they're asked for.
        let (ask, asks) = channel::<(usize, f32, bool)>();
        let (send, got) = channel();
        let file = path.to_path_buf();
        std::thread::spawn(move || {
            let Ok(dir) = work_dir() else { return };
            while let Ok((page, dpi, text)) = asks.recv() {
                let r = match render(&file, page, dpi, &dir) {
                    Ok(image) => Drawn::Page { page, dpi, image, lines: text.then(|| page_text(&file, page, &dir)) },
                    Err(error) => Drawn::Failed { page, error },
                };
                if send.send(r).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        Ok(Pdf { path: path.to_path_buf(), sizes, textures: HashMap::new(), lines: HashMap::new(), asked: HashSet::new(), failed: HashMap::new(), ask, got })
    }

    fn take(&mut self, ctx: &egui::Context) {
        while let Ok(d) = self.got.try_recv() {
            match d {
                Drawn::Page { page, dpi, image, lines } => {
                    self.asked.remove(&page);
                    let t = ctx.load_texture(format!("pdf-{}-{page}", self.path.display()), image, egui::TextureOptions::LINEAR);
                    self.textures.insert(page, (dpi, t));
                    if let Some(l) = lines {
                        self.lines.insert(page, l);
                    }
                }
                Drawn::Failed { page, error } => {
                    self.asked.remove(&page);
                    self.failed.insert(page, error);
                }
            }
        }
    }

    fn draw(&mut self, ui: &mut Ui, width: f32, avail: f32, tops: &mut Vec<f32>) {
        self.take(ui.ctx());
        let ppp = ui.ctx().pixels_per_point();
        let top0 = ui.min_rect().top();
        ui.add_space(GAP);
        let mut visible = Vec::new();
        for (i, size) in self.sizes.clone().iter().enumerate() {
            let scale = width / size.x.max(1.0);
            let h = (size.y * scale).round();
            let x = ((avail - width) / 2.0).max(GAP);
            let (row, _) = ui.allocate_exact_size(vec2((width + 2.0 * x).max(avail), h + GAP), Sense::hover());
            tops.push(row.top() - top0 - GAP);
            let rect = Rect::from_min_size(pos2(row.left() + x, row.top()), vec2(width, h));
            if !ui.is_rect_visible(rect.expand(h)) {
                continue;
            }
            visible.push(i);
            ui.painter().rect_filled(rect.translate(vec2(0.0, 2.0)), 2.0, Color32::from_black_alpha(60));
            ui.painter().rect_filled(rect, 2.0, PAPER);
            // Drawn at the screen's own pixels (again when zoomed a lot).
            let want = (width * ppp * 72.0 / size.x.max(1.0) / 12.0).ceil() * 12.0;
            let have = self.textures.get(&i).map(|t| t.0);
            if let Some((_, t)) = self.textures.get(&i) {
                egui::Image::new(egui::load::SizedTexture::new(t.id(), rect.size())).paint_at(ui, rect);
            } else if let Some(e) = self.failed.get(&i) {
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, e, FontId::proportional(13.0), Color32::from_rgb(180, 40, 40));
            } else {
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, format!("{}", i + 1), FontId::proportional(28.0), Color32::from_gray(200));
            }
            let stale = have.is_none_or(|d| want > d * 1.2 || want < d * 0.5);
            if stale && !self.asked.contains(&i) && !self.failed.contains_key(&i) {
                self.asked.insert(i);
                let _ = self.ask.send((i, want.clamp(24.0, 600.0), !self.lines.contains_key(&i)));
            }
            if let Some(lines) = self.lines.get(&i) {
                let k = width / size.x.max(1.0);
                // (On a child: put() would move the pages' own layout on.)
                let mut over = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                for l in lines {
                    let r = Rect::from_min_max(rect.min + l.rect.min.to_vec2() * k, rect.min + l.rect.max.to_vec2() * k);
                    ghost_text(&mut over, r, &l.text);
                }
            }
        }
        // Far from view: let their pictures go.
        if let (Some(a), Some(b)) = (visible.first(), visible.last()) {
            let (a, b) = (a.saturating_sub(4), b + 4);
            self.textures.retain(|k, _| (a..=b).contains(k));
        }
    }
}

/// pdfinfo's "Page    1 size: 595.276 x 841.89 pts (A4)".
pub fn page_sizes(info: &str) -> Vec<Vec2> {
    let mut v = Vec::new();
    for l in info.lines() {
        let Some(rest) = l.strip_prefix("Page") else { continue };
        let Some((_, size)) = rest.split_once("size:") else { continue };
        let mut it = size.split_whitespace();
        let (Some(w), Some("x"), Some(h)) = (it.next(), it.next(), it.next()) else { continue };
        if let (Ok(w), Ok(h)) = (w.parse::<f32>(), h.parse::<f32>()) {
            // A page turned on its side.
            let rot = rest.contains("rot:") && l.contains("rot: 90") || l.contains("rot: 270");
            v.push(if rot { vec2(h, w) } else { vec2(w, h) });
        }
    }
    // (Only the first page's line when pdfinfo gives one.)
    if v.is_empty() {
        let size = info.lines().find_map(|l| l.strip_prefix("Page size:")).and_then(|s| {
            let mut it = s.split_whitespace();
            Some(vec2(it.next()?.parse().ok()?, it.nth(1)?.parse().ok()?))
        });
        let n = info.lines().find_map(|l| l.strip_prefix("Pages:")).and_then(|n| n.trim().parse::<usize>().ok()).unwrap_or(0);
        if let Some(s) = size {
            v = vec![s; n.max(1)];
        }
    }
    v
}

fn render(file: &Path, page: usize, dpi: f32, dir: &Path) -> Result<egui::ColorImage, String> {
    let n = (page + 1).to_string();
    let prefix = dir.join(format!("page-{}-{n}", std::process::id()));
    let out = tool("pdftoppm").args(["-f", &n, "-l", &n, "-r", &format!("{dpi:.0}"), "-png", "-singlefile"]).arg(host(file)).arg(host(&prefix)).output().map_err(|e| e.to_string())?;
    let png = prefix.with_extension("png");
    let img = image::open(&png).map_err(|e| {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.is_empty() { e.to_string() } else { err }
    });
    let _ = std::fs::remove_file(&png);
    let img = img?.to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

/// The page's lines (pdftotext -bbox-layout), in points.
fn page_text(file: &Path, page: usize, dir: &Path) -> Vec<Line> {
    let n = (page + 1).to_string();
    let html = dir.join(format!("text-{}-{n}.html", std::process::id()));
    let _ = tool("pdftotext").args(["-f", &n, "-l", &n, "-bbox-layout"]).arg(host(file)).arg(host(&html)).output();
    let text = std::fs::read_to_string(&html).unwrap_or_default();
    let _ = std::fs::remove_file(&html);
    parse_bbox(&text)
}

/// pdftotext's lines: `<line xMin= yMin= xMax= yMax=><word …>text</word>…`.
pub fn parse_bbox(html: &str) -> Vec<Line> {
    // (Its doctype points outside; the rest reads as XML.)
    let body = html.find("<html").map_or(html, |i| &html[i..]);
    let Ok(doc) = roxmltree::Document::parse(body) else { return Vec::new() };
    let f = |n: &roxmltree::Node, a: &str| n.attribute(a).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    doc.descendants()
        .filter(|n| n.has_tag_name("line"))
        .filter_map(|n| {
            let words: Vec<&str> = n.children().filter(|w| w.has_tag_name("word")).filter_map(|w| w.text()).collect();
            (!words.is_empty()).then(|| Line { rect: Rect::from_min_max(pos2(f(&n, "xMin"), f(&n, "yMin")), pos2(f(&n, "xMax"), f(&n, "yMax"))), text: words.join(" ") })
        })
        .collect()
}

// ---------------------------------------------------------------- XML

/// A node's attribute by its local name (namespaces aside).
fn attr<'a>(n: &roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == name).map(|a| a.value())
}

fn is(n: &roxmltree::Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name
}

fn child<'a, 'i>(n: &roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| is(c, name))
}

fn kids<'a, 'i: 'a>(n: &roxmltree::Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> + 'a {
    n.children().filter(move |c| is(c, name))
}

fn find<'a, 'i>(n: &roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.descendants().find(|c| is(c, name))
}

/// "16.51cm", "6.5in", "25.4mm", "72pt", "96px" in points.
pub fn length(s: &str) -> Option<f32> {
    let s = s.trim();
    let at = s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))?;
    let (n, unit) = s.split_at(at);
    let n: f32 = n.parse().ok()?;
    Some(match unit {
        "cm" => n * 72.0 / 2.54,
        "mm" => n * 72.0 / 25.4,
        "in" => n * 72.0,
        "pt" => n,
        "px" => n * 0.75,
        "pc" => n * 12.0,
        _ => return None,
    })
}

fn hex(s: &str) -> Option<Color32> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

struct Zip(zip::ZipArchive<std::fs::File>);

impl Zip {
    fn open(path: &Path) -> Result<Zip, String> {
        let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        zip::ZipArchive::new(f).map(Zip).map_err(|e| e.to_string())
    }

    fn bytes(&mut self, name: &str) -> Option<Vec<u8>> {
        let mut f = self.0.by_name(name.trim_start_matches('/')).ok()?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        Some(v)
    }

    fn text(&mut self, name: &str) -> Option<String> {
        self.bytes(name).map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

/// A part's relations: id → the target's path in the zip.
fn rels(z: &mut Zip, part: &str) -> HashMap<String, String> {
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    let Some(xml) = z.text(&format!("{dir}/_rels/{file}.rels")) else { return HashMap::new() };
    let Ok(doc) = roxmltree::Document::parse(&xml) else { return HashMap::new() };
    doc.descendants()
        .filter(|n| is(n, "Relationship"))
        .filter_map(|n| Some((attr(&n, "Id")?.to_string(), resolve(dir, attr(&n, "Target")?))))
        .collect()
}

/// "../media/a.png" from "ppt/slides" → "ppt/media/a.png".
fn resolve(dir: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for p in target.split('/') {
        match p {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            p => parts.push(p),
        }
    }
    parts.join("/")
}

// ---------------------------------------------------------------- word processing

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// In points.
    pub size: Option<f32>,
    pub color: Option<Color32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Para {
        /// 0 a paragraph, 1… a heading's level.
        level: u8,
        /// In a list: its depth and number (none: a bullet).
        list: Option<(u8, Option<usize>)>,
        align: Align,
        runs: Vec<Run>,
    },
    /// Its columns' widths as the document gives them (none: even), and
    /// its rows of cells.
    Table { widths: Vec<f32>, rows: Vec<Vec<Cell>> },
    /// A picture (by index), its size in points.
    Image(usize, Vec2),
    Break,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    /// How many columns it spans.
    pub span: usize,
    /// The cell above goes on into it (merged down): no line between.
    pub merged: bool,
    pub fill: Option<Color32>,
    pub blocks: Vec<Block>,
}

pub struct Flow {
    pub blocks: Vec<Block>,
    pub images: Vec<Vec<u8>>,
    textures: HashMap<usize, Option<egui::TextureHandle>>,
    /// The zoom it's drawn at.
    scale: f32,
}

impl Flow {
    pub fn open(path: &Path) -> Result<Flow, String> {
        let mut z = Zip::open(path)?;
        let (blocks, images) = if z.0.by_name("content.xml").is_ok() { odt(&mut z)? } else { docx(&mut z)? };
        Ok(Flow { blocks, images, textures: HashMap::new(), scale: 1.0 })
    }

    fn draw(&mut self, ui: &mut Ui, width: f32, avail: f32, zoom: f32) {
        self.scale = zoom;
        let (mx, my) = ((44.0 * zoom).min(127.0), (40.0 * zoom).min(127.0));
        ui.add_space(GAP);
        ui.horizontal(|ui| {
            ui.add_space(((avail - width) / 2.0).max(GAP));
            egui::Frame::new().fill(PAPER).corner_radius(2.0).inner_margin(egui::Margin::symmetric(mx as i8, my as i8)).show(ui, |ui| {
                // (Top down inside, though the row it's centred in isn't.)
                ui.vertical(|ui| {
                    let inner = width - 2.0 * mx.round();
                    ui.set_width(inner);
                    ui.spacing_mut().item_spacing.y = 6.0 * zoom;
                    let blocks = std::mem::take(&mut self.blocks);
                    for b in &blocks {
                        self.block(ui, b, inner);
                    }
                    self.blocks = blocks;
                });
            });
        });
        ui.add_space(GAP);
    }

    fn block(&mut self, ui: &mut Ui, b: &Block, width: f32) {
        match b {
            Block::Para { level, list, align, runs } => {
                let z = self.scale;
                let base = z * match level {
                    0 => 15.0,
                    1 => 26.0,
                    2 => 21.0,
                    3 => 18.0,
                    _ => 16.0,
                };
                let indent = list.map_or(0.0, |(d, _)| 22.0 * z * (d as f32 + 1.0));
                let mut job = LayoutJob::default();
                if let Some((_, n)) = list {
                    let mark = n.map_or("•  ".to_string(), |n| format!("{n}.  "));
                    job.append(&mark, 0.0, TextFormat { font_id: FontId::proportional(base), color: INK, ..Default::default() });
                }
                for r in runs {
                    job.append(&r.text, 0.0, run_format(r, base, *level > 0, z));
                }
                if job.text.is_empty() {
                    ui.add_space(base * 0.6);
                    return;
                }
                job.wrap.max_width = width - indent;
                job.halign = match align {
                    Align::Center => egui::Align::Center,
                    Align::Right => egui::Align::RIGHT,
                    _ => egui::Align::LEFT,
                };
                if *level > 0 {
                    ui.add_space(base * 0.35);
                }
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    let w = width - indent;
                    let layout = match align {
                        Align::Center => egui::Layout::top_down(egui::Align::Center),
                        Align::Right => egui::Layout::top_down(egui::Align::RIGHT),
                        _ => egui::Layout::top_down(egui::Align::LEFT),
                    };
                    ui.allocate_ui_with_layout(vec2(w, 0.0), layout, |ui| {
                        ui.set_width(w);
                        ui.add(egui::Label::new(job).wrap());
                    });
                });
            }
            Block::Table { widths, rows } => self.table(ui, widths, rows, width),
            Block::Image(i, size) => {
                let tex = self.textures.entry(*i).or_insert_with(|| decode(ui.ctx(), &format!("doc-img-{i}"), &self.images[*i])).clone();
                let Some(t) = tex else { return };
                let s = self.scale * if size.x > 0.0 { *size * (4.0 / 3.0) } else { t.size_vec2() };
                let s = if s.x > width { s * (width / s.x) } else { s };
                ui.add(egui::Image::new(egui::load::SizedTexture::new(t.id(), s)));
            }
            Block::Break => {
                ui.add_space(8.0 * self.scale);
                let (r, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
                ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, Color32::from_gray(210)));
                ui.add_space(8.0 * self.scale);
            }
        }
    }
}

impl Flow {
    /// A table laid out by hand: each row as tall as its tallest cell,
    /// its lines drawn whole over the cells.
    fn table(&mut self, ui: &mut Ui, widths: &[f32], rows: &[Vec<Cell>], width: f32) {
        let z = self.scale;
        let pad = 6.0 * z;
        let cols = rows.iter().map(|r| r.iter().map(|c| c.span.max(1)).sum::<usize>()).max().unwrap_or(1).max(1);
        let given: f32 = widths.iter().sum();
        let left = ui.cursor().left().round();
        let mut xs = vec![left];
        for i in 0..cols {
            let w = if widths.len() == cols && given > 0.0 { widths[i] / given * width } else { width / cols as f32 };
            xs.push(xs[i] + w);
        }
        for x in &mut xs {
            *x = x.round();
        }
        let stroke = Stroke::new(1.0, Color32::from_gray(150));
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let top0 = ui.cursor().top();
            for row in rows {
                let top = ui.cursor().top();
                let mut cells = Vec::new();
                let mut h: f32 = 14.0 * z;
                let mut col = 0;
                for cell in row {
                    if col >= cols {
                        break;
                    }
                    let end = (col + cell.span.max(1)).min(cols);
                    let (x0, x1) = (xs[col], xs[end]);
                    col = end;
                    // (The fill goes under the text, once the height is known.)
                    let under = ui.painter().add(egui::Shape::Noop);
                    let inner = Rect::from_min_size(pos2(x0 + pad, top + pad), vec2((x1 - x0 - 2.0 * pad).max(1.0), f32::INFINITY));
                    let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::LEFT)));
                    cu.spacing_mut().item_spacing.y = 4.0 * z;
                    // (A line's no taller than its text.)
                    cu.spacing_mut().interact_size.y = 0.0;
                    cu.set_width(inner.width());
                    for b in &cell.blocks {
                        self.block(&mut cu, b, inner.width());
                    }
                    if !cell.blocks.is_empty() {
                        h = h.max(cu.min_rect().height());
                    }
                    cells.push((x0, x1, under, cell));
                }
                let bottom = top + h + 2.0 * pad;
                ui.allocate_exact_size(vec2(xs[cols] - left, bottom - top), Sense::hover());
                let p = ui.painter();
                for (x0, x1, under, cell) in cells {
                    if let Some(f) = cell.fill.filter(|f| *f != PAPER) {
                        p.set(under, egui::Shape::rect_filled(Rect::from_min_max(pos2(x0, top), pos2(x1, bottom)), 0.0, f));
                    }
                    if !cell.merged {
                        p.hline(x0..=x1, top, stroke);
                    }
                    p.vline(x0, top..=bottom, stroke);
                }
                p.vline(xs[cols], top..=bottom, stroke);
            }
            let end = ui.cursor().top();
            if end > top0 {
                ui.painter().hline(xs[0]..=xs[cols], end, stroke);
            }
        });
    }
}

/// A run's look: its size (points as pixels, by `scale`), weight, slant,
/// lines, colour.
fn run_format(r: &Run, base: f32, heading: bool, scale: f32) -> TextFormat {
    let size = r.size.map_or(base, |s| (s * 4.0 / 3.0 * scale).clamp(6.0, 120.0));
    let family = if r.bold || heading { FontFamily::Name("bold".into()) } else { FontFamily::Proportional };
    let color = r.color.unwrap_or(INK);
    TextFormat {
        font_id: FontId::new(size, family),
        color,
        italics: r.italic,
        underline: if r.underline { Stroke::new(1.0, color) } else { Stroke::NONE },
        strikethrough: if r.strike { Stroke::new(1.0, color) } else { Stroke::NONE },
        ..Default::default()
    }
}

/// A picture's bytes as a texture (none: unreadable).
fn decode(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?;
    // (Big photos: no bigger than the screen needs.)
    let img = if img.width() > 2400 { img.resize(2400, 2400 * img.height() / img.width().max(1), image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    Some(ctx.load_texture(name, egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()), egui::TextureOptions::LINEAR))
}

// .docx

struct DocxCtx {
    rels: HashMap<String, String>,
    /// Style id → heading level (Title: 1).
    headings: HashMap<String, u8>,
    /// numId → (level → numbered).
    numbered: HashMap<String, HashMap<String, bool>>,
    counters: HashMap<(String, String), usize>,
    images: Vec<Vec<u8>>,
    media: HashMap<String, usize>,
}

fn docx(z: &mut Zip) -> Result<(Vec<Block>, Vec<Vec<u8>>), String> {
    let xml = z.text("word/document.xml").ok_or("no word/document.xml")?;
    let doc = roxmltree::Document::parse(&xml).map_err(|e| e.to_string())?;
    let mut c = DocxCtx { rels: rels(z, "word/document.xml"), headings: HashMap::new(), numbered: HashMap::new(), counters: HashMap::new(), images: Vec::new(), media: HashMap::new() };
    if let Some(s) = z.text("word/styles.xml") {
        if let Ok(st) = roxmltree::Document::parse(&s) {
            for n in st.descendants().filter(|n| is(n, "style")) {
                let (Some(id), Some(name)) = (attr(&n, "styleId"), child(&n, "name").and_then(|x| attr(&x, "val"))) else { continue };
                let name = name.to_lowercase();
                let level = if name == "title" {
                    Some(1)
                } else if name == "subtitle" {
                    Some(3)
                } else {
                    name.strip_prefix("heading ").and_then(|l| l.trim().parse::<u8>().ok())
                };
                if let Some(l) = level {
                    c.headings.insert(id.to_string(), l);
                }
            }
        }
    }
    if let Some(s) = z.text("word/numbering.xml") {
        if let Ok(nd) = roxmltree::Document::parse(&s) {
            let mut abs: HashMap<String, HashMap<String, bool>> = HashMap::new();
            for a in nd.descendants().filter(|n| is(n, "abstractNum")) {
                let levels = kids(&a, "lvl").filter_map(|l| Some((attr(&l, "ilvl")?.to_string(), child(&l, "numFmt").and_then(|f| attr(&f, "val")).is_some_and(|f| f != "bullet" && f != "none")))).collect();
                if let Some(id) = attr(&a, "abstractNumId") {
                    abs.insert(id.to_string(), levels);
                }
            }
            for n in nd.descendants().filter(|n| is(n, "num")) {
                if let (Some(id), Some(a)) = (attr(&n, "numId"), child(&n, "abstractNumId").and_then(|x| attr(&x, "val"))) {
                    c.numbered.insert(id.to_string(), abs.get(a).cloned().unwrap_or_default());
                }
            }
        }
    }
    let body = find(&doc.root(), "body").ok_or("no body")?;
    let mut out = Vec::new();
    docx_blocks(z, &body, &mut c, &mut out);
    Ok((out, c.images))
}

fn docx_blocks(z: &mut Zip, n: &roxmltree::Node, c: &mut DocxCtx, out: &mut Vec<Block>) {
    for k in n.children().filter(|k| k.is_element()) {
        match k.tag_name().name() {
            "p" => docx_para(z, &k, c, out),
            "tbl" => {
                let widths = child(&k, "tblGrid").map(|g| kids(&g, "gridCol").filter_map(|c| attr(&c, "w")?.parse::<f32>().ok()).collect()).unwrap_or_default();
                let rows = kids(&k, "tr")
                    .map(|tr| {
                        kids(&tr, "tc")
                            .map(|tc| {
                                let pr = child(&tc, "tcPr");
                                let span = pr.and_then(|p| child(&p, "gridSpan")).and_then(|g| attr(&g, "val")?.parse().ok()).unwrap_or(1);
                                let merged = pr.and_then(|p| child(&p, "vMerge")).is_some_and(|m| attr(&m, "val") != Some("restart"));
                                let fill = pr.and_then(|p| child(&p, "shd")).and_then(|d| hex(attr(&d, "fill")?));
                                let mut blocks = Vec::new();
                                if !merged {
                                    docx_blocks(z, &tc, c, &mut blocks);
                                }
                                Cell { span, merged, fill, blocks }
                            })
                            .collect()
                    })
                    .collect();
                out.push(Block::Table { widths, rows });
            }
            "sdt" | "sdtContent" | "customXml" | "smartTag" => docx_blocks(z, &k, c, out),
            _ => {}
        }
    }
}

fn docx_para(z: &mut Zip, p: &roxmltree::Node, c: &mut DocxCtx, out: &mut Vec<Block>) {
    let ppr = child(p, "pPr");
    let style = ppr.and_then(|x| child(&x, "pStyle")).and_then(|s| attr(&s, "val")).unwrap_or("");
    let level = ppr.and_then(|x| child(&x, "outlineLvl")).and_then(|o| attr(&o, "val")?.parse::<u8>().ok()).map(|l| l + 1).or_else(|| c.headings.get(style).copied()).unwrap_or(0);
    let align = match ppr.and_then(|x| child(&x, "jc")).and_then(|j| attr(&j, "val")) {
        Some("center") => Align::Center,
        Some("right" | "end") => Align::Right,
        Some("both" | "distribute") => Align::Justify,
        _ => Align::Left,
    };
    let list = ppr.and_then(|x| child(&x, "numPr")).and_then(|np| {
        let id = child(&np, "numId").and_then(|x| attr(&x, "val"))?.to_string();
        let lvl = child(&np, "ilvl").and_then(|x| attr(&x, "val")).unwrap_or("0").to_string();
        if id == "0" {
            return None;
        }
        let numbered = c.numbered.get(&id).and_then(|m| m.get(&lvl)).copied().unwrap_or(false);
        let depth = lvl.parse::<u8>().unwrap_or(0);
        let n = numbered.then(|| {
            // (A lower level starting over under a higher one.)
            c.counters.retain(|(i, l), _| *i != id || l.parse::<u8>().unwrap_or(0) <= depth);
            let k = c.counters.entry((id.clone(), lvl.clone())).or_insert(0);
            *k += 1;
            *k
        });
        Some((depth, n))
    });
    let mut runs = Vec::new();
    let mut pictures = Vec::new();
    docx_runs(z, p, c, &mut runs, &mut pictures, false);
    let has_text = runs.iter().any(|r| !r.text.trim().is_empty());
    if has_text || pictures.is_empty() {
        out.push(Block::Para { level, list, align, runs });
    }
    out.extend(pictures);
}

fn docx_runs(z: &mut Zip, n: &roxmltree::Node, c: &mut DocxCtx, runs: &mut Vec<Run>, pictures: &mut Vec<Block>, link: bool) {
    for k in n.children().filter(|k| k.is_element()) {
        match k.tag_name().name() {
            "r" => {
                let rpr = child(&k, "rPr");
                let on = |name: &str| rpr.and_then(|r| child(&r, name)).is_some_and(|b| !matches!(attr(&b, "val"), Some("0" | "false" | "none")));
                let mut run = Run {
                    bold: on("b"),
                    italic: on("i"),
                    underline: on("u") || link,
                    strike: on("strike") || on("dstrike"),
                    size: rpr.and_then(|r| child(&r, "sz")).and_then(|s| attr(&s, "val")?.parse::<f32>().ok()).map(|h| h / 2.0),
                    color: rpr.and_then(|r| child(&r, "color")).and_then(|s| hex(attr(&s, "val")?)).or(link.then_some(Color32::from_rgb(30, 90, 200))),
                    ..Default::default()
                };
                for t in k.children().filter(|t| t.is_element()) {
                    match t.tag_name().name() {
                        "t" => run.text += t.text().unwrap_or(""),
                        "tab" => run.text.push('\t'),
                        "br" if attr(&t, "type") == Some("page") => pictures.push(Block::Break),
                        "br" | "cr" => run.text.push('\n'),
                        "noBreakHyphen" => run.text.push('-'),
                        "drawing" | "pict" | "object" => {
                            let id = t.descendants().find_map(|d| if is(&d, "blip") { attr(&d, "embed") } else if is(&d, "imagedata") { attr(&d, "id") } else { None });
                            let ext = find(&t, "extent").map(|e| vec2(attr(&e, "cx").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) / 12700.0, attr(&e, "cy").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) / 12700.0));
                            if let Some(i) = id.and_then(|id| c.rels.get(id).cloned()).and_then(|path| media(z, &mut c.images, &mut c.media, &path)) {
                                pictures.push(Block::Image(i, ext.unwrap_or(Vec2::ZERO)));
                            }
                        }
                        _ => {}
                    }
                }
                if !run.text.is_empty() {
                    runs.push(run);
                }
            }
            "hyperlink" => docx_runs(z, &k, c, runs, pictures, true),
            "fldSimple" | "smartTag" | "ins" | "sdt" | "sdtContent" | "customXml" => docx_runs(z, &k, c, runs, pictures, link),
            _ => {}
        }
    }
}

/// A picture in the zip, read once.
fn media(z: &mut Zip, images: &mut Vec<Vec<u8>>, seen: &mut HashMap<String, usize>, path: &str) -> Option<usize> {
    if let Some(i) = seen.get(path) {
        return Some(*i);
    }
    let b = z.bytes(path)?;
    images.push(b);
    seen.insert(path.to_string(), images.len() - 1);
    Some(images.len() - 1)
}

// .odt (and the text in .odp)

#[derive(Clone, Debug, Default)]
struct OdStyle {
    parent: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    size: Option<f32>,
    color: Option<Color32>,
    align: Option<Align>,
    heading: Option<u8>,
    /// A list style's numbering, by level (1…).
    numbered: HashMap<u8, bool>,
    fill: Option<Color32>,
}

/// The styles of content.xml and styles.xml, by name.
fn od_styles(docs: &[&roxmltree::Document]) -> HashMap<String, OdStyle> {
    let mut m = HashMap::new();
    for d in docs {
        for n in d.descendants().filter(|n| is(n, "style") || is(n, "list-style") || is(n, "default-style")) {
            let Some(name) = attr(&n, "name") else { continue };
            let mut s = OdStyle { parent: attr(&n, "parent-style-name").map(String::from), ..Default::default() };
            if let Some(t) = child(&n, "text-properties") {
                s.bold = attr(&t, "font-weight").map(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600));
                s.italic = attr(&t, "font-style").map(|v| v == "italic" || v == "oblique");
                s.underline = attr(&t, "text-underline-style").map(|v| v != "none");
                s.strike = attr(&t, "text-line-through-style").map(|v| v != "none");
                s.size = attr(&t, "font-size").and_then(length);
                s.color = attr(&t, "color").and_then(hex);
            }
            if let Some(p) = child(&n, "paragraph-properties") {
                s.align = attr(&p, "text-align").map(|a| match a {
                    "center" => Align::Center,
                    "end" | "right" => Align::Right,
                    "justify" => Align::Justify,
                    _ => Align::Left,
                });
            }
            if let Some(g) = child(&n, "graphic-properties").or_else(|| child(&n, "drawing-page-properties")) {
                if attr(&g, "fill") != Some("none") {
                    s.fill = attr(&g, "fill-color").and_then(hex);
                }
            }
            let lname = name.to_lowercase().replace("_20_", " ");
            if lname == "title" {
                s.heading = Some(1);
            } else if let Some(l) = lname.strip_prefix("heading ") {
                s.heading = l.trim().parse().ok();
            }
            for lv in n.children().filter(|c| is(c, "list-level-style-number") || is(c, "list-level-style-bullet")) {
                if let Some(l) = attr(&lv, "level").and_then(|l| l.parse().ok()) {
                    s.numbered.insert(l, is(&lv, "list-level-style-number"));
                }
            }
            m.insert(name.to_string(), s);
        }
    }
    m
}

/// A style with its parents' gaps filled.
fn od_style(m: &HashMap<String, OdStyle>, name: Option<&str>) -> OdStyle {
    let mut s = OdStyle::default();
    let mut at = name.map(String::from);
    for _ in 0..8 {
        let Some(n) = at.take() else { break };
        let Some(x) = m.get(&n) else { break };
        s.bold = s.bold.or(x.bold);
        s.italic = s.italic.or(x.italic);
        s.underline = s.underline.or(x.underline);
        s.strike = s.strike.or(x.strike);
        s.size = s.size.or(x.size);
        s.color = s.color.or(x.color);
        s.align = s.align.or(x.align);
        s.heading = s.heading.or(x.heading);
        s.fill = s.fill.or(x.fill);
        if s.numbered.is_empty() {
            s.numbered = x.numbered.clone();
        }
        at = x.parent.clone();
    }
    s
}

struct OdCtx<'z> {
    z: &'z mut Zip,
    styles: HashMap<String, OdStyle>,
    images: Vec<Vec<u8>>,
    media: HashMap<String, usize>,
}

fn odt(z: &mut Zip) -> Result<(Vec<Block>, Vec<Vec<u8>>), String> {
    let content = z.text("content.xml").ok_or("no content.xml")?;
    let styles = z.text("styles.xml").unwrap_or_default();
    let doc = roxmltree::Document::parse(&content).map_err(|e| e.to_string())?;
    let sdoc = roxmltree::Document::parse(&styles).ok();
    let mut all = vec![&doc];
    if let Some(s) = &sdoc {
        all.push(s);
    }
    let mut c = OdCtx { z, styles: od_styles(&all), images: Vec::new(), media: HashMap::new() };
    let text = find(&doc.root(), "text").filter(|t| t.parent().is_some_and(|p| is(&p, "body"))).ok_or("no office:text")?;
    let mut out = Vec::new();
    od_blocks(&mut c, &text, &mut out, None, &mut Vec::new());
    Ok((out, c.images))
}

/// Blocks of an odt body (or a cell, a list item): `list` is the list
/// style and depth around, `counters` its numbers by depth.
fn od_blocks(c: &mut OdCtx, n: &roxmltree::Node, out: &mut Vec<Block>, list: Option<(&str, u8)>, counters: &mut Vec<usize>) {
    for k in n.children().filter(|k| k.is_element()) {
        match k.tag_name().name() {
            "p" | "h" => {
                let st = od_style(&c.styles, attr(&k, "style-name"));
                let level = if is(&k, "h") { attr(&k, "outline-level").and_then(|l| l.parse().ok()).unwrap_or(1) } else { st.heading.unwrap_or(0) };
                let mut runs = Vec::new();
                let mut pictures = Vec::new();
                od_runs(c, &k, &st, &mut runs, &mut pictures);
                let list_mark = list.map(|(ls, depth)| {
                    let numbered = od_style(&c.styles, Some(ls)).numbered.get(&(depth + 1)).copied().unwrap_or(false);
                    (depth, numbered.then(|| counters.get(depth as usize).copied().unwrap_or(1)))
                });
                if runs.iter().any(|r| !r.text.trim().is_empty()) || pictures.is_empty() {
                    out.push(Block::Para { level, list: list_mark, align: st.align.unwrap_or_default(), runs });
                }
                out.extend(pictures);
            }
            "list" => {
                let style = attr(&k, "style-name").or(list.map(|l| l.0)).unwrap_or("");
                let depth = list.map_or(0, |l| l.1 + 1);
                let mut items = 0;
                for item in kids(&k, "list-item").chain(kids(&k, "list-header")) {
                    items += 1;
                    if counters.len() <= depth as usize {
                        counters.resize(depth as usize + 1, 0);
                    }
                    counters[depth as usize] = items;
                    counters.truncate(depth as usize + 1);
                    od_blocks(c, &item, out, Some((style, depth)), counters);
                }
            }
            "table" => {
                let mut rows = Vec::new();
                od_rows(c, &k, &mut rows);
                out.push(Block::Table { widths: Vec::new(), rows });
            }
            "section" | "index-body" | "table-of-content" | "alphabetical-index" | "illustration-index" => od_blocks(c, &k, out, list, counters),
            _ => {}
        }
    }
}

/// A table's rows, the header's and grouped ones too, not a table's inside.
fn od_rows(c: &mut OdCtx, n: &roxmltree::Node, rows: &mut Vec<Vec<Cell>>) {
    for k in n.children().filter(|k| k.is_element()) {
        match k.tag_name().name() {
            "table-row" => {
                let mut row = Vec::new();
                let mut skip = 0;
                for cell in k.children().filter(|x| is(x, "table-cell") || is(x, "covered-table-cell")) {
                    // (Under one spanning across.)
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    if is(&cell, "covered-table-cell") {
                        row.push(Cell { span: 1, merged: true, ..Default::default() });
                        continue;
                    }
                    let span = attr(&cell, "number-columns-spanned").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).max(1);
                    skip = span - 1;
                    let mut blocks = Vec::new();
                    od_blocks(c, &cell, &mut blocks, None, &mut Vec::new());
                    row.push(Cell { span, merged: false, fill: None, blocks });
                }
                rows.push(row);
            }
            "table-header-rows" | "table-rows" | "table-row-group" => od_rows(c, &k, rows),
            _ => {}
        }
    }
}

fn od_runs(c: &mut OdCtx, n: &roxmltree::Node, st: &OdStyle, runs: &mut Vec<Run>, pictures: &mut Vec<Block>) {
    let run = |text: String, s: &OdStyle| Run {
        text,
        bold: s.bold.unwrap_or(false),
        italic: s.italic.unwrap_or(false),
        underline: s.underline.unwrap_or(false),
        strike: s.strike.unwrap_or(false),
        size: s.size,
        color: s.color,
    };
    for k in n.children() {
        if k.is_text() {
            // (Runs of spaces count as one, as in XML.)
            let t = k.text().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
            let lead = k.text().is_some_and(|t| t.starts_with(char::is_whitespace)) && !t.is_empty();
            let trail = k.text().is_some_and(|t| t.ends_with(char::is_whitespace)) && !t.is_empty();
            let t = format!("{}{t}{}", if lead { " " } else { "" }, if trail { " " } else { "" });
            if !t.is_empty() {
                runs.push(run(t, st));
            }
            continue;
        }
        if !k.is_element() {
            continue;
        }
        match k.tag_name().name() {
            "span" => {
                let mut s = od_style(&c.styles, attr(&k, "style-name"));
                s.bold = s.bold.or(st.bold);
                s.italic = s.italic.or(st.italic);
                s.underline = s.underline.or(st.underline);
                s.strike = s.strike.or(st.strike);
                s.size = s.size.or(st.size);
                s.color = s.color.or(st.color);
                od_runs(c, &k, &s, runs, pictures);
            }
            "a" => {
                let mut s = st.clone();
                s.underline = Some(true);
                s.color = Some(Color32::from_rgb(30, 90, 200));
                od_runs(c, &k, &s, runs, pictures);
            }
            "s" => runs.push(run(" ".repeat(attr(&k, "c").and_then(|n| n.parse().ok()).unwrap_or(1)), st)),
            "tab" => runs.push(run("\t".into(), st)),
            "line-break" => runs.push(run("\n".into(), st)),
            "frame" => {
                let size = vec2(attr(&k, "width").and_then(length).unwrap_or(0.0), attr(&k, "height").and_then(length).unwrap_or(0.0));
                if let Some(href) = find(&k, "image").and_then(|i| attr(&i, "href")) {
                    if let Some(i) = media(c.z, &mut c.images, &mut c.media, href) {
                        pictures.push(Block::Image(i, size));
                    }
                } else if let Some(tb) = child(&k, "text-box") {
                    let mut b = Vec::new();
                    od_blocks(c, &tb, &mut b, None, &mut Vec::new());
                    pictures.extend(b);
                }
            }
            "note" | "annotation" | "bookmark" | "bookmark-start" | "bookmark-end" | "soft-page-break" => {}
            _ => od_runs(c, &k, st, runs, pictures),
        }
    }
}

// ---------------------------------------------------------------- presentations

#[derive(Clone, Debug, PartialEq)]
pub struct SPara {
    pub runs: Vec<Run>,
    pub align: Align,
    /// "• " or "1. ", and how deep.
    pub bullet: Option<String>,
    pub level: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ShapeKind {
    /// Its paragraphs, centred up and down (a title) or from the top.
    Text { paras: Vec<SPara>, middle: bool },
    Image(usize),
    Table(Vec<Vec<String>>),
    Box,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// In points from the slide's top left.
    pub rect: Rect,
    pub fill: Option<Color32>,
    pub kind: ShapeKind,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Slide {
    pub bg: Option<Color32>,
    pub shapes: Vec<Shape>,
}

pub struct Slides {
    /// In points.
    pub size: Vec2,
    pub slides: Vec<Slide>,
    pub images: Vec<Vec<u8>>,
    textures: HashMap<usize, Option<egui::TextureHandle>>,
}

impl Slides {
    pub fn open(path: &Path) -> Result<Slides, String> {
        let mut z = Zip::open(path)?;
        let (size, slides, images) = if z.0.by_name("content.xml").is_ok() { odp(&mut z)? } else { pptx(&mut z)? };
        if slides.is_empty() {
            return Err(tr("No slides").to_string());
        }
        Ok(Slides { size, slides, images, textures: HashMap::new() })
    }

    /// Each slide, the view's width, under the other.
    fn draw(&mut self, ui: &mut Ui, width: f32, avail: f32, tops: &mut Vec<f32>) {
        let top0 = ui.min_rect().top();
        ui.add_space(GAP);
        let k = width / self.size.x.max(1.0);
        let h = (self.size.y * k).round();
        let x = ((avail - width) / 2.0).max(GAP);
        for i in 0..self.slides.len() {
            let (row, _) = ui.allocate_exact_size(vec2((width + 2.0 * x).max(avail), h + GAP), Sense::hover());
            tops.push(row.top() - top0 - GAP);
            let card = Rect::from_min_size(pos2(row.left() + x, row.top()), vec2(width, h));
            if !ui.is_rect_visible(card) {
                continue;
            }
            let slide = self.slides[i].clone();
            ui.painter().rect_filled(card.translate(vec2(0.0, 2.0)), 3.0, Color32::from_black_alpha(60));
            ui.painter().rect_filled(card, 3.0, slide.bg.unwrap_or(PAPER));
            let clip = ui.clip_rect().intersect(card);
            // (Boxes put on a child of the card: the slides' layout stays.)
            let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(card));
            for s in &slide.shapes {
                let r = Rect::from_min_max(card.min + s.rect.min.to_vec2() * k, card.min + s.rect.max.to_vec2() * k);
                if let Some(f) = s.fill {
                    ui.painter().with_clip_rect(clip).rect_filled(r, 0.0, f);
                }
                match &s.kind {
                    ShapeKind::Image(idx) => {
                        let tex = self.textures.entry(*idx).or_insert_with(|| decode(ui.ctx(), &format!("slide-img-{idx}"), &self.images[*idx])).clone();
                        if let Some(t) = tex {
                            egui::Image::new(egui::load::SizedTexture::new(t.id(), r.size())).paint_at(&mut ui.new_child(egui::UiBuilder::new().max_rect(clip)), r);
                        }
                    }
                    ShapeKind::Text { paras, middle } => slide_text(ui, r, paras, *middle, k, clip),
                    ShapeKind::Table(rows) => {
                        let n = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
                        let rh = r.height() / rows.len().max(1) as f32;
                        let cw = r.width() / n as f32;
                        let p = ui.painter().with_clip_rect(clip);
                        for (ri, row) in rows.iter().enumerate() {
                            for (ci, cell) in row.iter().enumerate() {
                                let cr = Rect::from_min_size(pos2(r.min.x + ci as f32 * cw, r.min.y + ri as f32 * rh), vec2(cw, rh));
                                p.rect_stroke(cr, 0.0, Stroke::new(1.0, Color32::from_gray(150)), egui::StrokeKind::Inside);
                                let mut job = LayoutJob::single_section(cell.clone(), TextFormat { font_id: FontId::proportional((14.0 * k).max(5.0)), color: INK, ..Default::default() });
                                job.wrap.max_width = (cw - 6.0).max(4.0);
                                ui.put(cr.shrink(3.0).intersect(clip), egui::Label::new(job).wrap());
                            }
                        }
                    }
                    ShapeKind::Box => {}
                }
            }
        }
    }
}

/// A box's paragraphs inside it (k: pixels a point).
fn slide_text(ui: &mut Ui, r: Rect, paras: &[SPara], middle: bool, k: f32, clip: Rect) {
    let inset = 7.2 * k;
    let w = (r.width() - 2.0 * inset).max(4.0);
    let mut jobs = Vec::new();
    for p in paras {
        let indent = p.level as f32 * 24.0 * k;
        let mut job = LayoutJob::default();
        let first = p.runs.first().map(|r| r.size.unwrap_or(18.0)).unwrap_or(18.0);
        if let Some(b) = &p.bullet {
            job.append(b, 0.0, TextFormat { font_id: FontId::proportional((first * k * 4.0 / 3.0).max(4.0)), color: p.runs.first().and_then(|r| r.color).unwrap_or(INK), ..Default::default() });
        }
        for run in &p.runs {
            let mut f = run_format(run, (18.0 * k * 4.0 / 3.0).max(4.0), false, k);
            f.font_id.size = (run.size.unwrap_or(18.0) * k * 4.0 / 3.0).max(4.0);
            job.append(&run.text, 0.0, f);
        }
        if job.text.is_empty() {
            job.append(" ", 0.0, TextFormat { font_id: FontId::proportional((first * k * 4.0 / 3.0).max(4.0)), color: Color32::TRANSPARENT, ..Default::default() });
        }
        job.wrap.max_width = (w - indent).max(4.0);
        job.halign = match p.align {
            Align::Center => egui::Align::Center,
            Align::Right => egui::Align::RIGHT,
            _ => egui::Align::LEFT,
        };
        let g = ui.painter().layout_job(job.clone());
        jobs.push((job, g.size(), indent, p.align));
    }
    let total: f32 = jobs.iter().map(|j| j.1.y).sum();
    let mut y = if middle { r.center().y - total / 2.0 } else { r.min.y + inset };
    for (job, size, indent, align) in jobs {
        let x = match align {
            Align::Center => r.center().x - size.x / 2.0,
            Align::Right => r.max.x - inset - size.x,
            _ => r.min.x + inset + indent,
        };
        let at = Rect::from_min_size(pos2(x, y), vec2(size.x.max(1.0), size.y));
        if at.intersects(clip) {
            ui.put(at, egui::Label::new(job).wrap());
        }
        y += size.y;
    }
}

const EMU: f32 = 12700.0;

fn emu(n: &roxmltree::Node, a: &str) -> f32 {
    attr(n, a).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) / EMU
}

/// A shape's place (a:xfrm), in points.
fn xfrm(n: &roxmltree::Node) -> Option<Rect> {
    let x = n.children().find(|c| is(c, "spPr") || is(c, "grpSpPr") || is(c, "xfrm")).and_then(|s| if is(&s, "xfrm") { Some(s) } else { child(&s, "xfrm") })?;
    let off = child(&x, "off")?;
    let ext = child(&x, "ext")?;
    Some(Rect::from_min_size(pos2(emu(&off, "x"), emu(&off, "y")), vec2(emu(&ext, "cx"), emu(&ext, "cy"))))
}

/// A placeholder's kind and number (p:nvPr/p:ph).
fn ph(n: &roxmltree::Node) -> Option<(String, String)> {
    let p = n.descendants().find(|d| is(d, "ph"))?;
    Some((attr(&p, "type").unwrap_or("body").to_string(), attr(&p, "idx").unwrap_or("").to_string()))
}

fn srgb(n: &roxmltree::Node) -> Option<Color32> {
    child(n, "solidFill").and_then(|f| child(&f, "srgbClr")).and_then(|c| hex(attr(&c, "val")?))
}

/// A layout's (or master's) placeholders: (type, idx) → place.
fn ph_places(z: &mut Zip, part: &str) -> (Vec<((String, String), Rect)>, Option<String>, Option<Color32>) {
    let Some(xml) = z.text(part) else { return (Vec::new(), None, None) };
    let Ok(doc) = roxmltree::Document::parse(&xml) else { return (Vec::new(), None, None) };
    let places = doc.descendants().filter(|n| is(n, "sp")).filter_map(|n| Some((ph(&n)?, xfrm(&n)?))).collect();
    let bg = find(&doc.root(), "bgPr").and_then(|b| srgb(&b));
    let up = rels(z, part).into_values().find(|t| t.contains("slideLayout") || t.contains("slideMaster"));
    (places, up, bg)
}

type Deck = (Vec2, Vec<Slide>, Vec<Vec<u8>>);

fn pptx(z: &mut Zip) -> Result<Deck, String> {
    let pres = z.text("ppt/presentation.xml").ok_or("no ppt/presentation.xml")?;
    let doc = roxmltree::Document::parse(&pres).map_err(|e| e.to_string())?;
    let size = find(&doc.root(), "sldSz").map(|s| vec2(emu(&s, "cx"), emu(&s, "cy"))).unwrap_or(vec2(720.0, 405.0));
    let prels = rels(z, "ppt/presentation.xml");
    // (r:id, not the slide's own id beside it.)
    let order: Vec<String> = doc.descendants().filter(|n| is(n, "sldId")).filter_map(|n| prels.get(n.attributes().find(|a| a.name() == "id" && a.namespace().is_some())?.value()).cloned()).collect();
    let (mut images, mut seen) = (Vec::new(), HashMap::new());
    let mut slides = Vec::new();
    for part in order {
        let Some(xml) = z.text(&part) else { continue };
        let Ok(sd) = roxmltree::Document::parse(&xml) else { continue };
        let srels = rels(z, &part);
        // The layout's places, then the master's, for placeholders without one.
        let mut places = Vec::new();
        let mut bg = find(&sd.root(), "bgPr").and_then(|b| srgb(&b));
        let mut up = srels.values().find(|t| t.contains("slideLayout")).cloned();
        for _ in 0..2 {
            let Some(p) = up.take() else { break };
            let (pl, next, b) = ph_places(z, &p);
            places.extend(pl);
            bg = bg.or(b);
            up = next.filter(|n| n.contains("slideMaster"));
        }
        let mut shapes = Vec::new();
        if let Some(tree) = find(&sd.root(), "spTree") {
            pptx_shapes(z, &tree, &srels, &places, &mut images, &mut seen, &mut shapes, None);
        }
        slides.push(Slide { bg, shapes });
    }
    Ok((size, slides, images))
}

/// A tree's shapes; a group's through its own coordinates (from its child
/// space `ch` to where it stands).
#[allow(clippy::too_many_arguments)]
fn pptx_shapes(z: &mut Zip, tree: &roxmltree::Node, srels: &HashMap<String, String>, places: &[((String, String), Rect)], images: &mut Vec<Vec<u8>>, seen: &mut HashMap<String, usize>, out: &mut Vec<Shape>, map: Option<(Rect, Rect)>) {
    let place = |r: Rect| match map {
        Some((ch, to)) => {
            let sx = to.width() / ch.width().max(1e-3);
            let sy = to.height() / ch.height().max(1e-3);
            Rect::from_min_size(pos2(to.min.x + (r.min.x - ch.min.x) * sx, to.min.y + (r.min.y - ch.min.y) * sy), vec2(r.width() * sx, r.height() * sy))
        }
        None => r,
    };
    for n in tree.children().filter(|c| c.is_element()) {
        match n.tag_name().name() {
            "sp" => {
                let ph = ph(&n);
                let rect = xfrm(&n).or_else(|| {
                    let (t, i) = ph.as_ref()?;
                    places.iter().find(|(k, _)| k.0 == *t && (i.is_empty() || k.1 == *i || k.1.is_empty())).or_else(|| places.iter().find(|(k, _)| k.0 == *t)).or_else(|| places.iter().find(|(k, _)| !i.is_empty() && k.1 == *i)).map(|p| p.1)
                });
                let Some(rect) = rect else { continue };
                let fill = child(&n, "spPr").and_then(|s| srgb(&s));
                let kind = match child(&n, "txBody") {
                    Some(tb) => {
                        let t = ph.as_ref().map(|p| p.0.as_str()).unwrap_or("");
                        let default = match t {
                            "title" | "ctrTitle" => 40.0,
                            "subTitle" => 24.0,
                            "body" | "obj" => 22.0,
                            _ => 18.0,
                        };
                        let bullets = matches!(t, "body" | "obj") && ph.as_ref().is_some_and(|p| p.0 != "body" || !p.1.is_empty() || true);
                        let middle = matches!(t, "title" | "ctrTitle") || child(&tb, "bodyPr").and_then(|b| attr(&b, "anchor")) == Some("ctr");
                        ShapeKind::Text { paras: a_paras(&tb, default, bullets), middle }
                    }
                    None => ShapeKind::Box,
                };
                out.push(Shape { rect: place(rect), fill, kind });
            }
            "pic" => {
                let Some(rect) = xfrm(&n) else { continue };
                let id = n.descendants().find(|d| is(d, "blip")).and_then(|b| attr(&b, "embed"));
                if let Some(i) = id.and_then(|id| srels.get(id).cloned()).and_then(|p| media(z, images, seen, &p)) {
                    out.push(Shape { rect: place(rect), fill: None, kind: ShapeKind::Image(i) });
                }
            }
            "graphicFrame" => {
                let Some(x) = child(&n, "xfrm").and_then(|x| Some(Rect::from_min_size(pos2(emu(&child(&x, "off")?, "x"), emu(&child(&x, "off")?, "y")), vec2(emu(&child(&x, "ext")?, "cx"), emu(&child(&x, "ext")?, "cy"))))) else { continue };
                if let Some(tbl) = find(&n, "tbl") {
                    let rows = kids(&tbl, "tr").map(|tr| kids(&tr, "tc").map(|tc| tc.descendants().filter(|d| is(d, "t")).filter_map(|t| t.text()).collect::<Vec<_>>().join("")).collect()).collect();
                    out.push(Shape { rect: place(x), fill: None, kind: ShapeKind::Table(rows) });
                }
            }
            "grpSp" => {
                let gx = child(&n, "grpSpPr").and_then(|g| child(&g, "xfrm"));
                let m = gx.and_then(|x| {
                    let to = Rect::from_min_size(pos2(emu(&child(&x, "off")?, "x"), emu(&child(&x, "off")?, "y")), vec2(emu(&child(&x, "ext")?, "cx"), emu(&child(&x, "ext")?, "cy")));
                    let ch = Rect::from_min_size(pos2(emu(&child(&x, "chOff")?, "x"), emu(&child(&x, "chOff")?, "y")), vec2(emu(&child(&x, "chExt")?, "cx"), emu(&child(&x, "chExt")?, "cy")));
                    Some((ch, place(to)))
                });
                pptx_shapes(z, &n, srels, places, images, seen, out, m.or(map));
            }
            "AlternateContent" => {
                if let Some(f) = child(&n, "Fallback") {
                    pptx_shapes(z, &f, srels, places, images, seen, out, map);
                }
            }
            _ => {}
        }
    }
}

/// DrawingML paragraphs (a:p): runs, alignment, bullets.
fn a_paras(tb: &roxmltree::Node, default: f32, bullets: bool) -> Vec<SPara> {
    let mut v = Vec::new();
    let mut numbers: HashMap<u8, usize> = HashMap::new();
    for p in kids(tb, "p") {
        let ppr = child(&p, "pPr");
        let level = ppr.and_then(|x| attr(&x, "lvl")).and_then(|l| l.parse().ok()).unwrap_or(0);
        let align = match ppr.and_then(|x| attr(&x, "algn")) {
            Some("ctr") => Align::Center,
            Some("r") => Align::Right,
            Some("just") => Align::Justify,
            _ => Align::Left,
        };
        let def = ppr.and_then(|x| child(&x, "defRPr")).and_then(|d| attr(&d, "sz")).and_then(|s| s.parse::<f32>().ok()).map(|s| s / 100.0);
        let mut runs = Vec::new();
        for r in p.children().filter(|c| is(c, "r") || is(c, "br") || is(c, "fld")) {
            if is(&r, "br") {
                runs.push(Run { text: "\n".into(), size: Some(def.unwrap_or(default)), ..Default::default() });
                continue;
            }
            let rpr = child(&r, "rPr");
            let b = |a: &str| rpr.and_then(|x| attr(&x, a)).is_some_and(|v| v == "1" || v == "true");
            runs.push(Run {
                text: child(&r, "t").and_then(|t| t.text()).unwrap_or("").to_string(),
                bold: b("b"),
                italic: b("i"),
                underline: rpr.and_then(|x| attr(&x, "u")).is_some_and(|u| u != "none"),
                strike: rpr.and_then(|x| attr(&x, "strike")).is_some_and(|s| s != "noStrike"),
                size: Some(rpr.and_then(|x| attr(&x, "sz")).and_then(|s| s.parse::<f32>().ok()).map(|s| s / 100.0).or(def).unwrap_or(default)),
                color: rpr.and_then(|x| srgb(&x)),
            });
        }
        let none = ppr.is_some_and(|x| child(&x, "buNone").is_some());
        let auto = ppr.and_then(|x| child(&x, "buAutoNum"));
        let chr = ppr.and_then(|x| child(&x, "buChar")).and_then(|c| attr(&c, "char")).map(|c| format!("{c} "));
        let has_text = runs.iter().any(|r| !r.text.trim().is_empty());
        let bullet = if !has_text || none {
            None
        } else if auto.is_some() {
            let n = numbers.entry(level).or_insert(0);
            *n += 1;
            Some(format!("{n}. "))
        } else {
            chr.or(bullets.then(|| "• ".to_string()))
        };
        v.push(SPara { runs, align, bullet, level });
    }
    v
}

fn odp(z: &mut Zip) -> Result<Deck, String> {
    let content = z.text("content.xml").ok_or("no content.xml")?;
    let styles = z.text("styles.xml").unwrap_or_default();
    let doc = roxmltree::Document::parse(&content).map_err(|e| e.to_string())?;
    let sdoc = roxmltree::Document::parse(&styles).ok();
    let size = sdoc
        .as_ref()
        .and_then(|s| s.descendants().find(|n| is(n, "page-layout-properties")).and_then(|p| Some(vec2(attr(&p, "page-width").and_then(length)?, attr(&p, "page-height").and_then(length)?))))
        .unwrap_or(vec2(28.0 * 72.0 / 2.54, 15.75 * 72.0 / 2.54));
    let mut all = vec![&doc];
    if let Some(s) = &sdoc {
        all.push(s);
    }
    let mut c = OdCtx { z, styles: od_styles(&all), images: Vec::new(), media: HashMap::new() };
    let pres = find(&doc.root(), "presentation").ok_or("no office:presentation")?;
    let mut slides = Vec::new();
    for page in kids(&pres, "page") {
        let bg = od_style(&c.styles, attr(&page, "style-name")).fill;
        let mut shapes = Vec::new();
        for n in page.children().filter(|n| n.is_element()) {
            let Some(rect) = (|| Some(Rect::from_min_size(pos2(attr(&n, "x").and_then(length)?, attr(&n, "y").and_then(length)?), vec2(attr(&n, "width").and_then(length)?, attr(&n, "height").and_then(length)?))))() else { continue };
            let class = attr(&n, "class").unwrap_or("");
            let fill = od_style(&c.styles, attr(&n, "style-name")).fill;
            if let Some(href) = find(&n, "image").and_then(|i| attr(&i, "href")) {
                if let Some(i) = media(c.z, &mut c.images, &mut c.media, href) {
                    shapes.push(Shape { rect, fill: None, kind: ShapeKind::Image(i) });
                }
                continue;
            }
            if let Some(t) = find(&n, "table") {
                let rows = t.descendants().filter(|r| is(r, "table-row")).map(|r| kids(&r, "table-cell").map(|cell| cell.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect::<Vec<_>>().join("")).collect()).collect();
                shapes.push(Shape { rect, fill: None, kind: ShapeKind::Table(rows) });
                continue;
            }
            let default = match class {
                "title" => 36.0,
                "subtitle" => 28.0,
                "outline" => 24.0,
                _ => 18.0,
            };
            let holder = child(&n, "text-box").unwrap_or(n);
            let mut blocks = Vec::new();
            od_blocks(&mut c, &holder, &mut blocks, None, &mut Vec::new());
            let paras: Vec<SPara> = blocks
                .into_iter()
                .filter_map(|b| match b {
                    Block::Para { list, align, mut runs, .. } => {
                        for r in &mut runs {
                            r.size = Some(r.size.unwrap_or(default));
                        }
                        let bullet = list.map(|(_, n)| n.map_or("• ".to_string(), |n| format!("{n}. ")));
                        Some(SPara { runs, align, bullet, level: list.map_or(0, |l| l.0) })
                    }
                    _ => None,
                })
                .collect();
            let kind = if paras.iter().any(|p| p.runs.iter().any(|r| !r.text.trim().is_empty())) { ShapeKind::Text { paras, middle: class == "title" } } else { ShapeKind::Box };
            if kind != ShapeKind::Box || fill.is_some() {
                shapes.push(Shape { rect, fill, kind });
            }
        }
        slides.push(Slide { bg, shapes });
    }
    Ok((size, slides, c.images))
}

// ---------------------------------------------------------------- spreadsheets

pub struct Sheets {
    pub sheets: Vec<crate::table::Sheet>,
    /// Each sheet's column widths (from its first rows).
    widths: HashMap<usize, Vec<f32>>,
}

impl Sheets {
    fn new(sheets: Vec<crate::table::Sheet>) -> Sheets {
        Sheets { sheets, widths: HashMap::new() }
    }
}

/// A1's letters for a column.
pub fn col_name(mut c: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (c % 26) as u8) as char);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s
}

const ROW_H: f32 = 24.0;
const HEAD_W: f32 = 52.0;

/// The grid, zoomed; the view it's in.
fn draw_sheets(ui: &mut Ui, d: &mut DocState) -> Rect {
    let vis = ui.visuals().clone();
    let k = d.zoom;
    let (row_h, head_w, font, small) = ((ROW_H * k).round(), (HEAD_W * k).round(), FontId::proportional(14.0 * k), FontId::proportional(12.0 * k));
    let Body::Sheets(s) = &mut d.body else { return Rect::NOTHING };
    let n = s.sheets.len();
    d.sheet = d.sheet.min(n.saturating_sub(1));
    // The sheets, as tabs (the triggers on the pad).
    if n > 1 {
        ui.horizontal_wrapped(|ui| {
            for (i, sh) in s.sheets.iter().enumerate() {
                if ui.selectable_label(i == d.sheet, &sh.name).clicked() {
                    d.sheet = i;
                    d.scroll_to = Some(Vec2::ZERO);
                }
            }
        });
        ui.add_space(4.0);
    }
    let Some(sheet) = s.sheets.get(d.sheet) else { return Rect::NOTHING };
    let rows = sheet.rows.len();
    let cols = sheet.rows.iter().map(|r| r.len()).max().unwrap_or(0).max(1);
    let widths = s.widths.entry(d.sheet).or_insert_with(|| {
        (0..cols)
            .map(|c| {
                let longest = sheet.rows.iter().take(300).filter_map(|r| r.get(c)).map(|cell| cell.text().chars().count()).max().unwrap_or(0);
                (longest as f32 * 7.6 + 16.0).clamp(56.0, 340.0)
            })
            .collect()
    });
    let xs: Vec<f32> = std::iter::once(head_w).chain(widths.iter().scan(head_w, |x, w| {
        *x += (w * k).round();
        Some(*x)
    })).collect();
    let total = vec2(*xs.last().unwrap_or(&head_w), (rows + 1) as f32 * row_h);
    copy_menu(ui, ui.available_rect_before_wrap());
    let mut area = egui::ScrollArea::both().id_salt((d.salt, d.sheet)).auto_shrink([false, false]);
    if let Some(sc) = d.scroll_to.take() {
        area = area.scroll_offset(sc);
    }
    let line = vis.widgets.noninteractive.bg_stroke.color;
    let out = area.show_viewport(ui, |ui, view| {
        let (whole, _) = ui.allocate_exact_size(total, Sense::hover());
        let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(whole));
        let o = whole.min;
        let r0 = ((view.min.y / row_h).floor() as usize).saturating_sub(1);
        let r1 = ((view.max.y / row_h).ceil() as usize + 1).min(rows + 1);
        let c0 = xs.partition_point(|x| *x <= view.min.x).saturating_sub(1);
        let c1 = (xs.partition_point(|x| *x < view.max.x) + 1).min(cols);
        let p = ui.painter().clone();
        let head = vis.faint_bg_color;
        for r in r0..r1 {
            let y = o.y + r as f32 * row_h;
            for c in c0..c1 {
                let cell = Rect::from_min_max(pos2(o.x + xs[c], y), pos2(o.x + xs[c + 1], y + row_h));
                p.rect_stroke(cell, 0.0, Stroke::new(0.5, line), egui::StrokeKind::Inside);
                if r == 0 {
                    p.rect_filled(cell, 0.0, head);
                    p.text(cell.center(), Align2::CENTER_CENTER, col_name(c), small.clone(), vis.weak_text_color());
                    continue;
                }
                let Some(v) = sheet.rows[r - 1].get(c) else { continue };
                let text = v.text();
                if text.is_empty() {
                    continue;
                }
                let num = matches!(v, crate::table::Cell::Num(_));
                let g = ui.painter().layout_no_wrap(text.clone(), font.clone(), vis.text_color());
                let w = g.size().x.min(cell.width() - 8.0 * k);
                let at = if num { pos2(cell.max.x - 4.0 * k - w, cell.min.y) } else { pos2(cell.min.x + 4.0 * k, cell.min.y) };
                let job = LayoutJob::single_section(text, TextFormat { font_id: font.clone(), color: vis.text_color(), ..Default::default() });
                ui.put(Rect::from_min_size(at, vec2(w.max(1.0), row_h)), egui::Label::new(job).truncate());
            }
            // The row's number at its left.
            let hr = Rect::from_min_size(pos2(o.x, y), vec2(head_w, row_h));
            p.rect_filled(hr, 0.0, head);
            p.rect_stroke(hr, 0.0, Stroke::new(0.5, line), egui::StrokeKind::Inside);
            if r > 0 {
                p.text(hr.center(), Align2::CENTER_CENTER, r.to_string(), small.clone(), vis.weak_text_color());
            }
        }
    });
    d.offset = out.state.offset;
    d.view = out.inner_rect.size();
    d.content = out.content_size;
    out.inner_rect
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
        assert_eq!(col_name(27), "AB");
        assert_eq!(col_name(701), "ZZ");
        assert_eq!(col_name(702), "AAA");
        assert_eq!(resolve("ppt/slides", "../media/a.png"), "ppt/media/a.png");
        assert_eq!(resolve("word", "media/b.png"), "word/media/b.png");
        assert!((length("2.54cm").unwrap() - 72.0).abs() < 1e-3);
        assert_eq!(length("1in"), Some(72.0));
        assert_eq!(doc_kind("Report.DOCX"), Some(DocKind::Word));
        assert_eq!(doc_kind("a.odp"), Some(DocKind::Slides));
        assert_eq!(doc_kind("a.csv"), Some(DocKind::Sheets));
        assert_eq!(family("old.ppt"), Some(Family::Slides));
        assert_eq!(family("old.doc"), Some(Family::Text));
        assert_eq!(family("a.tsv"), Some(Family::Sheets));
        let sizes = page_sizes("Pages:           2\nPage    1 size: 595.276 x 841.89 pts (A4)\nPage    1 rot:  0\nPage    2 size: 612 x 792 pts (letter)\n");
        assert_eq!(sizes, vec![vec2(595.276, 841.89), vec2(612.0, 792.0)]);
        let html = r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd"><html xmlns="http://www.w3.org/1999/xhtml"><body><doc><page width="612" height="792"><flow><block><line xMin="72" yMin="70" xMax="200" yMax="84"><word xMin="72" yMin="70" xMax="110" yMax="84">Hello</word><word xMin="114" yMin="70" xMax="200" yMax="84">world</word></line></block></flow></page></doc></body></html>"#;
        let l = parse_bbox(html);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].text, "Hello world");
        assert_eq!(l[0].rect, Rect::from_min_max(pos2(72.0, 70.0), pos2(200.0, 84.0)));
    }
}

/// Small documents of each kind, made here (tests).
#[cfg(test)]
pub mod fixtures {
    use std::io::Write;
    use std::path::Path;

    fn zip(path: &Path, files: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in files {
            z.start_file(*name, o).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    /// A small PNG: a red square.
    pub fn png() -> Vec<u8> {
        let img = image::RgbImage::from_fn(40, 30, |x, _| image::Rgb([200, (x * 5) as u8, 40]));
        let mut b = std::io::Cursor::new(Vec::new());
        img.write_to(&mut b, image::ImageFormat::Png).unwrap();
        b.into_inner()
    }

    /// Two pages of Helvetica text.
    pub fn pdf(path: &Path) {
        let pages = ["Hello PDF world", "Second page here"];
        let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), String::new(), "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()];
        let mut kids = Vec::new();
        for t in pages {
            let content = format!("BT /F1 24 Tf 72 720 Td ({t}) Tj ET");
            objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
            let c = objs.len();
            objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {c} 0 R /Resources << /Font << /F1 3 0 R >> >> >>"));
            kids.push(format!("{} 0 R", objs.len()));
        }
        objs[1] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), kids.len());
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
        std::fs::write(path, out).unwrap();
    }

    pub fn docx(path: &Path) {
        let w = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;
        let doc = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document {w}><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Quarterly report</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Sales grew </w:t></w:r><w:r><w:rPr><w:b/><w:color w:val="C00000"/></w:rPr><w:t>twelve percent</w:t></w:r><w:r><w:t xml:space="preserve"> this year.</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>First point</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Second point</w:t></w:r></w:p>
<w:tbl><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="1500"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:shd w:val="clear" w:fill="D9E2F3"/></w:tcPr><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Region</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:shd w:val="clear" w:fill="D9E2F3"/></w:tcPr><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Total</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p><w:r><w:t>North</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>1200</w:t></w:r></w:p><w:p><w:r><w:t>(two lines)</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc><w:tc><w:p><w:r><w:t>800</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t>All regions: 2000</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:drawing><wp:inline><wp:extent cx="1270000" cy="952500"/><a:graphic><a:graphicData><a:blip r:embed="rId9"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
<w:p><w:r><w:br w:type="page"/></w:r></w:p>
<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:i/></w:rPr><w:t>The end</w:t></w:r></w:p>
</w:body></w:document>"#
        );
        let styles = format!(r#"<?xml version="1.0"?><w:styles {w}><w:style w:styleId="Heading1"><w:name w:val="heading 1"/></w:style></w:styles>"#);
        let numbering = format!(r#"<?xml version="1.0"?><w:numbering {w}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#);
        let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="image" Target="media/image1.png"/></Relationships>"#;
        let png = png();
        zip(path, &[("[Content_Types].xml", b"<Types/>"), ("word/document.xml", doc.as_bytes()), ("word/styles.xml", styles.as_bytes()), ("word/numbering.xml", numbering.as_bytes()), ("word/_rels/document.xml.rels", rels.as_bytes()), ("word/media/image1.png", &png)]);
    }

    pub fn odt(path: &Path) {
        let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0""#;
        let content = format!(
            r#"<?xml version="1.0"?><office:document-content {ns}><office:automatic-styles><style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style><style:style style:name="P1" style:family="paragraph"><style:paragraph-properties fo:text-align="center"/></style:style><text:list-style style:name="L1"><text:list-level-style-number text:level="1"/></text:list-style></office:automatic-styles><office:body><office:text>
<text:h text:outline-level="1">Meeting notes</text:h>
<text:p>We agreed on <text:span text:style-name="T1">three things</text:span>:</text:p>
<text:list text:style-name="L1"><text:list-item><text:p>Budget</text:p></text:list-item><text:list-item><text:p>Dates</text:p></text:list-item></text:list>
<table:table><table:table-header-rows><table:table-row><table:table-cell><text:p>Who</text:p></table:table-cell><table:table-cell><text:p>What</text:p></table:table-cell></table:table-row></table:table-header-rows><table:table-row><table:table-cell table:number-columns-spanned="2"><text:p>Everyone</text:p></table:table-cell><table:covered-table-cell/></table:table-row></table:table>
<text:p><draw:frame svg:width="2cm" svg:height="1.5cm"><draw:image xlink:href="Pictures/a.png"/></draw:frame></text:p>
<text:p text:style-name="P1">Signed</text:p>
</office:text></office:body></office:document-content>"#
        );
        let png = png();
        zip(path, &[("mimetype", b"application/vnd.oasis.opendocument.text"), ("content.xml", content.as_bytes()), ("styles.xml", format!("<office:document-styles {ns}/>").as_bytes()), ("Pictures/a.png", &png)]);
    }

    pub fn pptx(path: &Path) {
        let ns = r#"xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
        let pres = format!(r#"<?xml version="1.0"?><p:presentation {ns}><p:sldIdLst><p:sldId id="256" r:id="rId1"/><p:sldId id="257" r:id="rId2"/></p:sldIdLst><p:sldSz cx="9144000" cy="5143500"/></p:presentation>"#);
        let prels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="slides/slide1.xml"/><Relationship Id="rId2" Target="slides/slide2.xml"/></Relationships>"#;
        let sp = |ty: &str, x: i64, y: i64, cx: i64, cy: i64, body: &str| format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="s"/><p:cNvSpPr/><p:nvPr><p:ph type="{ty}"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/>{body}</p:txBody></p:sp>"#);
        let s1 = format!(
            r#"<?xml version="1.0"?><p:sld {ns}><p:cSld><p:spTree>{}{}</p:spTree></p:cSld></p:sld>"#,
            sp("title", 457200, 228600, 8229600, 1000000, r#"<a:p><a:r><a:rPr sz="4000" b="1"/><a:t>Polka roadmap</a:t></a:r></a:p>"#),
            sp("body", 457200, 1400000, 8229600, 3200000, r#"<a:p><a:r><a:t>Documents in the viewer</a:t></a:r></a:p><a:p><a:pPr lvl="1"/><a:r><a:t>PDF, Word, slides</a:t></a:r></a:p>"#)
        );
        let s2 = format!(
            r#"<?xml version="1.0"?><p:sld {ns}><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="1F3864"/></a:solidFill></p:bgPr></p:bg><p:spTree>{}<p:pic><p:blipFill><a:blip r:embed="rId5"/></p:blipFill><p:spPr><a:xfrm><a:off x="5000000" y="1500000"/><a:ext cx="3000000" cy="2250000"/></a:xfrm></p:spPr></p:pic></p:spTree></p:cSld></p:sld>"#,
            sp("title", 457200, 1500000, 4000000, 1500000, r#"<a:p><a:r><a:rPr sz="3200"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill></a:rPr><a:t>Thank you</a:t></a:r></a:p>"#)
        );
        let s2rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId5" Target="../media/image1.png"/></Relationships>"#;
        let png = png();
        zip(path, &[("ppt/presentation.xml", pres.as_bytes()), ("ppt/_rels/presentation.xml.rels", prels.as_bytes()), ("ppt/slides/slide1.xml", s1.as_bytes()), ("ppt/slides/slide2.xml", s2.as_bytes()), ("ppt/slides/_rels/slide2.xml.rels", s2rels.as_bytes()), ("ppt/media/image1.png", &png)]);
    }

    pub fn odp(path: &Path) {
        let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0""#;
        let content = format!(
            r#"<?xml version="1.0"?><office:document-content {ns}><office:body><office:presentation><draw:page draw:name="p1"><draw:frame presentation:class="title" svg:x="2cm" svg:y="1cm" svg:width="24cm" svg:height="3cm"><draw:text-box><text:p>Open slides</text:p></draw:text-box></draw:frame><draw:frame presentation:class="outline" svg:x="2cm" svg:y="5cm" svg:width="24cm" svg:height="8cm"><draw:text-box><text:list><text:list-item><text:p>Made in Impress</text:p></text:list-item></text:list></draw:text-box></draw:frame></draw:page></office:presentation></office:body></office:document-content>"#
        );
        let styles = format!(r#"<?xml version="1.0"?><office:document-styles {ns}><office:automatic-styles><style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm"/></style:page-layout></office:automatic-styles></office:document-styles>"#);
        zip(path, &[("mimetype", b"application/vnd.oasis.opendocument.presentation"), ("content.xml", content.as_bytes()), ("styles.xml", styles.as_bytes())]);
    }

    pub fn xlsx(path: &Path) {
        let mut wb = rust_xlsxwriter::Workbook::new();
        let ws = wb.add_worksheet();
        ws.set_name("Sales").unwrap();
        for (c, h) in ["Region", "Q1", "Q2"].iter().enumerate() {
            ws.write_string(0, c as u16, *h).unwrap();
        }
        for r in 1..=40u32 {
            ws.write_string(r, 0, format!("Region {r}")).unwrap();
            ws.write_number(r, 1, r as f64 * 10.5).unwrap();
            ws.write_number(r, 2, r as f64 * 3.0).unwrap();
        }
        let ws2 = wb.add_worksheet();
        ws2.set_name("Notes").unwrap();
        ws2.write_string(0, 0, "Second sheet").unwrap();
        wb.save(path).unwrap();
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("polka-docs-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| match b {
                Block::Para { runs, .. } => runs.iter().map(|r| r.text.as_str()).collect(),
                Block::Table { .. } => "<table>".into(),
                Block::Image(..) => "<image>".into(),
                Block::Break => "<break>".into(),
            })
            .collect()
    }

    #[test]
    fn word() {
        let p = dir().join("a.docx");
        fixtures::docx(&p);
        let f = Flow::open(&p).unwrap();
        assert_eq!(texts(&f.blocks), vec!["Quarterly report", "Sales grew twelve percent this year.", "First point", "Second point", "<table>", "<image>", "<break>", "The end"]);
        assert!(matches!(&f.blocks[0], Block::Para { level: 1, .. }));
        let Block::Para { runs, .. } = &f.blocks[1] else { panic!() };
        assert!(runs[1].bold && runs[1].color == Some(Color32::from_rgb(0xC0, 0, 0)));
        assert!(matches!(&f.blocks[3], Block::Para { list: Some((0, Some(2))), .. }), "{:?}", f.blocks[3]);
        let Block::Table { widths, rows } = &f.blocks[4] else { panic!() };
        assert_eq!(widths, &vec![3000.0, 1500.0]);
        assert_eq!(rows[0][0].fill, Some(Color32::from_rgb(0xD9, 0xE2, 0xF3)));
        assert!(!rows[1][0].merged && rows[2][0].merged && rows[2][0].blocks.is_empty());
        assert_eq!((rows[3].len(), rows[3][0].span), (1, 2));
        assert!(matches!(&f.blocks[5], Block::Image(0, s) if (s.x - 100.0).abs() < 0.1));
        assert!(matches!(&f.blocks[7], Block::Para { align: Align::Center, .. }));
        assert_eq!(f.images.len(), 1);
        let p = dir().join("a.odt");
        fixtures::odt(&p);
        let f = Flow::open(&p).unwrap();
        assert_eq!(texts(&f.blocks), vec!["Meeting notes", "We agreed on three things:", "Budget", "Dates", "<table>", "<image>", "Signed"]);
        let Block::Para { runs, .. } = &f.blocks[1] else { panic!() };
        assert!(runs.iter().any(|r| r.bold && r.text == "three things"));
        assert!(matches!(&f.blocks[3], Block::Para { list: Some((0, Some(2))), .. }), "{:?}", f.blocks[3]);
        assert!(matches!(&f.blocks[6], Block::Para { align: Align::Center, .. }));
        let Block::Table { rows, .. } = &f.blocks[4] else { panic!() };
        assert_eq!((rows.len(), rows[1].len(), rows[1][0].span), (2, 1, 2));
    }

    #[test]
    fn slides() {
        let p = dir().join("a.pptx");
        fixtures::pptx(&p);
        let s = Slides::open(&p).unwrap();
        assert_eq!(s.slides.len(), 2);
        assert!((s.size.x - 720.0).abs() < 0.1 && (s.size.y - 405.0).abs() < 0.1);
        let ShapeKind::Text { paras, middle } = &s.slides[0].shapes[0].kind else { panic!() };
        assert!(*middle && paras[0].runs[0].text == "Polka roadmap" && paras[0].runs[0].size == Some(40.0) && paras[0].runs[0].bold);
        let ShapeKind::Text { paras, .. } = &s.slides[0].shapes[1].kind else { panic!() };
        assert_eq!((paras[1].level, paras[1].bullet.as_deref()), (1, Some("• ")));
        assert_eq!(s.slides[1].bg, Some(Color32::from_rgb(0x1F, 0x38, 0x64)));
        assert!(matches!(s.slides[1].shapes[1].kind, ShapeKind::Image(0)));
        let p = dir().join("a.odp");
        fixtures::odp(&p);
        let s = Slides::open(&p).unwrap();
        assert_eq!(s.slides.len(), 1);
        assert!((s.size.x - 28.0 * 72.0 / 2.54).abs() < 0.1);
        let ShapeKind::Text { paras, middle } = &s.slides[0].shapes[0].kind else { panic!() };
        assert!(*middle && paras[0].runs[0].text == "Open slides" && paras[0].runs[0].size == Some(36.0));
        let ShapeKind::Text { paras, .. } = &s.slides[0].shapes[1].kind else { panic!() };
        assert_eq!(paras[0].bullet.as_deref(), Some("• "));
    }

    #[test]
    fn pdf_pages() {
        if crate::terminal::installed(&["pdftoppm"]).is_empty() {
            return;
        }
        let p = dir().join("a.pdf");
        fixtures::pdf(&p);
        let out = std::process::Command::new("pdfinfo").args(["-f", "1", "-l", "100000"]).arg(&p).output().unwrap();
        assert_eq!(page_sizes(&String::from_utf8_lossy(&out.stdout)), vec![vec2(612.0, 792.0); 2]);
        let d = dir();
        let img = render(&p, 1, 36.0, &d).unwrap();
        assert_eq!(img.size, [306, 396]);
        let lines = page_text(&p, 0, &d);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Hello PDF world");
    }
}
