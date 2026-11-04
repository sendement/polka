//! The torrent client: librqbit's session on a thread of its own (with a
//! tokio runtime), told what to do over a channel; what it's doing put in
//! a shared snapshot the page reads every frame.

pub mod app;
pub mod draw;

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};

/// Where a torrent comes from.
#[derive(Clone, Debug)]
pub enum Source {
    File(PathBuf),
    Magnet(String),
    #[cfg_attr(not(test), allow(dead_code))]
    Bytes(Vec<u8>),
}

impl Source {
    /// A path or a magnet link as given (on the command line, pasted).
    pub fn parse(s: &str) -> Option<Source> {
        let s = s.trim();
        if s.starts_with("magnet:") {
            Some(Source::Magnet(s.to_string()))
        } else if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) {
            // A bare info hash.
            Some(Source::Magnet(format!("magnet:?xt=urn:btih:{s}")))
        } else if is_torrent_file(Path::new(s)) {
            Some(Source::File(PathBuf::from(s)))
        } else {
            None
        }
    }
}

pub fn is_torrent_file(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
}

/// What the engine is set to.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Downloads go here unless chosen otherwise.
    pub folder: PathBuf,
    /// KiB a second (0: as fast as it goes).
    pub down: u32,
    pub up: u32,
    /// Go on sharing what's downloaded.
    pub seed: bool,
    /// Where the torrents are remembered between starts.
    pub data: PathBuf,
    /// No DHT, trackers, local discovery or port forwarding: peers only as
    /// given (tests).
    pub local_only: bool,
    /// Peers every torrent starts with (tests).
    pub peers: Vec<std::net::SocketAddr>,
}

/// What a torrent is doing.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Checking what's there already, or finding peers for a magnet.
    Starting,
    Downloading,
    /// Done, and sharing it.
    Seeding,
    /// Done, not shared.
    Done,
    Paused,
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileRow {
    pub path: String,
    pub len: u64,
    pub done: u64,
    /// Being downloaded (chosen).
    pub wanted: bool,
}

/// A torrent, as the page shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: usize,
    pub name: String,
    /// Where its files are (a folder of its own for many files).
    pub folder: PathBuf,
    /// Bytes chosen to download, and of them done.
    pub total: u64,
    pub done: u64,
    pub uploaded: u64,
    /// Bytes a second.
    pub down: f64,
    pub up: f64,
    pub peers: u32,
    pub seeders: u32,
    /// Seconds to go.
    pub eta: Option<u64>,
    pub state: State,
    pub files: Vec<FileRow>,
}

impl Item {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (self.done as f64 / self.total as f64) as f32 }
    }

    pub fn finished(&self) -> bool {
        matches!(self.state, State::Seeding | State::Done)
    }

    /// What it downloads into: the folder of many files, or the one file.
    pub fn root(&self) -> PathBuf {
        match &self.files[..] {
            [one] => self.folder.join(&one.path),
            _ => self.folder.clone(),
        }
    }

    /// Where its files go when `root` has been moved to `to`.
    pub fn folder_for(&self, to: &Path) -> PathBuf {
        if self.files.len() == 1 { to.parent().map_or_else(|| to.to_path_buf(), Path::to_path_buf) } else { to.to_path_buf() }
    }

    /// Where its files are seen in the panes: the folder of many files, or
    /// the folder holding a single one (and its name).
    pub fn reveal(&self) -> (PathBuf, Option<String>) {
        if self.files.len() == 1 {
            let p = self.folder.join(&self.files[0].path);
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
            (p.parent().map_or_else(|| self.folder.clone(), Path::to_path_buf), name)
        } else {
            (self.folder.clone(), None)
        }
    }
}

/// A torrent's contents, looked at before it's added.
#[derive(Clone, Debug, PartialEq)]
pub enum Peek {
    /// Reading it; a magnet: finding peers that have its list.
    Waiting,
    Ready { name: String, files: Vec<(String, u64)>, bytes: Vec<u8> },
    Failed(String),
}

