//! Thumbnails for images and videos, stored in the shared freedesktop cache
//! (~/.cache/thumbnails/normal) so other file managers' thumbnails are reused
//! and ours are reused by them. Generated on a few worker threads, most
//! recently requested first (what's on screen now).

use std::collections::HashMap;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

const SIZE: u32 = 128;
const MAX_QUEUE: usize = 256;
/// Bigger images aren't worth decoding for a thumbnail.
const MAX_IMAGE_BYTES: u64 = 150 << 20;

const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "ico", "tif", "tiff"];
const VIDEO_EXT: &[&str] = &["mp4", "mkv", "webm", "avi", "mov", "m4v", "wmv", "flv", "mpg", "mpeg", "ts", "3gp"];

#[derive(Clone)]
enum Slot {
    Pending,
    /// file:// uri and width/height ratio
    Ready(String, f32),
    Failed,
}

type Key = (PathBuf, i64);

struct Shared {
    slots: Mutex<HashMap<Key, Slot>>,
    queue: Mutex<Vec<Key>>,
    cv: Condvar,
    wake: Box<dyn Fn() + Send + Sync>,
}

pub struct Thumbs {
    shared: Arc<Shared>,
}

fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn video_tool() -> bool {
    static HAS: OnceLock<bool> = OnceLock::new();
    *HAS.get_or_init(|| {
        std::process::Command::new("ffmpegthumbnailer").arg("-v").output().is_ok()
    })
}

/// Files in a cursor theme's `cursors` folder (checked when decoding).
fn in_cursor_dir(path: &Path) -> bool {
    path.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "cursors")
}

pub fn supported(path: &Path) -> bool {
    let e = ext_of(path);
    in_cursor_dir(path)
        || IMAGE_EXT.contains(&e.as_str()) || (VIDEO_EXT.contains(&e.as_str()) && video_tool())
        || crate::viewer::audio::EXTS.contains(&e.as_str())
}

impl Thumbs {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            slots: Mutex::new(HashMap::new()),
            queue: Mutex::new(Vec::new()),
            cv: Condvar::new(),
            wake: Box::new(wake),
        });
        let n = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 4));
        for i in 0..n {
            let s = shared.clone();
            std::thread::Builder::new()
                .name(format!("polka-thumb-{i}"))
                .spawn(move || worker(s))
                .expect("spawn thumbnail worker");
        }
        Self { shared }
    }

    /// Thumbnail uri and aspect ratio if ready; otherwise queues it.
    pub fn get(&self, path: &Path, mtime: i64) -> Option<(String, f32)> {
        let key = (path.to_path_buf(), mtime);
        let mut slots = self.shared.slots.lock().unwrap();
        match slots.get(&key) {
            Some(Slot::Ready(u, a)) => return Some((u.clone(), *a)),
            Some(_) => return None,
            None => {}
        }
        if !supported(path) {
            slots.insert(key, Slot::Failed);
            return None;
        }
        slots.insert(key.clone(), Slot::Pending);
        drop(slots);
        let mut q = self.shared.queue.lock().unwrap();
        q.push(key);
        if q.len() > MAX_QUEUE {
            // Long scrolled past; forget them so they can be asked for again.
            let excess = q.len() - MAX_QUEUE;
            let stale: Vec<Key> = q.drain(..excess).collect();
            let mut slots = self.shared.slots.lock().unwrap();
            for k in stale {
                slots.remove(&k);
            }
        }
        self.shared.cv.notify_one();
        None
    }
}

fn worker(s: Arc<Shared>) {
    loop {
        let key = {
            let mut q = s.queue.lock().unwrap();
            loop {
                if let Some(k) = q.pop() {
                    break k;
                }
                q = s.cv.wait(q).unwrap();
            }
        };
        let slot = match thumbnail(&key.0, key.1) {
            Some((p, w, h)) => Slot::Ready(crate::icons::uri(&p), w as f32 / h.max(1) as f32),
            None => Slot::Failed,
        };
        s.slots.lock().unwrap().insert(key, slot);
        (s.wake)();
    }
}

/// file:// URI escaped the way GLib does it, which the thumbnail cache keys on.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~!$&'()*+,/:=@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn cache_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("thumbnails/normal"))
}

