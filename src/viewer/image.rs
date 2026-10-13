//! Pictures: decoded in the background (EXIF rotation applied, huge ones
//! scaled down to what a texture can hold); SVG and animated GIF go through
//! egui's own loaders.

use std::path::{Path, PathBuf};

use crossbeam_channel::Receiver;

pub const EXTS: &[&str] = &["png", "jpg", "jpeg", "jpe", "gif", "webp", "bmp", "tif", "tiff", "ico", "svg", "svgz"];

pub fn is_image(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.rsplit_once('.').is_some_and(|(_, e)| EXTS.contains(&e))
}

/// Drawn by egui's loaders from the file URI.
fn by_uri(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_lowercase();
    lower.ends_with(".svg") || lower.ends_with(".svgz") || lower.ends_with(".gif")
}

pub enum Pixels {
    Texture(egui::TextureHandle),
    Uri(String),
}

pub struct Decoded {
    pub image: egui::ColorImage,
    /// Size of the original (after rotation), before any scaling down.
    pub full: [usize; 2],
}

pub struct Picture {
    pub path: PathBuf,
    pub pixels: Option<Pixels>,
    pub full: Option<[usize; 2]>,
    pub error: Option<String>,
    loading: Option<Receiver<Result<Decoded, String>>>,
}

fn decode(path: &Path, max_side: usize) -> Result<Decoded, String> {
    use image::ImageDecoder;
    let reader = image::ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    let mut dec = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(dec).map_err(|e| e.to_string())?;
    img.apply_orientation(orientation);
    let full = [img.width() as usize, img.height() as usize];
    if img.width().max(img.height()) as usize > max_side {
        img = img.resize(max_side as u32, max_side as u32, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(Decoded { image: egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), full })
}

impl Picture {
    pub fn open(path: PathBuf, ctx: &egui::Context) -> Self {
        if by_uri(&path) {
            let uri = format!("file://{}", path.display());
            return Picture { path, pixels: Some(Pixels::Uri(uri)), full: None, error: None, loading: None };
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let max_side = ctx.input(|i| i.max_texture_side).clamp(2048, 16384);
        let (p, ctx2) = (path.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("polka-image".into())
            .spawn(move || {
                let _ = tx.send(decode(&p, max_side));
                ctx2.request_repaint();
            })
            .ok();
        Picture { path, pixels: None, full: None, error: None, loading: Some(rx) }
    }

    /// Pick up a finished decode.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.loading else { return };
        if let Ok(r) = rx.try_recv() {
            self.loading = None;
            match r {
                Ok(d) => {
                    self.full = Some(d.full);
                    let name = self.path.to_string_lossy().into_owned();
                    self.pixels = Some(Pixels::Texture(ctx.load_texture(name, d.image, egui::TextureOptions::LINEAR)));
                }
                Err(e) => self.error = Some(e),
            }
        }
    }
}