/// What the page reads.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// The session is up.
    pub ready: bool,
    /// It couldn't start.
    pub error: Option<String>,
    pub items: Vec<Item>,
    pub peeks: HashMap<u64, Peek>,
    /// Torrents that finished downloading since last asked (their names).
    pub finished: Vec<String>,
    /// Something asked for went wrong (to be said once).
    pub failed: Vec<String>,
}

enum Req {
    Peek(u64, Source),
    Add { bytes: Vec<u8>, folder: PathBuf, only: Option<Vec<usize>> },
    Pause(usize),
    Resume(usize),
    Remove(usize, bool),
    /// Its files moved: on from `folder` (paused still, or not).
    Relocate(usize, PathBuf, bool),
    Set(Settings),
    Stop,
}

pub struct Engine {
    tx: Sender<Req>,
    pub snap: Arc<Mutex<Snapshot>>,
    pub settings: Settings,
    next_peek: u64,
}

impl Engine {
    /// The session started on a thread of its own; `wake` repaints.
    pub fn start(settings: Settings, wake: impl Fn() + Send + 'static) -> Engine {
        let (tx, rx) = crossbeam_channel::unbounded();
        let snap = Arc::new(Mutex::new(Snapshot::default()));
        let s2 = snap.clone();
        let set = settings.clone();
        std::thread::Builder::new()
            .name("torrents".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("torrents-rt").build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        s2.lock().unwrap().error = Some(e.to_string());
                        return wake();
                    }
                };
                rt.block_on(run(set, rx, s2, wake));
            })
            .expect("a thread");
        Engine { tx, snap, settings, next_peek: 1 }
    }

    /// Look inside a torrent (its name and files); its number.
    pub fn peek(&mut self, src: Source) -> u64 {
        let id = self.next_peek;
        self.next_peek += 1;
        self.snap.lock().unwrap().peeks.insert(id, Peek::Waiting);
        let _ = self.tx.send(Req::Peek(id, src));
        id
    }

    /// A look no longer needed.
    pub fn forget_peek(&self, id: u64) {
        self.snap.lock().unwrap().peeks.remove(&id);
    }

    pub fn peek_state(&self, id: u64) -> Option<Peek> {
        self.snap.lock().unwrap().peeks.get(&id).cloned()
    }

    /// Download a torrent looked at into `folder` (its own folder there
    /// for many files); `only`: the files chosen (None: all).
    pub fn add(&self, bytes: Vec<u8>, folder: PathBuf, only: Option<Vec<usize>>) {
        let _ = self.tx.send(Req::Add { bytes, folder, only });
    }

    pub fn pause(&self, id: usize) {
        let _ = self.tx.send(Req::Pause(id));
    }

    pub fn resume(&self, id: usize) {
        let _ = self.tx.send(Req::Resume(id));
    }

    /// Off the list; with `files`, what it downloaded goes too.
    pub fn remove(&self, id: usize, files: bool) {
        let _ = self.tx.send(Req::Remove(id, files));
    }

    /// Its files were moved to `folder`: it goes on there (or stays
    /// paused).
    pub fn relocate(&self, id: usize, folder: PathBuf, paused: bool) {
        let _ = self.tx.send(Req::Relocate(id, folder, paused));
    }

    pub fn set(&mut self, s: Settings) {
        if s != self.settings {
            self.settings = s.clone();
            let _ = self.tx.send(Req::Set(s));
        }
    }

    pub fn items(&self) -> Vec<Item> {
        self.snap.lock().unwrap().items.clone()
    }

    /// Finished and failed since last asked.
    pub fn take_news(&self) -> (Vec<String>, Vec<String>) {
        let mut s = self.snap.lock().unwrap();
        (std::mem::take(&mut s.finished), std::mem::take(&mut s.failed))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Req::Stop);
    }
}

fn limit(kib: u32) -> Option<NonZeroU32> {
    NonZeroU32::new(kib.saturating_mul(1024))
}

/// Where a torrent's files go: `folder`, or a folder named after it there
/// when it has many.
pub fn target_folder(folder: &Path, name: &str, files: usize) -> PathBuf {
    if files > 1 { folder.join(sanitize(name)) } else { folder.to_path_buf() }
}

/// A name made safe as one folder's.
fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c == '/' || c == '\0' { '_' } else { c }).collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "torrent".into() } else { s }
}

