//! Music: playback through rodio (Symphonia decoders, or ffmpeg for what
//! Symphonia can't read, e.g. Opus), tags and cover art through lofty, and
//! the folder's tracks as a playlist.

use std::io::Read;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use rodio::{Player, Source};

pub const EXTS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "m4a", "m4b", "aac", "alac", "wav", "wave", "aif", "aiff", "aifc", "caf", "mka", "wma", "ape", "wv",
    "mpc", "mp2", "spx", "tta", "dsf", "dff",
];

pub fn is_audio(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.rsplit_once('.').is_some_and(|(_, e)| EXTS.contains(&e))
}

/// Tests (and `POLKA_NO_AUDIO=1`) play into a silent sink running in real
/// time, so nothing reaches the speakers.
fn silent_mode() -> bool {
    cfg!(test) || std::env::var_os("POLKA_NO_AUDIO").is_some_and(|v| v == "1")
}

// ------------------------------------------------------------ output

/// The sound device, open while some player uses it.
struct Output {
    mixer: rodio::mixer::Mixer,
    /// Dropped with the last player: the device thread then closes it.
    _alive: crossbeam_channel::Sender<()>,
}

fn device() -> Result<Arc<Output>, String> {
    static CUR: Mutex<Weak<Output>> = Mutex::new(Weak::new());
    let mut cur = CUR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(o) = cur.upgrade() {
        return Ok(o);
    }
    let (mtx, mrx) = crossbeam_channel::bounded(1);
    let (alive, keep) = crossbeam_channel::bounded::<()>(0);
    // The stream handle stays on its own thread; only the mixer travels.
    std::thread::Builder::new()
        .name("polka-audio".into())
        .spawn(move || match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(mut sink) => {
                sink.log_on_drop(false);
                let _ = mtx.send(Ok(sink.mixer().clone()));
                let _ = keep.recv();
            }
            Err(e) => {
                let _ = mtx.send(Err(e.to_string()));
            }
        })
        .map_err(|e| e.to_string())?;
    let mixer = mrx.recv().map_err(|e| e.to_string())??;
    let o = Arc::new(Output { mixer, _alive: alive });
    *cur = Arc::downgrade(&o);
    Ok(o)
}

/// Stops a silent player's thread when dropped.
struct NullGuard(Arc<std::sync::atomic::AtomicBool>);

