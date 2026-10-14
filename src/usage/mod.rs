//! Disk usage: a folder measured in the background and shown as rings
//! (like Filelight), tiles (a treemap) or a list, walked with the pad.

pub mod app;
pub mod draw;

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    /// The small files of a folder, summed up (how many).
    Rest(u32),
}

#[derive(Debug)]
pub struct Node {
    pub name: Box<str>,
    /// Bytes on disk, everything inside included.
    pub size: u64,
    /// Files inside (1 for a file).
    pub files: u64,
    pub kind: Kind,
    /// Biggest first.
    pub children: Vec<Node>,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    pub fn at(&self, path: &[usize]) -> &Node {
        path.iter().fold(self, |n, &i| &n.children[i])
    }

    /// The child path for a list of names (to find a folder again after a
    /// rescan); stops where a name is gone.
    pub fn find(&self, names: &[Box<str>]) -> Vec<usize> {
        let mut out = Vec::new();
        let mut n = self;
        for name in names {
            match n.children.iter().position(|c| c.kind == Kind::Dir && c.name == *name) {
                Some(i) => {
                    out.push(i);
                    n = &n.children[i];
                }
                None => break,
            }
        }
        out
    }

    pub fn names(&self, path: &[usize]) -> Vec<Box<str>> {
        let mut n = self;
        path.iter()
            .map(|&i| {
                n = &n.children[i];
                n.name.clone()
            })
            .collect()
    }
}

/// Files kept by name in each folder; the smaller ones become one item.
const KEEP_FILES: usize = 48;

#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    /// Folders that couldn't be read.
    pub errors: AtomicU64,
    pub current: Mutex<PathBuf>,
    pub cancel: AtomicBool,
}

pub struct Scan {
    pub root: PathBuf,
    pub progress: Arc<Progress>,
    pub started: Instant,
    /// Bytes used on the file system when the root is its mount point
    /// (for a progress bar).
    pub expect: Option<u64>,
    rx: Receiver<Node>,
}

impl Scan {
    pub fn start(root: PathBuf, ctx: &egui::Context) -> Scan {
        let progress = Arc::new(Progress::default());
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (p, r, ctx) = (progress.clone(), root.clone(), ctx.clone());
        std::thread::spawn(move || {
            let skip = mount_points(&r);
            let seen = Mutex::new(HashSet::new());
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).stack_size(16 << 20).build();
            let w = Walker { progress: &p, skip: &skip, seen: &seen };
            let name: Box<str> = r.display().to_string().into();
            let node = match pool {
                Ok(pool) => pool.install(|| w.dir(&r, name)),
                Err(_) => w.dir(&r, name),
            };
            let _ = tx.send(node);
            ctx.request_repaint();
        });
        let expect = mounts().into_iter().find(|m| m.path == root).map(|m| m.used);
        Scan { root, progress, started: Instant::now(), expect, rx }
    }

    pub fn done(&self) -> Option<Node> {
        self.rx.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.progress.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Scan {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Walker<'a> {
    progress: &'a Progress,
    skip: &'a HashSet<PathBuf>,
    seen: &'a Mutex<HashSet<(u64, u64)>>,
}

impl Walker<'_> {
    fn dir(&self, path: &Path, name: Box<str>) -> Node {
        let own = std::fs::symlink_metadata(path).map_or(0, |m| m.blocks() * 512);
        let mut node = Node { name, size: own, files: 0, kind: Kind::Dir, children: Vec::new() };
        if self.progress.cancel.load(Ordering::Relaxed) {
            return node;
        }
        let Ok(rd) = std::fs::read_dir(path) else {
            self.progress.errors.fetch_add(1, Ordering::Relaxed);
            return node;
        };
        if let Ok(mut c) = self.progress.current.try_lock() {
            c.clear();
            c.push(path);
        }
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        let (mut count, mut bytes) = (0, 0);
        for e in rd.flatten() {
            // Not followed: a link is the link itself.
            let Ok(md) = e.metadata() else { continue };
            let name: Box<str> = e.file_name().to_string_lossy().into();
            if md.is_dir() {
                let p = e.path();
                if !self.skip.contains(&p) {
                    dirs.push((p, name));
                }
                continue;
            }
            let mut size = md.blocks() * 512;
            // Hard links: counted where they're met first.
            if md.nlink() > 1 && !self.seen.lock().unwrap().insert((md.dev(), md.ino())) {
                size = 0;
            }
            count += 1;
            bytes += size;
            files.push(Node { name, size, files: 1, kind: Kind::File, children: Vec::new() });
        }
        self.progress.files.fetch_add(count, Ordering::Relaxed);
        self.progress.bytes.fetch_add(bytes, Ordering::Relaxed);
        let mut children: Vec<Node> = dirs.into_par_iter().map(|(p, n)| self.dir(&p, n)).collect();
        files.sort_unstable_by(|a, b| b.size.cmp(&a.size));
        if files.len() > KEEP_FILES {
            let rest = files.split_off(KEEP_FILES - 1);
            let size = rest.iter().map(|f| f.size).sum();
            files.push(Node { name: "".into(), size, files: rest.len() as u64, kind: Kind::Rest(rest.len() as u32), children: Vec::new() });
        }
        children.extend(files);
        children.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        node.size += children.iter().map(|c| c.size).sum::<u64>();
        node.files = children.iter().map(|c| c.files).sum();
        node.children = children;
        node
    }
}