/// Cached or freshly made thumbnail: (png path, width, height).
pub fn thumbnail(path: &Path, mtime: i64) -> Option<(PathBuf, u32, u32)> {
    let uri = file_uri(path);
    let dir = cache_dir()?;
    let out = dir.join(format!("{:x}.png", md5::compute(uri.as_bytes())));
    if let Some((w, h)) = valid_cached(&out, mtime) {
        return Some((out, w, h));
    }
    let img = if in_cursor_dir(path) {
        from_cursor(path)?
    } else if IMAGE_EXT.contains(&ext_of(path).as_str()) {
        from_image(path)?
    } else if crate::viewer::audio::EXTS.contains(&ext_of(path).as_str()) {
        // Music: its cover art.
        crate::viewer::audio::thumbnail(path)?
    } else {
        from_video(path)?
    };
    let img = img.thumbnail(SIZE, SIZE).to_rgba8();
    let (w, h) = img.dimensions();
    write_png(&dir, &out, &img, &uri, mtime)?;
    Some((out, w, h))
}

/// A cached thumbnail counts only if it was made from this exact mtime.
fn valid_cached(png_path: &Path, mtime: i64) -> Option<(u32, u32)> {
    let f = std::fs::File::open(png_path).ok()?;
    let reader = png::Decoder::new(std::io::BufReader::new(f)).read_info().ok()?;
    let info = reader.info();
    let ok = info
        .uncompressed_latin1_text
        .iter()
        .any(|t| t.keyword == "Thumb::MTime" && t.text.trim() == mtime.to_string());
    ok.then_some((info.width, info.height))
}

fn from_image(path: &Path) -> Option<image::DynamicImage> {
    use image::ImageDecoder;
    if std::fs::metadata(path).ok()?.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let mut decoder = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Some(img)
}

/// The biggest size's first frame of an X cursor.
fn from_cursor(path: &Path) -> Option<image::DynamicImage> {
    if !crate::xcursor::is_cursor(path) {
        return None;
    }
    let f = crate::xcursor::largest(path)?.into_iter().next()?;
    Some(image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(f.width, f.height, f.rgba)?))
}

fn from_video(path: &Path) -> Option<image::DynamicImage> {
    let tmp = crate::vfs::temp_root().join(format!(
        "thumb-{}-{:?}.png",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(tmp.parent()?).ok()?;
    let ok = std::process::Command::new("ffmpegthumbnailer")
        .arg("-i")
        .arg(path)
        .arg("-o")
        .arg(&tmp)
        .args(["-s", &SIZE.to_string(), "-c", "png"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?
        .success();
    let img = ok.then(|| image::open(&tmp).ok()).flatten();
    let _ = std::fs::remove_file(&tmp);
    img
}

fn write_png(dir: &Path, out: &Path, img: &image::RgbaImage, uri: &str, mtime: i64) -> Option<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).ok()?;
    let tmp = out.with_extension(format!("polka-{}.tmp", std::process::id()));
    let r = (|| -> Option<()> {
        let f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).ok()?;
        let mut enc = png::Encoder::new(BufWriter::new(f), img.width(), img.height());
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.add_text_chunk("Thumb::URI".into(), uri.into()).ok()?;
        enc.add_text_chunk("Thumb::MTime".into(), mtime.to_string()).ok()?;
        enc.add_text_chunk("Software".into(), "Polka".into()).ok()?;
        let mut w = enc.write_header().ok()?;
        w.write_image_data(img.as_raw()).ok()?;
        w.finish().ok()
    })();
    if r.is_some() {
        std::fs::rename(&tmp, out).ok()
    } else {
        let _ = std::fs::remove_file(&tmp);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_escaping_matches_glib() {
        assert_eq!(file_uri(Path::new("/home/a b/ж#1.png")), "file:///home/a%20b/%D0%B6%231.png");
        assert_eq!(file_uri(Path::new("/x/(1)+[2].jpg")), "file:///x/(1)+%5B2%5D.jpg");
    }

    #[test]
    fn makes_and_reuses_thumbnail() {
        let dir = std::env::temp_dir().join(format!("polka-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Point the cache at a temp dir.
        unsafe { std::env::set_var("XDG_CACHE_HOME", dir.join("cache")) };
        let src = dir.join("wide.png");
        image::RgbaImage::from_pixel(400, 100, image::Rgba([200, 30, 30, 255])).save(&src).unwrap();
        let (p, w, h) = thumbnail(&src, 42).unwrap();
        assert_eq!((w, h), (128, 32));
        assert!(p.starts_with(dir.join("cache/thumbnails/normal")));
        assert_eq!(valid_cached(&p, 42), Some((128, 32)));
        assert_eq!(valid_cached(&p, 43), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