async fn run(set: Settings, rx: Receiver<Req>, snap: Arc<Mutex<Snapshot>>, wake: impl Fn() + Send + 'static) {
    use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, Session, SessionOptions, SessionPersistenceConfig};
    let _ = std::fs::create_dir_all(&set.data);
    let _ = std::fs::create_dir_all(&set.folder);
    let mut opts = SessionOptions {
        fastresume: true,
        persistence: Some(SessionPersistenceConfig::Json { folder: Some(set.data.clone()) }),
        ratelimits: librqbit::limits::LimitsConfig { download_bps: limit(set.down), upload_bps: limit(set.up) },
        listen: Some(librqbit::ListenerOptions { enable_upnp_port_forwarding: !set.local_only, ..Default::default() }),
        client_name_and_version: Some(format!("Polka {}", env!("CARGO_PKG_VERSION"))),
        ..Default::default()
    };
    if set.local_only {
        opts.dht = None;
        opts.disable_trackers = true;
        opts.disable_local_service_discovery = true;
        opts.listen = Some(librqbit::ListenerOptions { listen_addr: ([127, 0, 0, 1], 0).into(), enable_upnp_port_forwarding: false, ..Default::default() });
    }
    let session = match Session::new_with_opts(set.folder.clone(), opts).await {
        Ok(s) => s,
        Err(e) => {
            snap.lock().unwrap().error = Some(format!("{e:#}"));
            return wake();
        }
    };
    snap.lock().unwrap().ready = true;
    wake();
    let mut seed = set.seed;
    let peers = set.peers.clone();
    // Done downloading before: (said, stopped for not seeding).
    let mut was_done: HashMap<usize, bool> = HashMap::new();
    let mut first = true;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut last_items: Vec<Item> = Vec::new();
    loop {
        tick.tick().await;
        // What's asked.
        let mut stop = false;
        while let Ok(r) = rx.try_recv() {
            match r {
                Req::Stop => stop = true,
                Req::Peek(id, src) => {
                    let (s, snap) = (session.clone(), snap.clone());
                    tokio::spawn(async move {
                        let add = match &src {
                            Source::File(p) => match std::fs::read(p) {
                                Ok(b) => AddTorrent::from_bytes(b),
                                Err(e) => {
                                    snap.lock().unwrap().peeks.insert(id, Peek::Failed(e.to_string()));
                                    return;
                                }
                            },
                            Source::Bytes(b) => AddTorrent::from_bytes(b.clone()),
                            Source::Magnet(m) => AddTorrent::from_url(m.clone()),
                        };
                        let res = s.add_torrent(add, Some(AddTorrentOptions { list_only: true, ..Default::default() })).await;
                        let peek = match res {
                            Ok(AddTorrentResponse::ListOnly(l)) => {
                                let name = l.info.name().map(|n| n.into_owned()).unwrap_or_else(|| "torrent".into());
                                let files = l.info.iter_file_details().map(|f| (f.filename.to_string(), f.len)).collect();
                                Peek::Ready { name, files, bytes: l.torrent_bytes.to_vec() }
                            }
                            Ok(AddTorrentResponse::AlreadyManaged(_, h)) => Peek::Failed(format!("already:{}", h.name().unwrap_or_default())),
                            Ok(AddTorrentResponse::Added(..)) => Peek::Failed("added".into()),
                            Err(e) => Peek::Failed(format!("{e:#}")),
                        };
                        // (Forgotten meanwhile: no longer wanted.)
                        let mut g = snap.lock().unwrap();
                        if g.peeks.contains_key(&id) {
                            g.peeks.insert(id, peek);
                        }
                    });
                }
                Req::Add { bytes, folder, only } => {
                    let (s, snap) = (session.clone(), snap.clone());
                    let initial_peers = (!peers.is_empty()).then(|| peers.clone());
                    tokio::spawn(async move {
                        let opts = AddTorrentOptions { output_folder: Some(folder.to_string_lossy().into_owned()), only_files: only, overwrite: true, initial_peers, ..Default::default() };
                        if let Err(e) = s.add_torrent(AddTorrent::from_bytes(bytes), Some(opts)).await {
                            snap.lock().unwrap().failed.push(format!("{e:#}"));
                        }
                    });
                }
                Req::Pause(id) => {
                    if let Some(h) = session.get(id.into()) {
                        let _ = session.pause(&h).await;
                    }
                }
                Req::Resume(id) => {
                    if let Some(h) = session.get(id.into()) {
                        // Resumed by hand: shared again, if done.
                        was_done.insert(id, true);
                        if let Err(e) = session.unpause(&h).await {
                            snap.lock().unwrap().failed.push(format!("{e:#}"));
                        }
                    }
                }
                Req::Relocate(id, folder, paused) => {
                    // Off the list (its files stay), on again from the new
                    // place: what's there is checked, the rest comes.
                    let Some(h) = session.get(id.into()) else { continue };
                    let Ok(bytes) = h.with_metadata(|m| m.torrent_bytes.clone()) else { continue };
                    let only = h.only_files();
                    let done = was_done.remove(&id);
                    if let Err(e) = session.delete(id.into(), false).await {
                        snap.lock().unwrap().failed.push(format!("{e:#}"));
                        continue;
                    }
                    let initial_peers = (!peers.is_empty()).then(|| peers.clone());
                    let opts = AddTorrentOptions { output_folder: Some(folder.to_string_lossy().into_owned()), only_files: only, overwrite: true, paused, initial_peers, ..Default::default() };
                    match session.add_torrent(AddTorrent::from_bytes(bytes), Some(opts)).await {
                        // (Done already: not news again.)
                        Ok(r) => {
                            if let (Some(d), Some(h)) = (done, r.into_handle()) {
                                was_done.insert(h.id(), d);
                            }
                        }
                        Err(e) => snap.lock().unwrap().failed.push(format!("{e:#}")),
                    }
                }
                Req::Remove(id, files) => {
                    if let Err(e) = session.delete(id.into(), files).await {
                        snap.lock().unwrap().failed.push(format!("{e:#}"));
                    }
                    was_done.remove(&id);
                }
                Req::Set(s) => {
                    seed = s.seed;
                    session.ratelimits.set_download_bps(limit(s.down));
                    session.ratelimits.set_upload_bps(limit(s.up));
                }
            }
        }
        if stop {
            session.stop().await;
            return;
        }
        // What the torrents are doing.
        let mut to_pause = Vec::new();
        let mut finished = Vec::new();
        let items: Vec<Item> = session.with_torrents(|it| it.map(|(id, h)| item(id, h)).collect());
        for i in &items {
            let done = i.total > 0 && i.done >= i.total && !matches!(i.state, State::Starting | State::Error(_));
            match was_done.get(&i.id) {
                None => {
                    // (Known already at the start: not news.)
                    if done && !first {
                        finished.push(i.name.clone());
                    }
                    if done && !seed && i.state == State::Seeding && !first {
                        to_pause.push(i.id);
                    }
                    if done {
                        was_done.insert(i.id, false);
                    }
                }
                Some(_) => {}
            }
            if !done {
                was_done.remove(&i.id);
            }
        }
        for id in to_pause {
            if let Some(h) = session.get(id.into()) {
                let _ = session.pause(&h).await;
            }
        }
        first = false;
        let changed = items != last_items;
        {
            let mut g = snap.lock().unwrap();
            g.items = items.clone();
            g.finished.extend(finished.iter().cloned());
        }
        if changed || !finished.is_empty() || snap.lock().unwrap().peeks.values().any(|p| *p != Peek::Waiting) {
            wake();
        }
        last_items = items;
    }
}