fn unescape(s: &str) -> String {
    s.replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\")
}

/// Mount points inside `root`: other file systems aren't walked into
/// (/proc, /run, another disk, a snapshot mounted inside…).
fn mount_points(root: &Path) -> HashSet<PathBuf> {
    let text = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    text.lines()
        .filter_map(|l| l.split(' ').nth(4))
        .map(|m| PathBuf::from(unescape(m)))
        .filter(|m| m != root && m.starts_with(root))
        .collect()
}

/// A mounted file system worth measuring.
#[derive(Clone, Debug)]
pub struct Mount {
    /// Where to read it from (inside Flatpak maybe under /run/host/root).
    pub path: PathBuf,
    /// Where the host has it, for names and filters.
    pub host: PathBuf,
    pub fstype: String,
    pub total: u64,
    pub used: u64,
}

const REAL_FS: &[&str] = &["btrfs", "ext4", "ext3", "ext2", "xfs", "f2fs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "zfs", "bcachefs", "jfs", "reiserfs", "nilfs2"];

/// Places for system parts that live on the same disk as / anyway.
fn boring(p: &Path) -> bool {
    let s = p.to_string_lossy();
    ["/var", "/srv", "/root", "/tmp", "/opt", "/usr", "/snap", "/efi", "/boot/efi"].iter().any(|b| s == *b || s.starts_with(&format!("{b}/"))) || s.contains("/.snapshots")
}

pub fn mounts() -> Vec<Mount> {
    let text = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    // (device, folder of that file system shown, where, type)
    let mut all: Vec<(&str, String, PathBuf, &str)> = Vec::new();
    for l in text.lines() {
        let f: Vec<&str> = l.split(' ').collect();
        let Some(dash) = f.iter().position(|x| *x == "-") else { continue };
        let (Some(dev), Some(root), Some(mnt), Some(fstype)) = (f.get(2), f.get(3), f.get(4), f.get(dash + 1)) else { continue };
        if REAL_FS.contains(fstype) {
            all.push((dev, unescape(root), PathBuf::from(unescape(mnt)), fstype));
        }
    }
    let mut out: Vec<Mount> = Vec::new();
    for (dev, root, path, fstype) in &all {
        // A plain folder of a disk already listed, bound elsewhere (as a
        // sandbox shares /media and /mnt): not a disk of its own.
        let inner = all.iter().any(|(d, r, _, _)| d == dev && r != root && (r == "/" || root.starts_with(&format!("{}/", r.trim_end_matches('/')))));
        let Some(host) = host_path(path) else { continue };
        if inner || boring(&host) {
            continue;
        }
        // The same disk shared twice into the sandbox: the plain path wins.
        if let Some(i) = out.iter().position(|m| m.host == host) {
            if *path == host {
                out[i].path = path.clone();
            }
            continue;
        }
        let Some((total, used)) = fs_space(path) else { continue };
        out.push(Mount { path: path.clone(), host, fstype: fstype.to_string(), total, used });
    }
    out.sort_by(|a, b| a.host.cmp(&b.host));
    out
}

/// The host's path of a mount seen here; None for the sandbox's own parts
/// (the app, the runtime, its private folders).
fn host_path(p: &Path) -> Option<PathBuf> {
    if !crate::sandbox::flatpak() {
        return Some(p.to_path_buf());
    }
    if let Ok(rest) = p.strip_prefix("/run/host/root") {
        return Some(Path::new("/").join(rest));
    }
    let shared = ["/home", "/media", "/mnt", "/run/media", "/srv", "/opt"].iter().any(|d| p.starts_with(d));
    let own = p.components().any(|c| c.as_os_str() == ".var");
    (shared && !own).then(|| p.to_path_buf())
}

/// Size and bytes used of the file system holding `p`.
pub fn fs_space(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let block = s.f_frsize as u64;
    let total = s.f_blocks as u64 * block;
    let free = s.f_bfree as u64 * block;
    (total > 0).then(|| (total, total - free))
}

// ---------------------------------------------------------------- the page

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Rings,
    Tiles,
    List,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Rings, Mode::Tiles, Mode::List];

    pub fn from(i: u8) -> Mode {
        Mode::ALL[(i as usize).min(2)]
    }

    pub fn index(self) -> u8 {
        self as u8
    }

    /// Levels shown below the middle.
    pub fn depth(self) -> usize {
        match self {
            Mode::Rings => RINGS,
            Mode::Tiles => 3,
            Mode::List => 1,
        }
    }
}

