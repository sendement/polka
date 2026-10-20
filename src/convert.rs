//! Converting files in bulk: pictures (iPhone's HEIC too), sound, video
//! (also to sound) and archives (repacked, multi-volume sets as one;
//! password-protected ones aren't touched). Whatever the system has does
//! the work: ImageMagick, ffmpeg, libheif's tools, 7-Zip; plain pictures
//! and archives need nothing but Polka.

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use crate::ops::{ArchiveFormat, Job};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Image,
    Audio,
    Video,
    Archive,
}

pub const GROUPS: [Group; 4] = [Group::Image, Group::Audio, Group::Video, Group::Archive];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Png,
    Jpg,
    Webp,
    Avif,
    Heic,
    Bmp,
    Tiff,
    Gif,
    Mp3,
    M4a,
    Opus,
    Ogg,
    Flac,
    Wav,
    Mp4,
    Mkv,
    Webm,
    Mov,
    Archive(ArchiveFormat),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Quality {
    High,
    #[default]
    Balanced,
    Small,
}

pub const QUALITIES: [Quality; 3] = [Quality::High, Quality::Balanced, Quality::Small];

/// Pictures the image library reads itself.
const IMAGE_OWN: &[&str] = &["png", "jpg", "jpeg", "jpe", "webp", "bmp", "gif", "tif", "tiff", "ico"];
/// Pictures read with an outside tool: iPhone's HEIC, AVIF, JPEG XL.
const IMAGE_TOOL: &[&str] = &["heic", "heif", "hif", "avif", "jxl"];

fn ext(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default()
}

/// What kind of file it is, for converting.
pub fn group_of(name: &str) -> Option<Group> {
    let e = ext(name);
    if IMAGE_OWN.contains(&e.as_str()) || IMAGE_TOOL.contains(&e.as_str()) {
        Some(Group::Image)
    } else if crate::viewer::audio::EXTS.contains(&e.as_str()) {
        Some(Group::Audio)
    } else if crate::viewer::video::EXTS.contains(&e.as_str()) {
        Some(Group::Video)
    } else if crate::vfs::is_archive_name(name) || crate::volumes::parse(name).is_some() {
        Some(Group::Archive)
    } else {
        None
    }
}

impl Target {
    pub fn label(self) -> &'static str {
        match self {
            Target::Png => "PNG",
            Target::Jpg => "JPEG",
            Target::Webp => "WebP",
            Target::Avif => "AVIF",
            Target::Heic => "HEIC",
            Target::Bmp => "BMP",
            Target::Tiff => "TIFF",
            Target::Gif => "GIF",
            Target::Mp3 => "MP3",
            Target::M4a => "M4A (AAC)",
            Target::Opus => "Opus",
            Target::Ogg => "OGG (Vorbis)",
            Target::Flac => "FLAC",
            Target::Wav => "WAV",
            Target::Mp4 => "MP4 (H.264)",
            Target::Mkv => "MKV",
            Target::Webm => "WebM (VP9)",
            Target::Mov => "MOV (H.264)",
            Target::Archive(f) => match f {
                ArchiveFormat::Zip => "ZIP",
                ArchiveFormat::SevenZ => "7z",
                ArchiveFormat::TarZst => "tar.zst",
                ArchiveFormat::TarGz => "tar.gz",
                ArchiveFormat::TarXz => "tar.xz",
            },
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Target::Png => ".png",
            Target::Jpg => ".jpg",
            Target::Webp => ".webp",
            Target::Avif => ".avif",
            Target::Heic => ".heic",
            Target::Bmp => ".bmp",
            Target::Tiff => ".tiff",
            Target::Gif => ".gif",
            Target::Mp3 => ".mp3",
            Target::M4a => ".m4a",
            Target::Opus => ".opus",
            Target::Ogg => ".ogg",
            Target::Flac => ".flac",
            Target::Wav => ".wav",
            Target::Mp4 => ".mp4",
            Target::Mkv => ".mkv",
            Target::Webm => ".webm",
            Target::Mov => ".mov",
            Target::Archive(f) => f.ext(),
        }
    }

    /// Lossy: the quality setting means something.
    pub fn lossy(self) -> bool {
        matches!(self, Target::Jpg | Target::Webp | Target::Avif | Target::Heic | Target::Mp3 | Target::M4a | Target::Opus | Target::Ogg | Target::Mp4 | Target::Webm | Target::Mov)
    }

    /// The same as what the file already is (nothing to do).
    fn same_as(self, name: &str) -> bool {
        let e = ext(name);
        match self {
            Target::Jpg => matches!(e.as_str(), "jpg" | "jpeg" | "jpe"),
            Target::Tiff => matches!(e.as_str(), "tif" | "tiff"),
            Target::Heic => matches!(e.as_str(), "heic" | "heif" | "hif"),
            Target::Archive(_) => name.to_lowercase().ends_with(self.ext()),
            _ => format!(".{e}") == self.ext(),
        }
    }
}