/// A torrent as the page shows it.
fn item(id: usize, h: &librqbit::ManagedTorrent) -> Item {
    use librqbit::TorrentStatsState as S;
    let st = h.stats();
    let only = h.only_files();
    let mut files = Vec::new();
    let _ = h.with_metadata(|m| {
        files = m
            .file_infos
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.attrs.padding)
            .map(|(i, f)| FileRow {
                path: f.relative_filename.to_string_lossy().into_owned(),
                len: f.len,
                done: st.file_progress.get(i).copied().unwrap_or(0),
                wanted: only.as_ref().is_none_or(|o| o.contains(&i)),
            })
            .collect();
    });
    let name = h.name().unwrap_or_else(|| tr_fallback(&h.info_hash().as_string()));
    let live = st.live.as_ref();
    // What's chosen: its size and how much of it is here.
    let (total, done) = if files.is_empty() {
        (st.total_bytes, st.progress_bytes)
    } else {
        files.iter().filter(|f| f.wanted).fold((0, 0), |(t, d), f| (t + f.len, d + f.done.min(f.len)))
    };
    let finished = st.finished || (total > 0 && done >= total);
    let state = match st.state {
        S::Initializing { .. } => State::Starting,
        S::Paused => State::Paused,
        S::Error => State::Error(st.error.clone().unwrap_or_default()),
        S::Live if finished => State::Seeding,
        S::Live => State::Downloading,
    };
    let state = if state == State::Paused && finished { State::Done } else { state };
    Item {
        id,
        name,
        folder: h.output_folder().to_path_buf(),
        total,
        done,
        uploaded: st.uploaded_bytes,
        down: live.map_or(0.0, |l| l.download_speed.mbps * 1024.0 * 1024.0),
        up: live.map_or(0.0, |l| l.upload_speed.mbps * 1024.0 * 1024.0),
        peers: live.map_or(0, |l| l.snapshot.peer_stats.live),
        seeders: live.map_or(0, |l| l.snapshot.peer_stats.live_seeders),
        eta: None,
        state,
        files,
    }
    .with_eta()
}