pub const RINGS: usize = 5;

/// Something to measure on the start screen.
#[derive(Clone, Debug)]
pub struct Target {
    pub label: String,
    pub path: PathBuf,
    pub icon: &'static [&'static str],
    /// (total, used) for a whole file system.
    pub space: Option<(u64, u64)>,
}

pub struct Ready {
    pub tree: Node,
    /// The folder in the middle and the item picked, as child indices
    /// from the root; `sel` is always below `center`.
    pub center: Vec<usize>,
    pub sel: Vec<usize>,
    pub took: Duration,
    pub errors: u64,
    pub cancelled: bool,
}

pub enum State {
    Start { targets: Vec<Target>, cursor: usize },
    Scanning(Scan),
    Ready(Box<Ready>),
}

pub struct Usage {
    pub state: State,
    pub mode: Mode,
    /// Hidden while the tab shows its panes (a sidebar place was opened);
    /// the result stays for when the page comes back.
    pub shown: bool,
    /// Scroll the list to the picked row.
    pub reveal: bool,
    /// Last scan's root, for the start screen's cursor.
    pub last_root: Option<PathBuf>,
    /// Names from the root to the folder in the middle, kept over a rescan.
    pub restore: Option<(Vec<Box<str>>, Vec<Box<str>>)>,
}

impl Ready {
    pub fn new(tree: Node, took: Duration, errors: u64, cancelled: bool) -> Self {
        let sel = if tree.children.is_empty() { Vec::new() } else { vec![0] };
        Ready { tree, center: Vec::new(), sel, took, errors, cancelled }
    }

    pub fn center_node(&self) -> &Node {
        self.tree.at(&self.center)
    }

    pub fn sel_node(&self) -> Option<&Node> {
        (self.sel.len() > self.center.len()).then(|| self.tree.at(&self.sel))
    }

    /// The real path of an item (a folder for the small files' item).
    pub fn path_of(&self, root: &Path, path: &[usize]) -> PathBuf {
        let mut p = root.to_path_buf();
        let mut n = &self.tree;
        for &i in path {
            n = &n.children[i];
            if !matches!(n.kind, Kind::Rest(_)) {
                p.push(&*n.name);
            }
        }
        p
    }

    /// ←/→: the next item around the same ring (wraps).
    pub fn sibling(&mut self, d: isize) {
        if self.sel.len() <= self.center.len() {
            return;
        }
        let parent = &self.sel[..self.sel.len() - 1];
        let n = self.tree.at(parent).children.len() as isize;
        let last = self.sel.last_mut().unwrap();
        *last = (*last as isize + d).rem_euclid(n.max(1)) as usize;
    }

    pub fn sibling_edge(&mut self, last: bool) {
        if self.sel.len() <= self.center.len() {
            return;
        }
        let parent = &self.sel[..self.sel.len() - 1];
        let n = self.tree.at(parent).children.len();
        *self.sel.last_mut().unwrap() = if last { n.saturating_sub(1) } else { 0 };
    }

