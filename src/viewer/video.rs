//! Video: ffprobe reads the streams, ffmpeg decodes frames (scaled, with
//! subtitles burnt in) on a thread, the sound plays through the music
//! engine and the picture follows its clock.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};

use super::audio::Playback;
use crate::i18n::tr;

pub const EXTS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "avi", "mov", "wmv", "flv", "mpg", "mpeg", "ts", "m2ts", "mts", "3gp", "ogv", "vob", "divx", "asf",
];

pub fn is_video(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.rsplit_once('.').is_some_and(|(_, e)| EXTS.contains(&e))
}

/// Largest size frames are decoded at (bigger ones are scaled down).
const MAX_W: u32 = 1920;
const MAX_H: u32 = 1080;

/// A stream to choose: its number among streams of its type, and a label.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub n: usize,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Subs {
    Off,
    /// Text subtitles inside the file (number among all its subtitle streams).
    Inside(Track),
    /// A .srt/.ass/.vtt next to the video.
    File(PathBuf),
}

impl Subs {
    pub fn label(&self) -> String {
        match self {
            Subs::Off => tr("off").to_string(),
            Subs::Inside(t) => t.label.clone(),
            Subs::File(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Probe {
    /// Display size (pixel aspect and rotation applied).
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: Option<f64>,
    pub codec: String,
    pub has_video: bool,
    pub audio: Vec<Track>,
    pub subs: Vec<Track>,
}

/// Language and title of a stream, else its codec.
fn stream_label(s: &serde_json::Value, n: usize) -> String {
    let tags = &s["tags"];
    let mut parts: Vec<String> = Vec::new();
    if let Some(l) = tags["language"].as_str().filter(|l| *l != "und") {
        parts.push(l.to_string());
    }
    if let Some(t) = tags["title"].as_str() {
        parts.push(t.to_string());
    }
    if parts.is_empty() {
        parts.push(format!("#{}", n + 1));
    }
    if let Some(c) = s["codec_name"].as_str() {
        parts.push(format!("({c})"));
    }
    parts.join(" ")
}

fn ratio(s: Option<&str>) -> Option<f64> {
    let (a, b) = s?.split_once(['/', ':'])?;
    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
    (a > 0.0 && b > 0.0).then_some(a / b)
}

pub fn probe(path: &Path) -> Result<Probe, String> {
    let out = std::process::Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_streams", "-show_format"])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|_| tr("Videos need ffmpeg").to_string())?;
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let streams = j["streams"].as_array().cloned().unwrap_or_default();
    if streams.is_empty() {
        return Err(tr("Can't play this file").into());
    }
    let mut p = Probe::default();
    let (mut na, mut ns) = (0, 0);
    for s in &streams {
        match s["codec_type"].as_str() {
            Some("video") if !p.has_video && s["disposition"]["attached_pic"].as_i64() != Some(1) => {
                p.has_video = true;
                let (w, h) = (s["width"].as_u64().unwrap_or(0) as f64, s["height"].as_u64().unwrap_or(0) as f64);
                let sar = ratio(s["sample_aspect_ratio"].as_str()).unwrap_or(1.0);
                let rot = s["side_data_list"]
                    .as_array()
                    .and_then(|l| l.iter().find_map(|d| d["rotation"].as_f64()))
                    .or_else(|| s["tags"]["rotate"].as_str().and_then(|r| r.parse().ok()))
                    .unwrap_or(0.0);
                let (mut dw, mut dh) = (w * sar, h);
                if (rot.abs() - 90.0).abs() < 1.0 {
                    std::mem::swap(&mut dw, &mut dh);
                }
                p.width = dw.round() as u32;
                p.height = dh.round() as u32;
                p.fps = ratio(s["avg_frame_rate"].as_str()).or_else(|| ratio(s["r_frame_rate"].as_str())).unwrap_or(25.0).clamp(1.0, 60.0);
                p.codec = s["codec_name"].as_str().unwrap_or("").to_uppercase();
            }
            Some("audio") => {
                p.audio.push(Track { n: na, label: stream_label(s, na) });
                na += 1;
            }
            Some("subtitle") => {
                // Pictures (PGS, DVD) can't go through libass.
                let text = matches!(s["codec_name"].as_str(), Some("subrip" | "ass" | "ssa" | "webvtt" | "mov_text" | "text"));
                if text {
                    p.subs.push(Track { n: ns, label: stream_label(s, ns) });
                }
                ns += 1;
            }
            _ => {}
        }
    }
    p.duration = j["format"]["duration"].as_str().and_then(|d| d.parse().ok()).filter(|d: &f64| *d > 0.0);
    Ok(p)
}

/// Subtitle files next to the video: same name, any suffix (`film.ru.srt`).
pub fn subtitle_files(path: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem().map(|s| s.to_string_lossy().to_lowercase())) else { return Vec::new() };
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            n.starts_with(&stem) && [".srt", ".ass", ".ssa", ".vtt"].iter().any(|e| n.ends_with(e))
        })
        .collect();
    v.sort();
    v
}

/// Fit the display size into the decode limit; even numbers.
fn decode_size(p: &Probe) -> (u32, u32) {
    let (w, h) = (p.width.max(2) as f64, p.height.max(2) as f64);
    let k = (MAX_W as f64 / w).min(MAX_H as f64 / h).min(1.0);
    let even = |x: f64| ((x * k / 2.0).round() as u32).max(1) * 2;
    (even(w), even(h))
}

// ------------------------------------------------------------ frames

pub struct Frame {
    /// Seconds into the file.
    pub ts: f64,
    pub image: egui::ColorImage,
}

/// The decoding ffmpeg and the frames it sends.
struct Decoder {
    child: std::process::Child,
    rx: Receiver<Frame>,
    _link: Option<PathBuf>,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(l) = &self._link {
            let _ = std::fs::remove_file(l);
        }
    }
}