impl Drop for NullGuard {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A player whose output is pulled in real time (48 kHz stereo after the
/// mixer converts it) and thrown away.
fn silent_player() -> (Player, NullGuard) {
    let (ch, rate) = (NonZero::new(2).unwrap(), NonZero::new(FF_RATE).unwrap());
    let (mixer, mut out) = rodio::mixer::mixer(ch, rate);
    let player = Player::connect_new(&mixer);
    let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = alive.clone();
    std::thread::Builder::new()
        .name("polka-audio-null".into())
        .spawn(move || {
            const CHUNK: usize = 960;
            while flag.load(std::sync::atomic::Ordering::Relaxed) {
                for _ in 0..CHUNK {
                    out.next();
                }
                std::thread::sleep(Duration::from_secs_f64(CHUNK as f64 / (2.0 * FF_RATE as f64)));
            }
        })
        .ok();
    (player, NullGuard(alive))
}

// ------------------------------------------------------------ ffmpeg

const FF_RATE: u32 = 48_000;

/// Decoding through the ffmpeg program: interleaved stereo f32.
struct Ffmpeg {
    child: std::process::Child,
    out: std::io::BufReader<std::process::ChildStdout>,
}

impl Ffmpeg {
    /// `stream`: which audio stream of the file (videos may have several).
    fn spawn(path: &Path, from: Duration, stream: Option<usize>) -> Result<Self, String> {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args(["-nostdin", "-v", "quiet", "-ss"]).arg(format!("{:.3}", from.as_secs_f64())).arg("-i").arg(path);
        if let Some(n) = stream {
            cmd.args(["-map", &format!("0:a:{n}")]);
        }
        let mut child = cmd
            .args(["-vn", "-sn", "-f", "f32le", "-ac", "2", "-ar"])
            .arg(FF_RATE.to_string())
            .arg("pipe:1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|_| crate::i18n::tr("This format needs ffmpeg").to_string())?;
        let out = std::io::BufReader::with_capacity(1 << 16, child.stdout.take().ok_or("ffmpeg")?);
        Ok(Ffmpeg { child, out })
    }
}

impl Iterator for Ffmpeg {
    type Item = rodio::Sample;
    fn next(&mut self) -> Option<rodio::Sample> {
        let mut b = [0u8; 4];
        self.out.read_exact(&mut b).ok()?;
        Some(f32::from_le_bytes(b) as rodio::Sample)
    }
}

impl Source for Ffmpeg {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(2).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        NonZero::new(FF_RATE).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

impl Drop for Ffmpeg {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ------------------------------------------------------------ playback

/// One track playing (or paused) on the device.
pub struct Playback {
    out: Option<Arc<Output>>,
    player: Player,
    _null: Option<NullGuard>,
    path: PathBuf,
    /// Decoded by ffmpeg: seeking restarts it at `base`.
    ffmpeg: bool,
    /// Audio stream of a video (always through ffmpeg).
    stream: Option<usize>,
    base: Duration,
    /// Length as the decoder knows it.
    pub length: Option<Duration>,
    /// Why there's no sound (no device), while it still "plays".
    pub no_device: Option<String>,
    volume: f32,
    started: Instant,
}

impl Playback {
    pub fn start(path: &Path, volume: f32) -> Result<Self, String> {
        Self::start_at(path, None, Duration::ZERO, false, volume)
    }

    /// Audio stream `stream` of a video from `at`.
    pub fn start_at(path: &Path, stream: Option<usize>, at: Duration, paused: bool, volume: f32) -> Result<Self, String> {
        let (out, no_device) = if silent_mode() {
            (None, None)
        } else {
            match device() {
                Ok(o) => (Some(o), None),
                Err(e) => (None, Some(e)),
            }
        };
        let (player, null) = silent_player();
        let mut p = Playback { out, player, _null: Some(null), path: path.to_path_buf(), ffmpeg: stream.is_some(), stream, base: Duration::ZERO, length: None, no_device, volume, started: Instant::now() };
        p.load(at, paused)?;
        Ok(p)
    }

    /// A fresh player for the track from `at`.
    fn load(&mut self, at: Duration, paused: bool) -> Result<(), String> {
        let (player, null) = match &self.out {
            Some(o) => (Player::connect_new(&o.mixer), None),
            None => {
                let (p, g) = silent_player();
                (p, Some(g))
            }
        };
        player.set_volume(gain(self.volume));
        if paused {
            player.pause();
        }
        if !self.ffmpeg {
            let file = std::fs::File::open(&self.path).map_err(|e| e.to_string())?;
            match rodio::Decoder::try_from(file) {
                Ok(d) => {
                    self.length = d.total_duration().or(self.length);
                    player.append(d);
                    if !at.is_zero() {
                        let _ = player.try_seek(at);
                    }
                    self.base = Duration::ZERO;
                }
                Err(_) => self.ffmpeg = true,
            }
        }
        if self.ffmpeg {
            player.append(Ffmpeg::spawn(&self.path, at, self.stream)?);
            self.base = at;
        }
        // The old one stops when dropped.
        self.player = player;
        self._null = null;
        self.started = Instant::now();
        Ok(())
    }

    pub fn pos(&self) -> Duration {
        let p = self.base + self.player.get_pos();
        self.length.map_or(p, |l| p.min(l))
    }

    pub fn paused(&self) -> bool {
        self.player.is_paused()
    }

    /// Played to the end (or the decoder gave up).
    pub fn ended(&self) -> bool {
        self.player.empty()
    }

    /// Ended right away without playing anything: it can't be decoded.
    pub fn broken(&self) -> bool {
        self.ended() && self.pos() < Duration::from_millis(50) && self.started.elapsed() < Duration::from_secs(2)
    }

    pub fn toggle(&mut self) {
        if self.ended() {
            let _ = self.load(Duration::ZERO, false);
        } else if self.player.is_paused() {
            self.player.play();
        } else {
            self.player.pause();
        }
    }

    pub fn pause(&mut self) {
        self.player.pause();
    }

    pub fn resume(&mut self) {
        self.player.play();
    }

    pub fn seek(&mut self, to: Duration) {
        let to = self.length.map_or(to, |l| to.min(l.saturating_sub(Duration::from_millis(200))));
        let paused = self.player.is_paused();
        if self.ffmpeg || self.ended() {
            let _ = self.load(to, paused);
        } else if self.player.try_seek(to).is_err() {
            let _ = self.load(to, paused);
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v;
        self.player.set_volume(gain(v));
    }
}

/// Volume slider → amplitude (closer to how loud it sounds).
fn gain(v: f32) -> f32 {
    v.clamp(0.0, 1.0).powi(2)
}

// ------------------------------------------------------------ tags

#[derive(Default, Clone)]
pub struct Meta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<u32>,
    pub track: Option<u32>,
    pub genre: Option<String>,
    pub length: Option<Duration>,
    /// kbit/s, Hz, channels, bits per sample.
    pub bitrate: Option<u32>,
    pub rate: Option<u32>,
    pub channels: Option<u8>,
    pub bits: Option<u8>,
}

/// Tags, stream properties and the front cover (tag picture or a cover
/// file next to the track).
pub fn read_meta(path: &Path) -> (Meta, Option<Vec<u8>>) {
    use lofty::prelude::*;
    let mut m = Meta::default();
    let mut cover = None;
    if let Ok(f) = lofty::read_from_path(path) {
        let p = f.properties();
        m.length = Some(p.duration()).filter(|d| !d.is_zero());
        m.bitrate = p.audio_bitrate().filter(|&b| b > 0);
        m.rate = p.sample_rate();
        m.channels = p.channels();
        m.bits = p.bit_depth();
        if let Some(t) = f.primary_tag().or_else(|| f.first_tag()) {
            let s = |c: Option<std::borrow::Cow<'_, str>>| c.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
            m.title = s(t.title());
            m.artist = s(t.artist());
            m.album = s(t.album());
            m.genre = s(t.genre());
            m.year = t.date().map(|d| d.year as u32).filter(|&y| y > 0);
            m.track = t.track();
            let pics = t.pictures();
            let front = pics.iter().find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront).or(pics.first());
            cover = front.map(|p| p.data().to_vec());
        }
    }
    if cover.is_none() {
        cover = folder_cover(path.parent().unwrap_or(Path::new("/")));
    }
    (m, cover)
}

/// cover.jpg, folder.png and the like.
fn folder_cover(dir: &Path) -> Option<Vec<u8>> {
    const NAMES: &[&str] = &["cover", "folder", "front", "album", "albumart"];
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            n.rsplit_once('.').is_some_and(|(stem, ext)| NAMES.contains(&stem) && matches!(ext, "jpg" | "jpeg" | "png" | "webp"))
        })
        .collect();
    found.sort_by_key(|p| NAMES.iter().position(|n| p.file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase() == *n)));
    std::fs::read(found.first()?).ok()
}

fn cover_image(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut img = image::load_from_memory(bytes).ok()?;
    if img.width().max(img.height()) > 1024 {
        img = img.resize(1024, 1024, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

/// The cover as a thumbnail (for the file list).
pub fn thumbnail(path: &Path) -> Option<image::DynamicImage> {
    let (_, cover) = read_meta(path);
    image::load_from_memory(&cover?).ok()
}

// ------------------------------------------------------------ state

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Repeat {
    Off,
    All,
    One,
}

impl Repeat {
    pub fn from(i: u8) -> Self {
        match i {
            1 => Repeat::All,
            2 => Repeat::One,
            _ => Repeat::Off,
        }
    }
}

pub struct AudioState {
    pub tracks: Vec<PathBuf>,
    /// Track playing, and the playlist row the pad points at.
    pub index: usize,
    pub cursor: usize,
    pub meta: Meta,
    pub cover: Option<egui::TextureHandle>,
    pub play: Result<Playback, String>,
    loading: Option<Receiver<(Meta, Option<egui::ColorImage>)>>,
    /// Bring the cursor row into view.
    pub reveal: bool,
    /// Volume shown while it's being changed.
    pub volume_shown: Option<Instant>,
}

impl AudioState {
    pub fn open(path: PathBuf, siblings: Vec<PathBuf>, volume: f32, ctx: &egui::Context) -> Self {
        let tracks = if siblings.contains(&path) { siblings } else { vec![path.clone()] };
        let index = tracks.iter().position(|p| *p == path).unwrap_or(0);
        let mut a = AudioState {
            tracks,
            index,
            cursor: index,
            meta: Meta::default(),
            cover: None,
            play: Err(String::new()),
            loading: None,
            reveal: true,
            volume_shown: None,
        };
        a.start(volume, ctx);
        a
    }

    pub fn path(&self) -> &Path {
        &self.tracks[self.index]
    }

    fn start(&mut self, volume: f32, ctx: &egui::Context) {
        // Stop the old track before the new one opens the device.
        self.play = Err(String::new());
        let path = self.path().to_path_buf();
        self.play = Playback::start(&path, volume);
        self.meta = Meta::default();
        self.cover = None;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let c = ctx.clone();
        std::thread::Builder::new()
            .name("polka-tags".into())
            .spawn(move || {
                let (m, cover) = read_meta(&path);
                let _ = tx.send((m, cover.as_deref().and_then(cover_image)));
                c.request_repaint();
            })
            .ok();
        self.loading = Some(rx);
    }

    /// Play track `i` of the playlist.
    pub fn play_index(&mut self, i: usize, volume: f32, ctx: &egui::Context) {
        if i >= self.tracks.len() {
            return;
        }
        self.index = i;
        self.cursor = i;
        self.reveal = true;
        self.start(volume, ctx);
    }

    /// Previous / next track (wrapping).
    pub fn step(&mut self, d: isize, volume: f32, ctx: &egui::Context) {
        // Back near the start of a track goes to the previous one, else to
        // the start of this one.
        if d < 0 && self.play.as_ref().is_ok_and(|p| p.pos() > Duration::from_secs(3)) {
            if let Ok(p) = &mut self.play {
                p.seek(Duration::ZERO);
            }
            return;
        }
        let n = self.tracks.len() as isize;
        let i = (self.index as isize + d).rem_euclid(n.max(1)) as usize;
        self.play_index(i, volume, ctx);
    }

    pub fn length(&self) -> Option<Duration> {
        self.meta.length.or(self.play.as_ref().ok().and_then(|p| p.length))
    }

    pub fn pos(&self) -> Duration {
        self.play.as_ref().map_or(Duration::ZERO, |p| p.pos())
    }

    pub fn playing(&self) -> bool {
        self.play.as_ref().is_ok_and(|p| !p.paused() && !p.ended())
    }

    pub fn seek_by(&mut self, secs: f32) {
        let to = (self.pos().as_secs_f32() + secs).max(0.0);
        if let Ok(p) = &mut self.play {
            p.seek(Duration::from_secs_f32(to));
        }
    }

    pub fn seek_frac(&mut self, f: f32) {
        if let (Some(l), Ok(p)) = (self.length(), &mut self.play) {
            p.seek(l.mul_f32(f.clamp(0.0, 1.0)));
        }
    }

    pub fn toggle(&mut self) {
        if let Ok(p) = &mut self.play {
            p.toggle();
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        if let Ok(p) = &mut self.play {
            p.set_volume(v);
        }
        self.volume_shown = Some(Instant::now());
    }

    /// Per frame: pick up tags, move on at the end of a track. Returns true
    /// when another track became current.
    pub fn tick(&mut self, repeat: Repeat, volume: f32, ctx: &egui::Context) -> bool {
        if let Some(rx) = &self.loading {
            if let Ok((m, img)) = rx.try_recv() {
                self.loading = None;
                self.meta = m;
                self.cover = img.map(|i| ctx.load_texture(format!("cover:{}", self.path().display()), i, egui::TextureOptions::LINEAR));
            }
        }
        let Ok(p) = &self.play else { return false };
        if !p.ended() || p.paused() {
            return false;
        }
        if p.broken() {
            self.play = Err(crate::i18n::tr("Can't play this file").to_string());
            return false;
        }
        let last = self.index + 1 >= self.tracks.len();
        match repeat {
            Repeat::One => {
                if let Ok(p) = &mut self.play {
                    let _ = p.load(Duration::ZERO, false);
                }
                false
            }
            Repeat::Off if last => {
                // Stop at the end, ready to play again.
                if let Ok(p) = &mut self.play {
                    p.pause();
                }
                false
            }
            _ => {
                self.step(1, volume, ctx);
                true
            }
        }
    }
}

/// 3:07, 1:02:03.
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-second 440 Hz mono WAV.
    fn wav(path: &Path, secs: f32) {
        let rate = 8000u32;
        let n = (rate as f32 * secs) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + n * 2).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        for v in [16u32] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(n * 2).to_le_bytes());
        for i in 0..n {
            let v = ((i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 8000.0) as i16;
            b.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn plays_seeks_and_moves_on() {
        let dir = std::env::temp_dir().join(format!("polka-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
        wav(&a, 0.6);
        wav(&b, 3.0);
        let (m, cover) = read_meta(&b);
        assert!(cover.is_none());
        assert_eq!(m.rate, Some(8000));
        assert!(m.length.is_some_and(|l| (l.as_secs_f32() - 3.0).abs() < 0.05), "{:?}", m.length);

        let ctx = egui::Context::default();
        let mut st = AudioState::open(a.clone(), vec![a.clone(), b.clone()], 0.5, &ctx);
        assert!(st.play.is_ok());
        // Runs in real time into the silent sink, then goes on to b.
        let t0 = Instant::now();
        while st.index == 0 && t0.elapsed() < Duration::from_secs(5) {
            st.tick(Repeat::Off, 0.5, &ctx);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(st.index, 1, "moved on to the next track");
        let took = t0.elapsed().as_secs_f32();
        assert!((0.5..1.2).contains(&took), "played in real time: {took}");
        st.seek_frac(0.5);
        let p = st.pos().as_secs_f32();
        assert!((1.3..2.0).contains(&p), "seeked to {p}");
        st.toggle();
        assert!(!st.playing());
        st.toggle();
        assert!(st.playing());
        // ← early in a track: previous track; later: its start.
        st.step(-1, 0.5, &ctx);
        assert_eq!(st.index, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ffmpeg_fallback_decodes_opus() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("polka-opus-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (w, o) = (dir.join("t.wav"), dir.join("t.opus"));
        wav(&w, 2.0);
        let ok = std::process::Command::new("ffmpeg").args(["-v", "quiet", "-i"]).arg(&w).args(["-c:a", "libopus"]).arg(&o).status().is_ok_and(|s| s.success());
        if ok {
            let mut p = Playback::start(&o, 1.0).unwrap();
            assert!(p.ffmpeg);
            std::thread::sleep(Duration::from_millis(300));
            assert!(!p.ended() && p.pos() > Duration::from_millis(100));
            p.seek(Duration::from_secs(1));
            assert!(p.pos() >= Duration::from_secs(1));
            assert!(read_meta(&o).0.length.is_some());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clock_format() {
        assert_eq!(clock(Duration::from_secs(187)), "3:07");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
        assert!(is_audio("Песня.FLAC") && !is_audio("a.flac.txt"));
    }
}