    /// One ring further out, onto the biggest item inside.
    pub fn deeper(&mut self, max_depth: usize) -> bool {
        let ok = self.sel.len() > self.center.len() && self.sel.len() - self.center.len() < max_depth && !self.tree.at(&self.sel).children.is_empty();
        if ok {
            self.sel.push(0);
        }
        ok
    }

    pub fn shallower(&mut self) -> bool {
        let ok = self.sel.len() > self.center.len() + 1;
        if ok {
            self.sel.pop();
        }
        ok
    }

    /// The picked folder goes in the middle.
    pub fn zoom_in(&mut self) -> bool {
        let ok = self.sel_node().is_some_and(|n| n.is_dir() && !n.children.is_empty());
        if ok {
            self.center = self.sel.clone();
            self.sel.push(0);
        }
        ok
    }

    /// Back to the folder above, with the one we were in picked.
    pub fn zoom_out(&mut self) -> bool {
        if self.center.is_empty() {
            return false;
        }
        self.sel = self.center.clone();
        self.center.pop();
        true
    }

    /// Show `path` (anywhere): the middle moves so that it's visible.
    pub fn select(&mut self, path: Vec<usize>, max_depth: usize) {
        if !path.starts_with(&self.center) || path.len() == self.center.len() {
            self.center = path[..path.len().saturating_sub(1)].to_vec();
        }
        while path.len() - self.center.len() > max_depth {
            self.center.push(path[self.center.len()]);
        }
        self.sel = path;
    }

    /// Keep the pick within what a mode shows.
    pub fn fit(&mut self, max_depth: usize) {
        self.sel.truncate(self.center.len() + max_depth);
    }
}

impl Usage {
    pub fn new(mode: Mode, targets: Vec<Target>) -> Self {
        Usage { state: State::Start { targets, cursor: 0 }, mode, shown: true, reveal: true, last_root: None, restore: None }
    }

    /// Per frame: has the scan finished?
    pub fn tick(&mut self) {
        let State::Scanning(scan) = &self.state else { return };
        let Some(tree) = scan.done() else { return };
        let p = &scan.progress;
        let mut r = Ready::new(tree, scan.started.elapsed(), p.errors.load(Ordering::Relaxed), p.cancel.load(Ordering::Relaxed));
        if let Some((center, sel)) = self.restore.take() {
            r.center = r.tree.find(&center);
            if r.center.len() == center.len() {
                let s = r.tree.at(&r.center).find(&sel[center.len().min(sel.len())..]);
                let mut full = r.center.clone();
                full.extend(s);
                if full.len() > r.center.len() {
                    r.sel = full;
                } else {
                    r.sel = r.center.clone();
                    if !r.center_node().children.is_empty() {
                        r.sel.push(0);
                    }
                }
            } else {
                r.center.clear();
                r.sel = if r.tree.children.is_empty() { vec![] } else { vec![0] };
            }
        }
        self.last_root = Some(scan.root.clone());
        self.state = State::Ready(Box::new(r));
        self.reveal = true;
    }

    pub fn scan(&mut self, root: PathBuf, ctx: &egui::Context) {
        self.state = State::Scanning(Scan::start(root, ctx));
    }

    pub fn ready(&self) -> Option<&Ready> {
        match &self.state {
            State::Ready(r) => Some(r),
            _ => None,
        }
    }