/// libass reads its file from a filter argument: give it a plain name.
fn plain_link(target: &Path, ext: &str) -> Option<PathBuf> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = crate::vfs::temp_root();
    std::fs::create_dir_all(&dir).ok()?;
    let link = dir.join(format!("subs-{}-{}.{ext}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(target, &link).ok()?;
    let plain = link.to_string_lossy().chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c));
    plain.then_some(link)
}

impl Decoder {
    /// `hw`: GPU decoding — worth its ~0.4 s start (paid on every seek)
    /// only for videos bigger than full HD.
    fn spawn(path: &Path, from: f64, size: (u32, u32), fps: f64, subs: &Subs, hw: bool, ctx: &egui::Context) -> Result<Self, String> {
        let (w, h) = size;
        let mut vf = vec![format!("scale={w}:{h}:flags=bilinear")];
        let mut link = None;
        let sub_filter = match subs {
            Subs::Off => None,
            Subs::Inside(t) => {
                let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "mkv".into());
                link = plain_link(path, &ext);
                link.as_ref().map(|l| format!("subtitles=filename={}:si={}", l.display(), t.n))
            }
            Subs::File(f) => {
                let ext = f.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "srt".into());
                link = plain_link(f, &ext);
                link.as_ref().map(|l| format!("subtitles=filename={}", l.display()))
            }
        };
        if let Some(sf) = sub_filter {
            // Seeking restarts timestamps at 0; libass needs the real ones.
            vf.insert(0, format!("setpts=PTS+{from:.3}/TB"));
            vf.push(sf);
            vf.push("setpts=PTS-STARTPTS".into());
        }
        vf.push(format!("fps={fps:.5}"));
        vf.push("format=rgb24".into());
        let mut child = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-v", "quiet", "-hwaccel", if hw { "auto" } else { "none" }, "-ss"])
            .arg(format!("{from:.3}"))
            .arg("-i")
            .arg(path)
            .args(["-map", "0:v:0", "-an", "-sn", "-vf"])
            .arg(vf.join(","))
            .args(["-f", "rawvideo", "pipe:1"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|_| tr("Videos need ffmpeg").to_string())?;
        let mut out = child.stdout.take().ok_or("ffmpeg")?;
        let (tx, rx) = crossbeam_channel::bounded(3);
        let c = ctx.clone();
        std::thread::Builder::new()
            .name("polka-video".into())
            .spawn(move || {
                let mut buf = vec![0u8; (w * h * 3) as usize];
                let mut k = 0u64;
                while out.read_exact(&mut buf).is_ok() {
                    let image = egui::ColorImage::from_rgb([w as usize, h as usize], &buf);
                    if tx.send(Frame { ts: from + k as f64 / fps, image }).is_err() {
                        return;
                    }
                    k += 1;
                    c.request_repaint();
                }
                c.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        Ok(Decoder { child, rx, _link: link })
    }
}

// ------------------------------------------------------------ state

pub struct VideoState {
    pub tracks: Vec<PathBuf>,
    pub index: usize,
    pub probe: Probe,
    pub error: Option<String>,
    decoder: Option<Decoder>,
    /// The next frame, not yet due.
    pending: Option<Frame>,
    pub texture: Option<egui::TextureHandle>,
    pub audio: Option<Playback>,
    pub audio_track: usize,
    pub subs: Subs,
    pub sub_files: Vec<PathBuf>,
    /// Clock without sound: position when it last (re)started, and since when.
    base: f64,
    since: Option<Instant>,
    paused: bool,
    /// Decoder ran out of frames.
    video_done: bool,
    pub full: bool,
    /// Controls show until then (and while paused).
    pub controls_until: Instant,
    volume: f32,
    /// Frames put on screen, and how late the last one was (seconds).
    pub shown: u64,
    pub late: f64,
}

impl VideoState {
    pub fn open(path: PathBuf, siblings: Vec<PathBuf>, volume: f32, ctx: &egui::Context) -> Self {
        let tracks = if siblings.contains(&path) { siblings } else { vec![path.clone()] };
        let index = tracks.iter().position(|p| *p == path).unwrap_or(0);
        let mut v = VideoState {
            tracks,
            index,
            probe: Probe::default(),
            error: None,
            decoder: None,
            pending: None,
            texture: None,
            audio: None,
            audio_track: 0,
            subs: Subs::Off,
            sub_files: Vec::new(),
            base: 0.0,
            since: None,
            paused: false,
            video_done: false,
            full: false,
            controls_until: Instant::now() + Duration::from_secs(3),
            volume,
            shown: 0,
            late: 0.0,
        };
        v.load(ctx);
        v
    }

    pub fn path(&self) -> &Path {
        &self.tracks[self.index]
    }

    /// Open the current file from the start.
    fn load(&mut self, ctx: &egui::Context) {
        self.decoder = None;
        self.audio = None;
        self.pending = None;
        self.texture = None;
        self.error = None;
        self.audio_track = 0;
        self.paused = false;
        self.sub_files = subtitle_files(self.path());
        match probe(self.path()) {
            Ok(p) => {
                // Subtitles on by default when the file brings its own.
                self.subs = match (p.subs.first(), self.sub_files.first()) {
                    (_, Some(f)) => Subs::File(f.clone()),
                    (Some(t), None) => Subs::Inside(t.clone()),
                    _ => Subs::Off,
                };
                self.probe = p;
                self.restart(0.0, ctx);
            }
            Err(e) => {
                self.probe = Probe::default();
                self.error = Some(e);
            }
        }
    }

    /// (Re)start decoding and sound at `at` seconds.
    fn restart(&mut self, at: f64, ctx: &egui::Context) {
        let at = self.probe.duration.map_or(at, |d| at.min((d - 0.25).max(0.0))).max(0.0);
        self.pending = None;
        self.video_done = !self.probe.has_video;
        self.decoder = None;
        if self.probe.has_video {
            let hw = self.probe.width * self.probe.height > MAX_W * MAX_H;
            match Decoder::spawn(self.path(), at, decode_size(&self.probe), self.probe.fps, &self.subs, hw, ctx) {
                Ok(d) => self.decoder = Some(d),
                Err(e) => self.error = Some(e),
            }
        }
        self.audio = None;
        if !self.probe.audio.is_empty() {
            let track = self.audio_track.min(self.probe.audio.len() - 1);
            self.audio = Playback::start_at(self.path(), Some(track), Duration::from_secs_f64(at), self.paused, self.volume).ok();
        }
        self.base = at;
        self.since = (!self.paused).then(Instant::now);
    }

    /// Seconds into the file: the sound's clock, else the wall clock.
    pub fn pos(&self) -> f64 {
        if let Some(a) = &self.audio {
            return a.pos().as_secs_f64();
        }
        let p = self.base + self.since.map_or(0.0, |s| s.elapsed().as_secs_f64());
        self.probe.duration.map_or(p, |d| p.min(d))
    }

    pub fn duration(&self) -> Option<f64> {
        self.probe.duration
    }

    pub fn ended(&self) -> bool {
        self.video_done && self.audio.as_ref().is_none_or(|a| a.ended())
    }

    pub fn playing(&self) -> bool {
        !self.paused && !self.ended() && self.error.is_none()
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn toggle(&mut self, ctx: &egui::Context) {
        if self.ended() {
            self.paused = false;
            return self.restart(0.0, ctx);
        }
        if self.paused {
            self.resume();
        } else {
            self.pause();
        }
    }

    pub fn pause(&mut self) {
        if self.paused {
            return;
        }
        self.base = self.pos();
        self.since = None;
        self.paused = true;
        if let Some(a) = &mut self.audio {
            a.pause();
        }
    }

    fn resume(&mut self) {
        self.paused = false;
        self.since = Some(Instant::now());
        if let Some(a) = &mut self.audio {
            a.resume();
        }
    }

    pub fn seek(&mut self, to: f64, ctx: &egui::Context) {
        self.restart(to, ctx);
    }

    pub fn seek_by(&mut self, secs: f64, ctx: &egui::Context) {
        let to = self.pos() + secs;
        self.seek(to.max(0.0), ctx);
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v;
        if let Some(a) = &mut self.audio {
            a.set_volume(v);
        }
    }

    /// Next audio track (wrapping); its label.
    pub fn cycle_audio(&mut self, ctx: &egui::Context) -> Option<String> {
        let n = self.probe.audio.len();
        if n < 2 {
            return None;
        }
        self.audio_track = (self.audio_track + 1) % n;
        let at = self.pos();
        self.restart(at, ctx);
        Some(self.probe.audio[self.audio_track].label.clone())
    }

    /// Every subtitle choice, "off" first.
    pub fn sub_choices(&self) -> Vec<Subs> {
        let mut v = vec![Subs::Off];
        v.extend(self.probe.subs.iter().cloned().map(Subs::Inside));
        v.extend(self.sub_files.iter().cloned().map(Subs::File));
        v
    }

    /// Next subtitles (wrapping through "off"); their label.
    pub fn cycle_subs(&mut self, ctx: &egui::Context) -> Option<String> {
        let all = self.sub_choices();
        if all.len() < 2 {
            return None;
        }
        let i = all.iter().position(|s| *s == self.subs).unwrap_or(0);
        self.subs = all[(i + 1) % all.len()].clone();
        let at = self.pos();
        self.restart(at, ctx);
        Some(self.subs.label())
    }

    /// Previous / next video of the folder.
    pub fn step(&mut self, d: isize, ctx: &egui::Context) -> bool {
        let n = self.tracks.len() as isize;
        if n < 2 {
            return false;
        }
        self.index = (self.index as isize + d).rem_euclid(n) as usize;
        self.load(ctx);
        true
    }

    pub fn show_controls(&mut self) {
        self.controls_until = Instant::now() + Duration::from_secs(3);
    }

    pub fn controls_shown(&self) -> bool {
        self.paused || self.ended() || Instant::now() < self.controls_until
    }

    /// Per frame: put the due frame on the texture; stop in hidden tabs.
    pub fn tick(&mut self, visible: bool, ctx: &egui::Context) {
        if !visible {
            self.full = false;
            if self.playing() {
                self.pause();
            }
        }
        // Sound that can't be decoded: go on with the wall clock.
        if self.audio.as_ref().is_some_and(|a| a.broken()) {
            self.base = self.pos();
            self.since = (!self.paused).then(Instant::now);
            self.audio = None;
        }
        let Some(dec) = &self.decoder else { return };
        let clock = self.pos() + 0.005;
        let mut due = None;
        loop {
            let f = match self.pending.take() {
                Some(f) => f,
                None => match dec.rx.try_recv() {
                    Ok(f) => f,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.video_done = true;
                        break;
                    }
                },
            };
            // The first frame after a restart shows even when early (paused).
            if f.ts <= clock || (self.texture.is_none() && due.is_none()) {
                due = Some(f);
            } else {
                self.pending = Some(f);
                break;
            }
        }
        if let Some(f) = due {
            self.shown += 1;
            self.late = clock - f.ts;
            match &mut self.texture {
                Some(t) => t.set(f.image, egui::TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture(format!("video:{}", self.path().display()), f.image, egui::TextureOptions::LINEAR)),
            }
        }
        if self.video_done && self.audio.is_none() && !self.paused {
            // No sound: hold the clock at the end.
            self.base = self.pos();
            self.since = None;
            self.paused = true;
        }
        if self.playing() {
            let wait = self.pending.as_ref().map_or(1.0 / self.probe.fps.max(1.0), |f| (f.ts - self.pos()).max(0.0));
            ctx.request_repaint_after(Duration::from_secs_f64(wait.min(0.1)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ffmpeg(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new("ffmpeg").args(["-v", "quiet", "-y"]).args(args).current_dir(dir).status().is_ok_and(|s| s.success())
    }

    #[test]
    fn plays_seeks_tracks_and_subtitles() {
        let dir = std::env::temp_dir().join(format!("polka-video-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 4 s, 160×90 (anamorphic 2:1 pixels → 320×90 on screen), two audio
        // tracks, one text subtitle stream, and a .srt beside it.
        std::fs::write(dir.join("s.srt"), "1\n00:00:00,000 --> 00:00:04,000\nПривет\n").unwrap();
        let ok = ffmpeg(
            &dir,
            &[
                "-f", "lavfi", "-i", "testsrc=size=160x90:rate=25:duration=4", "-f", "lavfi", "-i", "sine=f=440:d=4", "-f", "lavfi", "-i", "sine=f=660:d=4",
                "-i", "s.srt", "-map", "0", "-map", "1", "-map", "2", "-map", "3", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-c:s", "srt",
                "-aspect", "32:9", "-metadata:s:a:1", "language=rus", "v.mkv",
            ],
        );
        if !ok {
            std::fs::remove_dir_all(&dir).unwrap();
            return; // no ffmpeg with x264
        }
        std::fs::rename(dir.join("s.srt"), dir.join("v.ru.srt")).unwrap();
        let p = probe(&dir.join("v.mkv")).unwrap();
        assert!(p.has_video && p.width == 320 && p.height == 90, "{p:?}");
        assert!((p.fps - 25.0).abs() < 0.01);
        assert_eq!(p.audio.len(), 2);
        assert!(p.audio[1].label.starts_with("rus"));
        assert_eq!(p.subs.len(), 1);
        assert!(p.duration.is_some_and(|d| (d - 4.0).abs() < 0.2));
        assert_eq!(decode_size(&p), (320, 90));

        let ctx = egui::Context::default();
        let mut v = VideoState::open(dir.join("v.mkv"), Vec::new(), 1.0, &ctx);
        assert_eq!(v.subs, Subs::File(dir.join("v.ru.srt")), "a subtitle file is picked first");
        let run = |v: &mut VideoState, ms: u64| {
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(ms) {
                v.tick(true, &ctx);
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        run(&mut v, 700);
        assert!(v.texture.is_some(), "frames arrive: err {:?} dec {} done {} subs {:?}", v.error, v.decoder.is_some(), v.video_done, v.subs);
        assert!(v.pos() > 0.3, "clock runs: {}", v.pos());
        v.seek(2.0, &ctx);
        run(&mut v, 300);
        assert!(v.pos() >= 2.0);
        v.pause();
        let at = v.pos();
        run(&mut v, 200);
        assert!((v.pos() - at).abs() < 0.05, "paused");
        assert_eq!(v.cycle_audio(&ctx).as_deref(), Some(v.probe.audio[1].label.as_str()));
        assert!((v.pos() - at).abs() < 0.3, "a track switch keeps the place");
        // off → inside → file → off…
        assert_eq!(v.sub_choices().len(), 3);
        v.cycle_subs(&ctx);
        assert_eq!(v.subs, Subs::Off);
        v.cycle_subs(&ctx);
        assert!(matches!(v.subs, Subs::Inside(_)));
        v.toggle(&ctx);
        run(&mut v, 2500);
        assert!(v.ended(), "plays to the end: {}", v.pos());
        // Hidden tabs pause.
        v.toggle(&ctx);
        run(&mut v, 100);
        v.tick(false, &ctx);
        assert!(v.paused());
        drop(v);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Full HD keeps up: `cargo test full_hd -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn full_hd_keeps_up() {
        let dir = std::env::temp_dir().join(format!("polka-hd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(ffmpeg(&dir, &["-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30:duration=8", "-f", "lavfi", "-i", "sine=d=8", "-c:v", "libx264", "-preset", "veryfast", "-c:a", "aac", "hd.mp4"]));
        let ctx = egui::Context::default();
        let mut v = VideoState::open(dir.join("hd.mp4"), Vec::new(), 1.0, &ctx);
        let t = Instant::now();
        let mut worst: f64 = 0.0;
        while t.elapsed() < Duration::from_secs(4) {
            v.tick(true, &ctx);
            if v.shown > 5 {
                worst = worst.max(v.late);
            }
            std::thread::sleep(Duration::from_millis(16));
        }
        println!("shown {} frames in 4 s at pos {:.2}, worst lateness {:.3}s", v.shown, v.pos(), worst);
        assert!(v.shown >= 100, "{} frames", v.shown);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names() {
        assert!(is_video("Фильм.MKV") && !is_video("a.mp3"));
    }
}