// ---------------------------------------------------------------- tools

static FOUND: Mutex<Vec<(&'static str, bool)>> = Mutex::new(Vec::new());

/// A tool on the system (the host's, from Flatpak).
pub fn has(tool: &'static str) -> bool {
    if cfg!(test) {
        if let Ok(v) = std::env::var("POLKA_CONVERT_TOOLS") {
            return v.split(',').any(|t| t == tool);
        }
    }
    if let Some(&(_, v)) = FOUND.lock().ok().and_then(|f| f.iter().find(|(t, _)| *t == tool).copied()).as_ref() {
        return v;
    }
    let mut c = Command::new("sh");
    c.args(["-c", &format!("command -v {tool}")]);
    let ok = crate::sandbox::host(c).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if let Ok(mut f) = FOUND.lock() {
        f.push((tool, ok));
    }
    ok
}

fn tool(name: &str) -> Command {
    crate::sandbox::host(Command::new(name))
}

fn arg(p: &Path) -> std::ffi::OsString {
    crate::sandbox::host_path(p).into_os_string()
}

/// What a group of files can become, here.
pub fn targets(g: Group) -> Vec<Target> {
    let ff = has("ffmpeg");
    let magick = has("magick");
    match g {
        Group::Image => {
            let mut v = vec![Target::Png, Target::Jpg, Target::Webp];
            if magick || has("avifenc") || ff {
                v.push(Target::Avif);
            }
            if magick || has("heif-enc") {
                v.push(Target::Heic);
            }
            v.extend([Target::Bmp, Target::Tiff, Target::Gif]);
            v
        }
        Group::Audio if ff => vec![Target::Mp3, Target::M4a, Target::Opus, Target::Ogg, Target::Flac, Target::Wav],
        // A video's sound alone, too.
        Group::Video if ff => vec![Target::Mp4, Target::Mkv, Target::Webm, Target::Mov, Target::Mp3, Target::M4a, Target::Opus, Target::Ogg, Target::Flac, Target::Wav],
        Group::Archive => {
            let mut v = vec![Target::Archive(ArchiveFormat::Zip)];
            if crate::ops::have_7z() {
                v.push(Target::Archive(ArchiveFormat::SevenZ));
            }
            v.extend([Target::Archive(ArchiveFormat::TarZst), Target::Archive(ArchiveFormat::TarGz), Target::Archive(ArchiveFormat::TarXz)]);
            v
        }
        _ => Vec::new(),
    }
}

/// Why a file can't be converted (None: it can).
pub fn refusal(path: &Path, name: &str) -> Option<&'static str> {
    let g = group_of(name)?;
    match g {
        Group::Image if IMAGE_TOOL.contains(&ext(name).as_str()) && !(has("magick") || has("heif-dec") || has("ffmpeg")) => Some(crate::i18n::tr("needs ImageMagick or libheif")),
        Group::Audio | Group::Video if !has("ffmpeg") => Some(crate::i18n::tr("needs ffmpeg")),
        Group::Archive if crate::archive_rw::probe_encrypted(&crate::volumes::first(path)).unwrap_or(false) => Some(crate::i18n::tr("password-protected")),
        _ => None,
    }
}

// ---------------------------------------------------------------- the work

/// One file (or a multi-volume set: its first volume) to convert.
#[derive(Clone, Debug)]
pub struct Item {
    pub src: PathBuf,
    pub target: Target,
}

/// Where the converted file goes: beside it, named after it.
pub fn out_path(src: &Path, target: Target) -> PathBuf {
    let dir = src.parent().unwrap_or(Path::new("/"));
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = if group_of(&name) == Some(Group::Archive) {
        let whole = crate::volumes::archive_name(src);
        archive_stem(&whole)
    } else {
        crate::ops::split_ext(&name).0.to_string()
    };
    crate::ops::unique_path(dir, &format!("{stem}{}", target.ext()))
}

