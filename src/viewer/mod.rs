//! Built-in viewer: text (with syntax highlighting), pictures, sound,
//! video and documents, shown in place of a tab's panes.

pub mod audio;
pub mod docs;
pub mod draw;
pub mod edit;
pub mod glue;
pub mod image;
pub mod md;
pub mod player;
pub mod syntax;
pub mod text;
pub mod theme;
pub mod video;
pub mod video_app;

use std::path::{Path, PathBuf};

use egui::Vec2;

use crate::input::Action;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Image,
    Audio,
    Video,
    /// PDF, word processing, presentations, spreadsheets.
    Doc,
}

/// What the viewer can show for this file, if anything. Launchers and
/// programs keep running instead of being shown.
pub fn kind(path: &Path, name: &str) -> Option<Kind> {
    if image::is_picture(path, name) {
        return Some(Kind::Image);
    }
    if audio::is_audio(name) {
        return Some(Kind::Audio);
    }
    if video::is_video(name) || crate::steam::is_recording(path) {
        return Some(Kind::Video);
    }
    if docs::doc_kind(name).is_some() && std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return Some(Kind::Doc);
    }
    if crate::launch::runs(path) || crate::vfs::is_archive_name(name) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mime_text = mime_guess::from_path(name).first().is_some_and(|m| m.type_() == "text");
    if syntax::detect(name).is_some() || mime_text || text::sniff(path) {
        return Some(Kind::Text);
    }
    None
}

/// Which kinds open in Polka (from the settings).
#[derive(Clone, Copy)]
pub struct OpenIn {
    pub text: bool,
    pub images: bool,
    pub audio: bool,
    pub video: bool,
    pub docs: bool,
}

impl OpenIn {
    pub fn from(cfg: &crate::config::Config) -> Self {
        OpenIn { text: cfg.open_text, images: cfg.open_images, audio: cfg.open_music, video: cfg.open_videos, docs: cfg.open_docs }
    }
}

/// [`kind`], if that kind is set to open in Polka.
pub fn kind_in(path: &Path, name: &str, open: OpenIn) -> Option<Kind> {
    match kind(path, name)? {
        Kind::Text if open.text => Some(Kind::Text),
        Kind::Image if open.images => Some(Kind::Image),
        Kind::Audio if open.audio => Some(Kind::Audio),
        Kind::Video if open.video => Some(Kind::Video),
        Kind::Doc if open.docs => Some(Kind::Doc),
        _ => None,
    }
}

pub struct TextState {
    pub doc: text::TextDoc,
    pub ed: edit::Editor,
    /// Can't be saved (e.g. in a read-only archive).
    pub read_only: bool,
    /// Rendered Markdown for a document generation, its scroll and anchors.
    pub md: Option<(u64, md::MdDoc)>,
    pub md_scroll: f32,
    pub md_scroll_to: Option<f32>,
    pub anchors: std::collections::HashMap<String, f32>,
    pub rows: Vec<text::Row>,
    /// What `rows` was built for: wrap columns.
    pub rows_for: Option<Option<usize>>,
    /// Scroll offset from the last frame, and one to apply.
    pub scroll: Vec2,
    pub scroll_to: Option<Vec2>,
    pub row_h: f32,
    pub page_rows: usize,
    pub char_w: f32,
}

pub struct ImageState {
    pub pic: image::Picture,
    /// Pictures of the folder, for ←/→.
    pub siblings: Vec<PathBuf>,
    pub index: usize,
    /// Scale (1 = one image pixel per point); None = fit to the view.
    pub zoom: Option<f32>,
    /// Offset of the picture's centre from the view's centre.
    pub pan: Vec2,
    /// Fit scale and view size from the last frame.
    pub fit: f32,
    pub view: Vec2,
    /// Scale that shows one picture pixel per screen pixel (last frame).
    pub actual: f32,
}

pub enum Content {
    Text(Box<TextState>),
    Image(Box<ImageState>),
    Audio(Box<audio::AudioState>),
    Video(Box<video::VideoState>),
    Doc(Box<docs::DocState>),
    Error(String),
}

pub struct Viewer {
    /// The file on disk (a temp copy for files inside archives).
    pub path: PathBuf,
    pub name: String,
    /// Pane the file was opened from, and the folder it showed then.
    pub origin: usize,
    pub folder: Option<crate::vfs::Location>,
    /// For files opened from an archive: (archive, inner path).
    pub source: Option<(PathBuf, String)>,
    pub content: Content,
}

/// What the app should do after a key in the viewer.
pub enum Out {
    Nothing,
    Close,
    Settings,
    Actions,
    /// Another picture of the folder became current.
    Moved(PathBuf),
}