fn tr_fallback(hash: &str) -> String {
    hash.chars().take(12).collect()
}

impl Item {
    /// Time to go from what's left and the speed.
    fn with_eta(mut self) -> Item {
        self.eta = (self.state == State::Downloading && self.down > 1.0).then(|| ((self.total.saturating_sub(self.done)) as f64 / self.down) as u64);
        self
    }
}

/// "1.5 MB/s".
pub fn speed(bps: f64) -> String {
    format!("{}/s", crate::app::human_size(bps.max(0.0) as u64))
}

/// "2 h 5 min", "4 min", "30 s".
pub fn eta(secs: u64) -> String {
    use crate::i18n::tr;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 99 {
        "∞".into()
    } else if h > 0 {
        format!("{h} {} {m} {}", tr("h"), tr("min"))
    } else if m > 0 {
        format!("{m} {}", tr("min"))
    } else {
        format!("{s} {}", tr("s"))
    }
}

/// The speed limits offered (KiB a second; 0: none).
pub const LIMITS: [u32; 10] = [0, 128, 256, 512, 1024, 2048, 5120, 10240, 20480, 51200];

/// A limit's name.
pub fn limit_name(kib: u32) -> String {
    if kib == 0 { crate::i18n::tr("No limit").to_string() } else { speed(kib as f64 * 1024.0) }
}

/// The next limit up or down from `kib`.
pub fn step_limit(kib: u32, d: isize) -> u32 {
    let i = LIMITS.iter().position(|l| *l >= kib).unwrap_or(LIMITS.len() - 1) as isize;
    LIMITS[(i + d).rem_euclid(LIMITS.len() as isize) as usize]
}

/// Where the session remembers its torrents.
pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("polka").join("torrents")
}