/// "photos.tar.gz" → "photos".
fn archive_stem(name: &str) -> String {
    let lower = name.to_lowercase();
    for suffix in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.lz4", ".tar.lzma", ".pkg.tar.zst", ".pkg.tar.xz"] {
        if lower.ends_with(suffix) {
            return name[..name.len() - suffix.len()].to_string();
        }
    }
    crate::ops::split_ext(name).0.to_string()
}

/// Nothing to do: already that (a multi-volume set always has something
/// to do: it becomes one file).
pub fn needless(src: &Path, target: Target) -> bool {
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    crate::volumes::set_of(src).is_none() && target.same_as(&name)
}

/// Convert them, one by one. The originals that worked go to the trash if
/// asked to.
pub fn run(items: &[Item], quality: Quality, trash: bool, job: &Job) -> std::io::Result<()> {
    job.items.store(true, Ordering::Relaxed);
    job.total.store(items.len() as u64, Ordering::Relaxed);
    let mut failed: Vec<String> = Vec::new();
    let mut done: Vec<PathBuf> = Vec::new();
    for it in items {
        if job.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
        }
        job.now_on(&it.src);
        let out = out_path(&it.src, it.target);
        let r = match group_of(&it.src.to_string_lossy()) {
            Some(Group::Image) => image(&it.src, &out, it.target, quality),
            Some(Group::Audio) | Some(Group::Video) => media(&it.src, &out, it.target, quality, job),
            Some(Group::Archive) => archive(&it.src, &out, it.target),
            None => Err("not convertible".into()),
        };
        match r {
            Ok(()) => done.push(it.src.clone()),
            Err(e) => {
                let _ = std::fs::remove_file(&out);
                if job.cancel.load(Ordering::Relaxed) {
                    return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
                }
                let name = it.src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                crate::log::line(format!("converting {}: {e}", it.src.display()));
                failed.push(format!("{name}: {e}"));
            }
        }
        job.done.fetch_add(1, Ordering::Relaxed);
    }
    if trash && !done.is_empty() {
        // A set goes whole.
        let all: Vec<PathBuf> = done.iter().flat_map(|p| crate::volumes::set_of(p).map(|s| s.files).unwrap_or_else(|| vec![p.clone()])).collect();
        trash::delete_all(&all).map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    match failed.as_slice() {
        [] => Ok(()),
        [one] => Err(std::io::Error::other(one.clone())),
        many => Err(std::io::Error::other(format!("{} (+{})", many[0], many.len() - 1))),
    }
}

/// Run a tool; its complaint if it failed.
fn run_tool(mut c: Command) -> Result<(), String> {
    let out = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("failed").trim().to_string())
    }
}

/// A scratch file the host's tools can reach too (from Flatpak, the
/// runtime folder is the sandbox's own; the cache is the user's real one).
fn scratch(name: &str) -> PathBuf {
    let d = dirs::cache_dir().map(|c| c.join("polka/convert")).unwrap_or_else(crate::vfs::temp_root);
    let _ = std::fs::create_dir_all(&d);
    d.join(format!("convert-{}-{:?}-{name}", std::process::id(), std::thread::current().id()))
}

// ---------------------------------------------------------------- pictures

fn image(src: &Path, out: &Path, t: Target, q: Quality) -> Result<(), String> {
    let quality = match q {
        Quality::High => 92,
        Quality::Balanced => 82,
        Quality::Small => 65,
    };
    // ImageMagick reads and writes them all, turned as the camera said.
    if has("magick") {
        let mut c = tool("magick");
        let mut first = arg(src);
        first.push("[0]");
        c.arg(first).arg("-auto-orient");
        if t.lossy() {
            c.args(["-quality", &quality.to_string()]);
        }
        return run_tool({
            c.arg(arg(out));
            c
        });
    }
    let img = decode(src)?;
    match t {
        Target::Png | Target::Bmp | Target::Tiff | Target::Gif => {
            let fmt = match t {
                Target::Png => image::ImageFormat::Png,
                Target::Bmp => image::ImageFormat::Bmp,
                Target::Tiff => image::ImageFormat::Tiff,
                _ => image::ImageFormat::Gif,
            };
            let img = if t == Target::Bmp { image::DynamicImage::ImageRgb8(img.to_rgb8()) } else { img };
            img.save_with_format(out, fmt).map_err(|e| e.to_string())
        }
        Target::Jpg => {
            let f = std::fs::File::create(out).map_err(|e| e.to_string())?;
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(f), quality);
            enc.encode_image(&image::DynamicImage::ImageRgb8(img.to_rgb8())).map_err(|e| e.to_string())
        }
        // The rest through a PNG for the tool that writes them.
        _ => {
            let tmp = scratch("in.png");
            img.save_with_format(&tmp, image::ImageFormat::Png).map_err(|e| e.to_string())?;
            let r = encode_with_tool(&tmp, out, t, quality);
            let _ = std::fs::remove_file(&tmp);
            r
        }
    }
}