impl Viewer {
    pub fn open(path: PathBuf, name: String, origin: usize, siblings: Vec<PathBuf>, volume: f32, ctx: &egui::Context) -> Self {
        let content = match kind(&path, &name) {
            Some(Kind::Audio) => Content::Audio(Box::new(audio::AudioState::open(path.clone(), siblings, volume, ctx))),
            Some(Kind::Video) => Content::Video(Box::new(video::VideoState::open(path.clone(), siblings, volume, ctx))),
            Some(Kind::Doc) => Content::Doc(Box::new(docs::DocState::open(&path, &name, ctx))),
            Some(Kind::Image) => {
                let index = siblings.iter().position(|p| *p == path).unwrap_or(0);
                let siblings = if siblings.is_empty() { vec![path.clone()] } else { siblings };
                Content::Image(Box::new(ImageState {
                    pic: image::Picture::open(path.clone(), ctx),
                    siblings,
                    index,
                    zoom: None,
                    pan: Vec2::ZERO,
                    fit: 1.0,
                    view: Vec2::ZERO,
                    actual: 1.0,
                }))
            }
            _ => {
                let c = ctx.clone();
                match text::TextDoc::load(&path, &name, move || c.request_repaint()) {
                    Ok(doc) => Content::Text(Box::new(TextState {
                        ed: edit::Editor::new(&doc.text),
                        read_only: false,
                        md: None,
                        md_scroll: 0.0,
                        md_scroll_to: None,
                        anchors: Default::default(),
                        doc,
                        rows: Vec::new(),
                        rows_for: None,
                        scroll: Vec2::ZERO,
                        scroll_to: None,
                        row_h: 18.0,
                        page_rows: 20,
                        char_w: 8.0,
                    })),
                    Err(e) => Content::Error(e.to_string()),
                }
            }
        };
        Viewer { path, name, origin, folder: None, source: None, content }
    }

    pub fn is_text(&self) -> bool {
        matches!(self.content, Content::Text(_))
    }

    pub fn is_doc(&self) -> bool {
        matches!(self.content, Content::Doc(_))
    }

    pub fn is_sheets(&self) -> bool {
        matches!(&self.content, Content::Doc(d) if matches!(d.body, docs::Body::Sheets(_)))
    }

    pub fn input(&mut self, a: Action, ctx: &egui::Context) -> Out {
        match a {
            Action::Back => return Out::Close,
            Action::Actions => return Out::Actions,
            Action::Menu if self.is_text() => return Out::Settings,
            _ => {}
        }
        match &mut self.content {
            Content::Text(t) => {
                let rows = t.rows.len() as f32;
                let page = t.page_rows.max(1) as f32 * t.row_h;
                let mut s = t.scroll;
                match a {
                    Action::Up => s.y -= t.row_h,
                    Action::Down => s.y += t.row_h,
                    Action::LeftTrigger => s.y -= page,
                    Action::RightTrigger => s.y += page,
                    Action::Left => s.x -= t.char_w * 8.0,
                    Action::Right => s.x += t.char_w * 8.0,
                    Action::Home => s = Vec2::ZERO,
                    Action::End => s.y = rows * t.row_h,
                    _ => return Out::Nothing,
                }
                s.x = s.x.max(0.0);
                s.y = s.y.clamp(0.0, (rows * t.row_h - page).max(0.0));
                t.scroll_to = Some(s);
                Out::Nothing
            }
            Content::Image(im) => {
                let zoomed = im.zoom.is_some_and(|z| z > im.fit + 1e-3);
                let step = 120.0;
                match a {
                    Action::Left | Action::Right if !zoomed => {
                        let n = im.siblings.len();
                        if n > 1 {
                            im.index = if a == Action::Right { (im.index + 1) % n } else { (im.index + n - 1) % n };
                            let p = im.siblings[im.index].clone();
                            im.pic = image::Picture::open(p.clone(), ctx);
                            im.zoom = None;
                            im.pan = Vec2::ZERO;
                            self.name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            self.path = p.clone();
                            return Out::Moved(p);
                        }
                    }
                    Action::Left => im.pan.x += step,
                    Action::Right => im.pan.x -= step,
                    Action::Up => im.pan.y += step,
                    Action::Down => im.pan.y -= step,
                    Action::RightTrigger => im.zoom_by(1.25),
                    Action::LeftTrigger => im.zoom_by(0.8),
                    Action::Accept => im.toggle_actual(),
                    Action::Home => {
                        im.zoom = None;
                        im.pan = Vec2::ZERO;
                    }
                    _ => {}
                }
                Out::Nothing
            }
            Content::Doc(d) => {
                d.input(a);
                Out::Nothing
            }
            Content::Audio(_) | Content::Video(_) | Content::Error(_) => Out::Nothing,
        }
    }
}

impl ImageState {
    pub fn scale(&self) -> f32 {
        self.zoom.unwrap_or(self.fit)
    }

    /// Zoom keeping the view centre on the same spot of the picture.
    pub fn zoom_by(&mut self, k: f32) {
        let old = self.scale();
        let new = (old * k).clamp(0.02, 32.0);
        self.pan *= new / old;
        // Back at (or below) fit: snap to fit.
        if new <= self.fit + 1e-3 && k < 1.0 {
            self.zoom = None;
            self.pan = Vec2::ZERO;
        } else {
            self.zoom = Some(new);
        }
    }

    /// Fit ↔ one picture pixel per screen pixel.
    pub fn toggle_actual(&mut self) {
        let at_actual = self.zoom.is_some_and(|z| (z - self.actual).abs() < 1e-4);
        self.zoom = if at_actual { None } else { Some(self.actual) };
        self.pan = Vec2::ZERO;
    }
}