/// Torrents remembered from before (worth starting the session for).
pub fn any_remembered(data: &Path) -> bool {
    std::fs::read_to_string(data.join("session.json")).is_ok_and(|s| s.contains("info_hash"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_and_folders() {
        assert!(matches!(Source::parse(" magnet:?xt=urn:btih:abc "), Some(Source::Magnet(m)) if m == "magnet:?xt=urn:btih:abc"));
        let hash = "0123456789abcdef0123456789abcdef01234567";
        assert!(matches!(Source::parse(hash), Some(Source::Magnet(m)) if m.ends_with(hash)));
        assert!(matches!(Source::parse("/x/a.TORRENT"), Some(Source::File(_))));
        assert!(Source::parse("/x/a.txt").is_none());
        assert_eq!(target_folder(Path::new("/d"), "Big / set", 3), PathBuf::from("/d/Big _ set"));
        assert_eq!(target_folder(Path::new("/d"), "one.iso", 1), PathBuf::from("/d"));
        assert_eq!(target_folder(Path::new("/d"), "..", 2), PathBuf::from("/d/torrent"));
    }

    #[test]
    fn limits_step_round() {
        assert_eq!(step_limit(0, 1), 128);
        assert_eq!(step_limit(0, -1), 51200);
        assert_eq!(step_limit(51200, 1), 0);
        assert_eq!(step_limit(300, 1), 1024);
        assert_eq!(step_limit(512, -1), 256);
    }

    /// A file shared by one session, downloaded by the engine over
    /// localhost, chosen files only.
    #[test]
    fn downloads_from_a_peer() {
        let base = std::env::temp_dir().join(format!("polka-torrent-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src/set");
        std::fs::create_dir_all(&src).unwrap();
        let a: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(src.join("a.bin"), &a).unwrap();
        std::fs::write(src.join("b.txt"), b"not wanted").unwrap();
        // The other side: shares it.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (seeder, bytes, addr) = rt.block_on(async {
            let opts = librqbit::SessionOptions {
                dht: None,
                disable_trackers: true,
                disable_local_service_discovery: true,
                persistence: None,
                listen: Some(librqbit::ListenerOptions { listen_addr: ([127, 0, 0, 1], 0).into(), enable_upnp_port_forwarding: false, ..Default::default() }),
                ..Default::default()
            };
            let s = librqbit::Session::new_with_opts(base.join("src"), opts).await.unwrap();
            let (t, h) = s.create_and_serve_torrent(&src, librqbit::CreateTorrentOptions { name: None, trackers: vec![], piece_length: Some(16384) }).await.unwrap();
            h.wait_until_initialized().await.unwrap();
            let addr = s.listen_addr().unwrap();
            (s, t.as_bytes().unwrap().to_vec(), addr)
        });
        let set = Settings { folder: base.join("dl"), down: 0, up: 0, seed: false, data: base.join("data"), local_only: true, peers: vec![addr] };
        let mut e = Engine::start(set, || {});
        let id = e.peek(Source::Bytes(bytes));
        let mut peek = None;
        for _ in 0..200 {
            if let Some(p @ Peek::Ready { .. }) = e.peek_state(id) {
                peek = Some(p);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let Some(Peek::Ready { name, files, bytes }) = peek else { panic!("no look inside: {:?}", e.peek_state(id)) };
        assert_eq!(name, "set");
        let mut names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
        names.sort();
        assert_eq!(names, ["a.bin", "b.txt"]);
        let want = files.iter().position(|f| f.0 == "a.bin").unwrap();
        let folder = target_folder(&base.join("dl"), &name, files.len());
        // (The other side, as a peer: no DHT nor trackers here.)
        e.add(bytes, folder.clone(), Some(vec![want]));
        let mut got = None;
        for _ in 0..400 {
            if let Some(it) = e.items().into_iter().next() {
                if it.finished() {
                    got = Some(it);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(seeder);
        let it = got.unwrap_or_else(|| panic!("not downloaded: {:?}", e.items()));
        assert_eq!(it.folder, folder);
        assert_eq!(std::fs::read(folder.join("a.bin")).unwrap(), a);
        // (b.txt shares a.bin's last piece: written all the same, as
        // torrents do; it's just not asked for.)
        let rows: Vec<(String, bool)> = it.files.iter().map(|f| (f.path.clone(), f.wanted)).collect();
        assert!(rows.contains(&("a.bin".into(), true)) && rows.contains(&("b.txt".into(), false)), "{rows:?}");
        assert_eq!(it.total, a.len() as u64, "counted what wasn't chosen");
        let (fin, _) = e.take_news();
        assert_eq!(fin, vec!["set".to_string()]);
        // Not seeding: stopped once done.
        std::thread::sleep(Duration::from_millis(800));
        assert!(matches!(e.items()[0].state, State::Done), "{:?}", e.items()[0].state);
        // Gone, with its files.
        e.remove(it.id, true);
        std::thread::sleep(Duration::from_millis(800));
        assert!(e.items().is_empty());
        assert!(!folder.join("a.bin").exists());
        drop(e);
        let _ = std::fs::remove_dir_all(&base);
    }
}