    pub fn ready_mut(&mut self) -> Option<&mut Ready> {
        match &mut self.state {
            State::Ready(r) => Some(r),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------- layout

/// One drawn piece: an item `depth` levels below the middle, covering the
/// fraction f0..f1 of the middle folder (angles for the rings, colours
/// everywhere).
#[derive(Clone, Debug)]
pub struct Seg {
    pub path: Vec<usize>,
    pub depth: usize,
    pub f0: f64,
    pub f1: f64,
}

/// Rings: items too thin to see (and what's inside them) are left out.
pub fn ring_segs(center: &Node, depth: usize, min_frac: f64) -> Vec<Seg> {
    fn walk(n: &Node, path: &mut Vec<usize>, d: usize, max: usize, f0: f64, span: f64, min: f64, out: &mut Vec<Seg>) {
        if n.size == 0 {
            return;
        }
        let mut at = f0;
        for (i, c) in n.children.iter().enumerate() {
            let w = span * c.size as f64 / n.size as f64;
            if w < min {
                break; // biggest first: the rest is thinner still
            }
            path.push(i);
            out.push(Seg { path: path.clone(), depth: d, f0: at, f1: at + w });
            if d < max {
                walk(c, path, d + 1, max, at, w, min, out);
            }
            path.pop();
            at += w;
        }
    }
    let mut out = Vec::new();
    walk(center, &mut Vec::new(), 1, depth, 0.0, 1.0, min_frac, &mut out);
    out
}

/// Where an item sits around the middle folder (0..1), for its colour.
pub fn frac_of(center: &Node, rel: &[usize]) -> (f64, f64) {
    let (mut f0, mut span) = (0.0, 1.0);
    let mut n = center;
    for &i in rel {
        if n.size == 0 {
            break;
        }
        let before: u64 = n.children[..i].iter().map(|c| c.size).sum();
        f0 += span * before as f64 / n.size as f64;
        span *= n.children[i].size as f64 / n.size as f64;
        n = &n.children[i];
    }
    (f0, f0 + span)
}

/// Squarified treemap: rectangles for `sizes` (biggest first) filling `r`.
pub fn squarify(sizes: &[f64], r: egui::Rect) -> Vec<egui::Rect> {
    let total: f64 = sizes.iter().sum();
    let mut out = vec![egui::Rect::NOTHING; sizes.len()];
    if total <= 0.0 || r.width() <= 0.0 || r.height() <= 0.0 {
        return out;
    }
    let scale = (r.width() as f64 * r.height() as f64) / total;
    let areas: Vec<f64> = sizes.iter().map(|s| s * scale).collect();
    let mut rest = r;
    let mut i = 0;
    while i < areas.len() {
        let side = rest.width().min(rest.height()) as f64;
        if side <= 0.0 {
            break;
        }
        // Grow the row while the worst aspect ratio improves.
        let worst = |row: &[f64]| {
            let s: f64 = row.iter().sum();
            let (mx, mn) = row.iter().fold((0.0f64, f64::MAX), |(a, b), &x| (a.max(x), b.min(x)));
            ((side * side * mx) / (s * s)).max((s * s) / (side * side * mn.max(1e-9)))
        };
        let mut j = i + 1;
        while j < areas.len() && areas[j] > 0.0 && worst(&areas[i..=j]) <= worst(&areas[i..j]) {
            j += 1;
        }
        let row = &areas[i..j];
        let sum: f64 = row.iter().sum();
        let thick = (sum / side) as f32;
        let horizontal = rest.width() >= rest.height();
        let mut at = if horizontal { rest.min.y } else { rest.min.x };
        for (k, a) in row.iter().enumerate() {
            let len = (a / sum * side) as f32;
            out[i + k] = if horizontal {
                egui::Rect::from_min_size(egui::pos2(rest.min.x, at), egui::vec2(thick, len))
            } else {
                egui::Rect::from_min_size(egui::pos2(at, rest.min.y), egui::vec2(len, thick))
            };
            at += len;
        }
        if horizontal {
            rest.min.x += thick;
        } else {
            rest.min.y += thick;
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_sizes_links_and_small_files() {
        let d = crate::vfs::temp_root().join(format!("usage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("big/inner")).unwrap();
        std::fs::create_dir_all(d.join("many")).unwrap();
        std::fs::write(d.join("big/inner/a.bin"), vec![1u8; 512 * 1024]).unwrap();
        std::fs::write(d.join("big/b.bin"), vec![1u8; 64 * 1024]).unwrap();
        std::fs::hard_link(d.join("big/b.bin"), d.join("b-link.bin")).unwrap();
        for i in 0..60 {
            std::fs::write(d.join(format!("many/f{i:02}")), vec![1u8; 100 + i]).unwrap();
        }
        let ctx = egui::Context::default();
        let scan = Scan::start(d.clone(), &ctx);
        let tree = loop {
            if let Some(t) = scan.done() {
                break t;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(tree.children[0].name.as_ref(), "big");
        let big = &tree.children[0];
        assert!(big.size >= 512 * 1024, "{}", big.size);
        assert_eq!(big.files, 2);
        // The second name of a hard-linked file costs nothing.
        let links: Vec<&Node> = tree.children.iter().chain(big.children.iter()).filter(|n| n.name.ends_with(".bin")).collect();
        assert_eq!(links.iter().filter(|n| n.size > 0).count(), 1, "{links:?}");
        let many = tree.children.iter().find(|n| n.name.as_ref() == "many").unwrap();
        assert_eq!(many.files, 60);
        assert_eq!(many.children.len(), KEEP_FILES);
        assert!(many.children.iter().any(|c| c.kind == Kind::Rest(60 - KEEP_FILES as u32 + 1)));
        // Sorted biggest first everywhere.
        assert!(many.children.windows(2).all(|w| w[0].size >= w[1].size));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn walks_rings_and_tiles() {
        let f = |name: &str, size| Node { name: name.into(), size, files: 1, kind: Kind::File, children: vec![] };
        let dir = |name: &str, children: Vec<Node>| Node { name: name.into(), size: children.iter().map(|c| c.size).sum(), files: children.len() as u64, kind: Kind::Dir, children };
        let tree = dir("/", vec![dir("a", vec![dir("a1", vec![f("x", 30)]), f("a2", 20)]), f("b", 40), f("c", 10)]);
        let mut r = Ready::new(tree, Duration::ZERO, 0, false);
        assert_eq!(r.sel, [0]);
        r.sibling(-1);
        assert_eq!(r.sel, [2]);
        assert!(!r.deeper(5), "a file has nothing inside");
        r.sibling(1);
        assert!(r.deeper(5) && r.deeper(5));
        assert_eq!(r.sel, [0, 0, 0]);
        assert!(!r.deeper(3) && !r.deeper(5));
        assert!(r.shallower());
        assert!(r.zoom_in());
        assert_eq!((r.center.clone(), r.sel.clone()), (vec![0, 0], vec![0, 0, 0]));
        assert!(r.zoom_out() && r.zoom_out());
        assert_eq!((r.center.clone(), r.sel.clone()), (vec![], vec![0]));
        assert!(!r.zoom_out());
        let segs = ring_segs(&r.tree, 5, 0.0);
        let a1 = segs.iter().find(|s| s.path == [0, 0]).unwrap();
        assert!((a1.f0 - 0.0).abs() < 1e-9 && (a1.f1 - 0.3).abs() < 1e-9, "{a1:?}");
        assert_eq!(frac_of(&r.tree, &[0, 1]), (0.3, 0.5));
        // Rename-proof restore.
        assert_eq!(r.tree.find(&["a".into(), "a1".into()]), [0, 0]);
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(300.0, 200.0));
        let tiles = squarify(&[50.0, 40.0, 10.0], rect);
        let area: f32 = tiles.iter().map(|t| t.area()).sum();
        assert!((area - rect.area()).abs() < 1.0, "{tiles:?}");
        assert!(tiles.iter().all(|t| rect.expand(0.5).contains_rect(*t)));
        assert!((tiles[0].area() / rect.area() - 0.5).abs() < 0.01);
        // Selecting deep down moves the middle along.
        r.select(vec![0, 0, 0], 1);
        assert_eq!(r.center, [0, 0]);
    }

    /// A real folder, timed: `POLKA_USAGE_DIR=~ cargo test --release usage_real -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn usage_real() {
        let root = PathBuf::from(std::env::var_os("POLKA_USAGE_DIR").expect("POLKA_USAGE_DIR"));
        let scan = Scan::start(root, &egui::Context::default());
        let tree = loop {
            if let Some(t) = scan.done() {
                break t;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        fn count(n: &Node) -> usize {
            1 + n.children.iter().map(count).sum::<usize>()
        }
        println!(
            "{:.2} s, {} files, {} on disk, {} nodes kept, {} unreadable",
            scan.started.elapsed().as_secs_f32(),
            tree.files,
            crate::app::human_size(tree.size),
            count(&tree),
            scan.progress.errors.load(Ordering::Relaxed)
        );
        for c in tree.children.iter().take(5) {
            println!("  {:>10}  {}", crate::app::human_size(c.size), c.name);
        }
    }
}