fn encode_with_tool(png: &Path, out: &Path, t: Target, quality: u8) -> Result<(), String> {
    match t {
        Target::Webp if has("ffmpeg") => {
            let mut c = tool("ffmpeg");
            c.args(["-nostdin", "-v", "error", "-y", "-i"]).arg(arg(png)).args(["-c:v", "libwebp", "-quality", &quality.to_string()]).arg(arg(out));
            run_tool(c)
        }
        // No tool: lossless, which the library writes itself.
        Target::Webp => image::open(png).and_then(|i| i.save_with_format(out, image::ImageFormat::WebP)).map_err(|e| e.to_string()),
        Target::Avif if has("avifenc") => {
            let mut c = tool("avifenc");
            c.args(["-q", &(quality.saturating_sub(20)).to_string()]).arg(arg(png)).arg(arg(out));
            run_tool(c)
        }
        Target::Avif => {
            let crf = 63 - (quality as u32 * 63 / 100).min(63);
            let mut c = tool("ffmpeg");
            c.args(["-nostdin", "-v", "error", "-y", "-i"]).arg(arg(png)).args(["-c:v", "libaom-av1", "-still-picture", "1", "-crf", &crf.to_string()]).arg(arg(out));
            run_tool(c)
        }
        Target::Heic => {
            let mut c = tool("heif-enc");
            c.args(["-q", &quality.to_string()]).arg(arg(png)).arg("-o").arg(arg(out));
            run_tool(c)
        }
        _ => Err("unsupported".into()),
    }
}

/// A picture as pixels: the library's own formats directly (turned as the
/// camera said), HEIC and the like through a tool.
pub fn decode(src: &Path) -> Result<image::DynamicImage, String> {
    let name = src.to_string_lossy();
    if !IMAGE_TOOL.contains(&ext(&name).as_str()) {
        use image::ImageDecoder;
        let mut dec = image::ImageReader::open(src).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?.into_decoder().map_err(|e| e.to_string())?;
        let o = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
        let mut img = image::DynamicImage::from_decoder(dec).map_err(|e| e.to_string())?;
        img.apply_orientation(o);
        return Ok(img);
    }
    let tmp = scratch("out.png");
    let _ = std::fs::remove_file(&tmp);
    let tries: Vec<Command> = {
        let mut v = Vec::new();
        if has("magick") {
            let mut c = tool("magick");
            let mut first = arg(src);
            first.push("[0]");
            c.arg(first).arg("-auto-orient").arg(arg(&tmp));
            v.push(c);
        }
        if has("heif-dec") {
            let mut c = tool("heif-dec");
            c.arg(arg(src)).arg(arg(&tmp));
            v.push(c);
        }
        if has("ffmpeg") {
            let mut c = tool("ffmpeg");
            c.args(["-nostdin", "-v", "error", "-y", "-i"]).arg(arg(src)).args(["-frames:v", "1"]).arg(arg(&tmp));
            v.push(c);
        }
        v
    };
    let mut last = "needs ImageMagick or libheif".to_string();
    for c in tries {
        match run_tool(c) {
            Ok(()) if tmp.exists() => {
                let img = image::open(&tmp).map_err(|e| e.to_string());
                let _ = std::fs::remove_file(&tmp);
                return img;
            }
            Ok(()) => {}
            Err(e) => last = e,
        }
    }
    Err(last)
}

// ---------------------------------------------------------------- sound and video

fn media(src: &Path, out: &Path, t: Target, q: Quality, job: &Job) -> Result<(), String> {
    let pick = |h: &'static str, b: &'static str, s: &'static str| match q {
        Quality::High => h,
        Quality::Balanced => b,
        Quality::Small => s,
    };
    let mut c = tool("ffmpeg");
    c.args(["-nostdin", "-v", "error", "-y", "-progress", "pipe:1", "-nostats", "-i"]).arg(arg(src));
    let audio_only = !matches!(t, Target::Mp4 | Target::Mkv | Target::Webm | Target::Mov);
    if audio_only {
        c.args(["-vn", "-sn", "-map_metadata", "0"]);
    }
    match t {
        Target::Mp3 => c.args(["-c:a", "libmp3lame", "-q:a", pick("0", "2", "5")]),
        Target::M4a => c.args(["-c:a", "aac", "-b:a", pick("256k", "192k", "128k")]),
        Target::Opus => c.args(["-c:a", "libopus", "-b:a", pick("192k", "128k", "80k")]),
        Target::Ogg => c.args(["-c:a", "libvorbis", "-q:a", pick("8", "5", "3")]),
        Target::Flac => c.args(["-c:a", "flac"]),
        Target::Wav => c.args(["-c:a", "pcm_s16le"]),
        // Only the container changes: quick, and nothing lost.
        Target::Mkv => c.args(["-map", "0", "-c", "copy"]),
        Target::Webm => c.args(["-map", "0:v:0?", "-map", "0:a?", "-c:v", "libvpx-vp9", "-crf", pick("24", "31", "38"), "-b:v", "0", "-row-mt", "1", "-c:a", "libopus", "-b:a", "128k"]),
        // Plays anywhere: H.264, 8-bit 4:2:0 (iPhone's HDR HEVC too), AAC.
        _ => c.args(["-map", "0:v:0?", "-map", "0:a?", "-c:v", "libx264", "-preset", "medium", "-crf", pick("18", "22", "27"), "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"]),
    };
    c.arg(arg(out));
    let total = duration(src);
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut child = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let mut err_pipe = child.stderr.take();
    let err_text = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = err_pipe.as_mut() {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    // How far it is, from ffmpeg's progress lines.
    if let Some(o) = child.stdout.take() {
        for line in std::io::BufReader::new(o).lines().map_while(Result::ok) {
            if job.cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                break;
            }
            if let (Some(us), Some(total)) = (line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()), total) {
                let pct = (us / 1e6 / total * 100.0).clamp(0.0, 100.0);
                if let Ok(mut cur) = job.current.try_lock() {
                    *cur = format!("{name} · {pct:.0}%");
                }
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let err = err_text.join().unwrap_or_default();
    if job.cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    if status.success() {
        Ok(())
    } else {
        Err(err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("ffmpeg failed").trim().to_string())
    }
}

/// Seconds, from ffprobe.
fn duration(src: &Path) -> Option<f64> {
    let mut c = tool("ffprobe");
    c.args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"]).arg(arg(src));
    let out = c.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok().filter(|d: &f64| *d > 0.0)
}

// ---------------------------------------------------------------- archives

/// Unpacked into a scratch folder, packed again as `t` (a multi-volume set
/// comes out as one file).
fn archive(src: &Path, out: &Path, t: Target) -> Result<(), String> {
    let Target::Archive(format) = t else { return Err("not an archive format".into()) };
    let first = crate::volumes::first(src);
    let stage = scratch("stage");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let r = (|| {
        crate::vfs::extract(&first, &[String::new()], &stage, |_| true).map_err(|e| e.to_string())?;
        let mut sources: Vec<PathBuf> = std::fs::read_dir(&stage).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
        sources.sort();
        if sources.is_empty() {
            return Err("empty archive".to_string());
        }
        let scratch_job = Job::detached();
        crate::ops::compress(&sources, out, &crate::ops::CompressOpts::new(format), &scratch_job).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_dir_all(&stage);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_names() {
        assert_eq!(group_of("IMG_0001.HEIC"), Some(Group::Image));
        assert_eq!(group_of("clip.MOV"), Some(Group::Video));
        assert_eq!(group_of("voice.m4a"), Some(Group::Audio));
        assert_eq!(group_of("pack.part2.rar"), Some(Group::Archive));
        assert_eq!(group_of("notes.txt"), None);
        assert!(Target::Jpg.same_as("a.JPEG") && Target::Png.same_as("x.png") && !Target::Png.same_as("x.webp"));
        assert!(Target::Archive(ArchiveFormat::TarGz).same_as("a.tar.gz"));
        assert_eq!(archive_stem("photos.tar.gz"), "photos");
        assert_eq!(archive_stem("box.7z"), "box");
        let d = std::env::temp_dir();
        assert_eq!(out_path(&d.join("polka-none-IMG_1.heic"), Target::Jpg), d.join("polka-none-IMG_1.jpg"));
    }
}
