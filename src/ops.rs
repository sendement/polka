//! Background file operations with progress and cancellation.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use std::collections::{HashMap, HashSet};

use crate::archive_rw::{self, Plan};
use crate::i18n::tr;
use crate::vfs::{self, Dest, Location};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ArchiveFormat {
    Zip,
    SevenZ,
    TarZst,
    TarGz,
    TarXz,
}

impl ArchiveFormat {
    pub const ALL: [ArchiveFormat; 5] =
        [ArchiveFormat::Zip, ArchiveFormat::SevenZ, ArchiveFormat::TarZst, ArchiveFormat::TarGz, ArchiveFormat::TarXz];
    pub fn ext(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => ".zip",
            ArchiveFormat::SevenZ => ".7z",
            ArchiveFormat::TarZst => ".tar.zst",
            ArchiveFormat::TarGz => ".tar.gz",
            ArchiveFormat::TarXz => ".tar.xz",
        }
    }
    /// Compression algorithms to choose from; tarballs are named after theirs.
    pub fn methods(self) -> &'static [Method] {
        match self {
            ArchiveFormat::Zip => &[Method::Deflate, Method::Bzip2, Method::Zstd],
            ArchiveFormat::SevenZ => &[Method::Lzma2, Method::Lzma, Method::Ppmd, Method::Bzip2, Method::Deflate],
            _ => &[],
        }
    }
    pub fn levels(self) -> &'static [Level] {
        match self {
            ArchiveFormat::Zip | ArchiveFormat::SevenZ => &[Level::Store, Level::Fast, Level::Normal, Level::Max],
            _ => &[Level::Fast, Level::Normal, Level::Max],
        }
    }
    pub fn encrypts(self) -> bool {
        matches!(self, ArchiveFormat::Zip | ArchiveFormat::SevenZ)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Method {
    Deflate,
    Bzip2,
    Zstd,
    Lzma2,
    Lzma,
    Ppmd,
}

impl Method {
    pub fn label(self) -> &'static str {
        match self {
            Method::Deflate => "Deflate",
            Method::Bzip2 => "BZip2",
            Method::Zstd => "Zstandard",
            Method::Lzma2 => "LZMA2",
            Method::Lzma => "LZMA",
            Method::Ppmd => "PPMd",
        }
    }
    fn seven_z(self) -> &'static str {
        match self {
            Method::Deflate => "Deflate",
            Method::Bzip2 => "BZip2",
            Method::Zstd | Method::Lzma2 => "LZMA2",
            Method::Lzma => "LZMA",
            Method::Ppmd => "PPMd",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Level {
    Store,
    Fast,
    Normal,
    Max,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Store => tr("No compression"),
            Level::Fast => tr("Fast"),
            Level::Normal => tr("Normal"),
            Level::Max => tr("Maximum"),
        }
    }
}

/// How to build a new archive.
#[derive(Clone, Debug)]
pub struct CompressOpts {
    pub format: ArchiveFormat,
    pub method: Method,
    pub level: Level,
    /// Volume size in bytes: the result is cut into `name.001`, `name.002`…
    pub split: Option<u64>,
    pub password: Option<String>,
    /// 7z only: hide file names too.
    pub encrypt_names: bool,
}

impl CompressOpts {
    pub fn new(format: ArchiveFormat) -> Self {
        CompressOpts { format, method: format.methods().first().copied().unwrap_or(Method::Deflate), level: Level::Normal, split: None, password: None, encrypt_names: false }
    }

    /// Bring method/level/password in line with the format.
    pub fn fit(&mut self) {
        let f = self.format;
        if !f.methods().is_empty() && !f.methods().contains(&self.method) {
            self.method = f.methods()[0];
        }
        if !f.levels().contains(&self.level) {
            self.level = Level::Fast;
        }
        if !f.encrypts() {
            self.password = None;
        }
        if f != ArchiveFormat::SevenZ {
            self.encrypt_names = false;
        }
    }
}

pub struct Job {
    pub label: String,
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    pub result: Mutex<Option<Result<(), String>>>,
    pub finished_at: Mutex<Option<Instant>>,
    /// Directories whose listings change when the job ends.
    pub touches: Vec<Location>,
    /// The operation, to run again once a password is known.
    pub op: Option<Op>,
    /// Set when the job stopped for an archive password.
    pub need_password: Mutex<Option<(PathBuf, bool)>>,
    /// It wasn't allowed (administrator rights would do).
    pub denied: AtomicBool,
    /// Progress counts items, not bytes.
    pub items: std::sync::atomic::AtomicBool,
    /// What is being worked on now.
    pub current: Mutex<String>,
    pub started: Instant,
    /// Where it goes, for the progress panel ("→ Music").
    pub target: String,
}

impl Job {
    /// A job nobody watches, for internal steps.
    fn detached() -> Self {
        Job {
            label: String::new(),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            result: Mutex::new(None),
            finished_at: Mutex::new(None),
            touches: Vec::new(),
            op: None,
            need_password: Mutex::new(None),
            denied: AtomicBool::new(false),
            items: Default::default(),
            current: Default::default(),
            started: Instant::now(),
            target: String::new(),
        }
    }

    /// A job that only shows (tests of the progress panel).
    #[cfg(test)]
    pub fn shown(label: &str, target: &str, done: u64, total: u64, current: &str, secs: u64) -> Self {
        let j = Job { label: label.into(), target: target.into(), started: Instant::now() - std::time::Duration::from_secs(secs), ..Job::detached() };
        j.done.store(done, Ordering::Relaxed);
        j.total.store(total, Ordering::Relaxed);
        *j.current.lock().unwrap() = current.into();
        j
    }

    /// Note the item being worked on (its name).
    pub fn now_on(&self, p: &Path) {
        if let Ok(mut c) = self.current.try_lock() {
            c.clear();
            c.push_str(&p.file_name().map_or_else(|| p.to_string_lossy(), |n| n.to_string_lossy()));
        }
    }

    pub fn fraction(&self) -> f32 {
        let t = self.total.load(Ordering::Relaxed);
        if t == 0 { 0.0 } else { (self.done.load(Ordering::Relaxed) as f64 / t as f64).min(1.0) as f32 }
    }
    fn tick(&self, n: u64) -> io::Result<()> {
        self.done.fetch_add(n, Ordering::Relaxed);
        if self.cancel.load(Ordering::Relaxed) {
            Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
        } else {
            Ok(())
        }
    }
}

/// What to do when a pasted item's name is already taken at the destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// Keep both: the new one becomes "name (2)".
    Rename,
    /// Replace files, merge folders.
    Overwrite,
    Skip,
}

/// A password, kept out of anything printed.
#[derive(Clone)]
pub struct Secret(pub String);

/// What follows an operation done with administrator rights.
#[derive(Clone)]
pub enum After {
    /// A text file saved through a copy: the editor's changes are saved.
    Saved { path: PathBuf, text: String },
}

/// The job's error when sudo didn't take the password.
pub const WRONG_PASSWORD: &str = "polka:wrong-password";

#[derive(Clone)]
pub enum Op {
    /// Commands run with sudo (each an argument list), after the password
    /// is checked.
    Elevated { steps: Vec<Vec<std::ffi::OsString>>, label: &'static str, touches: Vec<Location>, pw: Secret, after: Option<After> },
    Copy { sources: Vec<Location>, dest: Dest, policy: Conflict },
    Move { sources: Vec<Location>, dest: Dest, policy: Conflict },
    Trash { paths: Vec<PathBuf> },
    /// Out of the trash: (item in it, where it came from).
    Restore { items: Vec<(PathBuf, PathBuf)>, policy: Conflict },
    Delete { paths: Vec<PathBuf> },
    Compress { sources: Vec<PathBuf>, out: PathBuf, opts: CompressOpts },
    Extract { archive: PathBuf, dest: PathBuf },
    /// Delete / rename / mkdir inside an archive.
    ArchiveEdit { archive: PathBuf, plan: Plan, label: &'static str },
}

pub fn spawn(op: Op, wake: impl Fn() + Send + 'static) -> Arc<Job> {
    let (label, touches) = match &op {
        Op::Copy { sources, dest, .. } => (tr("Copying"), touched(sources, Some(dest))),
        Op::Move { sources, dest, .. } => (tr("Moving"), touched(sources, Some(dest))),
        Op::Trash { paths } | Op::Delete { paths } => (
            tr("Deleting"),
            touched(&paths.iter().cloned().map(Location::Local).collect::<Vec<_>>(), None),
        ),
        Op::Restore { items, .. } => {
            let mut t: Vec<Location> = items.iter().map(|(from, _)| Location::Local(from.clone())).collect();
            t.extend(items.iter().filter_map(|(_, to)| to.parent()).map(|d| Location::Local(d.to_path_buf())));
            (tr("Restoring"), t)
        }
        Op::Compress { out, .. } => {
            (tr("Compressing"), vec![Location::Local(out.parent().unwrap_or(Path::new("/")).into())])
        }
        Op::Extract { dest, .. } => (tr("Extracting"), vec![Location::Local(dest.clone())]),
        Op::ArchiveEdit { archive, label, .. } => {
            (*label, vec![Location::Archive { archive: archive.clone(), inner: String::new() }])
        }
        Op::Elevated { label, touches, .. } => (*label, touches.clone()),
    };
    // Editing an archive changes its folder too (size, and the number of
    // volumes for a split one).
    let mut touches = touches;
    let folders: Vec<Location> = touches
        .iter()
        .filter_map(|t| match t {
            Location::Archive { archive, .. } => archive.parent().map(|p| Location::Local(p.into())),
            _ => None,
        })
        .collect();
    touches.extend(folders);
    // "→ folder" (or the archive) for the progress panel.
    let name_of = |p: &Path| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
    let target = match &op {
        Op::Copy { dest, .. } | Op::Move { dest, .. } => match dest {
            Dest::Dir(d) => name_of(d),
            Dest::Archive { archive, .. } => name_of(archive),
        },
        Op::Compress { out, .. } => name_of(out),
        Op::Extract { dest, .. } => name_of(dest),
        Op::ArchiveEdit { archive, .. } => name_of(archive),
        Op::Trash { .. } => tr("Trash").to_string(),
        Op::Restore { items, .. } => match items.as_slice() {
            [(_, to)] => to.parent().map(name_of).unwrap_or_default(),
            _ => String::new(),
        },
        Op::Delete { .. } | Op::Elevated { .. } => String::new(),
    };
    let job = Arc::new(Job {
        label: label.to_string(),
        items: Default::default(),
        current: Default::default(),
        started: Instant::now(),
        target,
        done: AtomicU64::new(0),
        total: AtomicU64::new(0),
        cancel: AtomicBool::new(false),
        result: Mutex::new(None),
        finished_at: Mutex::new(None),
        touches,
        op: Some(op.clone()),
        need_password: Mutex::new(None),
        denied: AtomicBool::new(false),
    });
    let j = job.clone();
    std::thread::spawn(move || {
        let r = run(op, &j).map_err(|e| {
            if let Some(np) = vfs::need_password(&e) {
                *j.need_password.lock().unwrap() = Some((np.archive.clone(), np.wrong));
            }
            if denied(&e) {
                j.denied.store(true, Ordering::Relaxed);
            }
            e.to_string()
        });
        *j.result.lock().unwrap() = Some(r);
        *j.finished_at.lock().unwrap() = Some(Instant::now());
        wake();
    });
    job
}

fn touched(sources: &[Location], dest: Option<&Dest>) -> Vec<Location> {
    let mut v: Vec<Location> = sources.iter().filter_map(|s| s.parent().map(|(p, _)| p)).collect();
    if let Some(d) = dest {
        v.push(d.location());
    }
    v.dedup();
    v
}

fn run(op: Op, job: &Job) -> io::Result<()> {
    match op {
        Op::Copy { sources, dest, policy } => transfer(&sources, &dest, false, policy, job),
        Op::Move { sources, dest, policy } => transfer(&sources, &dest, true, policy, job),
        Op::Trash { paths } => {
            job.items.store(true, Ordering::Relaxed);
            job.total.store(paths.len() as u64, Ordering::Relaxed);
            trash::delete_all(&paths).map_err(|e| io::Error::other(e.to_string()))?;
            job.done.store(paths.len() as u64, Ordering::Relaxed);
            Ok(())
        }
        Op::Restore { items, policy } => {
            job.total.store(items.iter().map(|(from, _)| tree_size(from)).sum(), Ordering::Relaxed);
            for (from, to) in items {
                job.now_on(&to);
                let (Some(dir), Some(name)) = (to.parent(), to.file_name()) else { continue };
                // Its folder may have gone since.
                std::fs::create_dir_all(dir)?;
                let Some(target) = resolve(dir, &name.to_string_lossy(), policy) else {
                    job.tick(tree_size(&from))?;
                    continue;
                };
                move_path(&from, &target, job)?;
                if let Some(info) = crate::trashbin::info_of(&from) {
                    let _ = std::fs::remove_file(info);
                }
            }
            Ok(())
        }
        Op::Delete { paths } => {
            job.items.store(true, Ordering::Relaxed);
            job.total.store(paths.len() as u64, Ordering::Relaxed);
            for p in paths {
                job.now_on(&p);
                remove_any(&p)?;
                job.tick(1)?;
            }
            Ok(())
        }
        Op::Compress { sources, out, opts } => compress(&sources, &out, &opts, job),
        Op::Extract { archive, dest } => extract_all(&archive, &dest, job),
        Op::Elevated { steps, pw, .. } => run_elevated(&steps, &pw, job),
        Op::ArchiveEdit { archive, plan, .. } => {
            job.total.store(archive_rw::estimate(&archive, &plan), Ordering::Relaxed);
            archive_rw::rewrite(&archive, &plan, |n| {
                job.done.store(n, Ordering::Relaxed);
                !job.cancel.load(Ordering::Relaxed)
            })
        }
    }
}

/// Not allowed: what administrator rights would get past (not a read-only
/// disk).
pub fn denied(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(1) || {
        let s = e.to_string();
        s.contains("os error 13") || s.contains("Permission denied") || s.contains("os error 1)")
    }
}

/// sudo with the password on its input; the command's own output is left
/// out, its errors come back.
fn sudo(args: &[std::ffi::OsString], pw: &Secret) -> io::Result<(bool, String)> {
    use std::process::{Command, Stdio};
    let mut c = Command::new("sudo");
    c.args(["-S", "-p", ""]).args(args).env("LC_ALL", "C").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = crate::sandbox::host(c)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| io::Error::other(format!("sudo: {e}")))?;
    if let Some(mut i) = child.stdin.take() {
        let _ = writeln!(i, "{}", pw.0);
    }
    let out = child.wait_with_output()?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stderr).trim().to_string()))
}

fn run_elevated(steps: &[Vec<std::ffi::OsString>], pw: &Secret, job: &Job) -> io::Result<()> {
    job.items.store(true, Ordering::Relaxed);
    job.total.store(steps.len() as u64, Ordering::Relaxed);
    // The password first, on its own: a wrong one is asked again.
    let (ok, err) = sudo(&["-k".into(), "-v".into()], pw)?;
    if !ok {
        let wrong = ["incorrect password", "Sorry, try again", "no password was provided", "Authentication failure"].iter().any(|m| err.contains(m));
        return Err(io::Error::other(if wrong { WRONG_PASSWORD.to_string() } else { err }));
    }
    for (i, s) in steps.iter().enumerate() {
        if let Some(target) = s.last() {
            job.now_on(Path::new(target));
        }
        let mut args: Vec<std::ffi::OsString> = vec!["--".into()];
        // Run on the host: paths as the host has them.
        args.extend(s.iter().map(|a| if a.to_string_lossy().starts_with('/') { crate::sandbox::host_path(Path::new(a)).into_os_string() } else { a.clone() }));
        let (ok, err) = sudo(&args, pw)?;
        if !ok {
            return Err(io::Error::other(err));
        }
        job.done.store(i as u64 + 1, Ordering::Relaxed);
    }
    Ok(())
}

/// The same operation as commands for sudo (local files only).
pub fn elevated_steps(op: &Op) -> Option<(Vec<Vec<std::ffi::OsString>>, &'static str)> {
    let args = |v: &[&std::ffi::OsStr]| v.iter().map(|s| s.to_os_string()).collect::<Vec<_>>();
    match op {
        Op::Copy { sources, dest: Dest::Dir(d), policy } | Op::Move { sources, dest: Dest::Dir(d), policy } => {
            let mv = matches!(op, Op::Move { .. });
            let mut steps = Vec::new();
            for s in sources {
                let src = s.local_path()?;
                let name = src.file_name()?.to_string_lossy().into_owned();
                let (flag, target) = match policy {
                    Conflict::Overwrite => ("-f", d.join(&name)),
                    Conflict::Skip => ("-n", d.join(&name)),
                    Conflict::Rename => ("-n", unique_path(d, &name)),
                };
                let cmd: &[&std::ffi::OsStr] = if mv {
                    &["mv".as_ref(), flag.as_ref(), "--".as_ref(), src.as_os_str(), target.as_os_str()]
                } else {
                    &["cp".as_ref(), "-a".as_ref(), flag.as_ref(), "--".as_ref(), src.as_os_str(), target.as_os_str()]
                };
                steps.push(args(cmd));
            }
            Some((steps, if mv { tr("Moving") } else { tr("Copying") }))
        }
        // Out of a system folder there's no trash to go to: deleted.
        Op::Delete { paths } | Op::Trash { paths } => {
            let mut v: Vec<std::ffi::OsString> = vec!["rm".into(), "-rf".into(), "--".into()];
            v.extend(paths.iter().map(|p| p.as_os_str().to_os_string()));
            Some((vec![v], tr("Deleting")))
        }
        _ => None,
    }
}

/// `dir/name`, or `dir/name (2)`, `dir/name (3)`… if taken. Keeps compound
/// extensions like `.tar.gz` intact.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(unique_name(name, |n| dir.join(n).symlink_metadata().is_ok()))
}

pub fn unique_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    (2..).map(|i| format!("{stem} ({i}){ext}")).find(|n| !taken(n)).unwrap()
}

pub fn split_ext(name: &str) -> (&str, &str) {
    let lower = name.to_lowercase();
    for compound in [".pkg.tar.zst", ".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2"] {
        if lower.ends_with(compound) && name.len() > compound.len() {
            let at = name.len() - compound.len();
            return (&name[..at], &name[at..]);
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

fn tree_size(p: &Path) -> u64 {
    let Ok(m) = p.symlink_metadata() else { return 0 };
    if m.is_dir() {
        std::fs::read_dir(p)
            .map(|rd| rd.flatten().map(|e| tree_size(&e.path())).sum())
            .unwrap_or(0)
    } else {
        m.len()
    }
}

fn remove_any(p: &Path) -> io::Result<()> {
    if p.symlink_metadata()?.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }
}

fn join_inner(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

/// Target path in a real dir under `policy`; None = skip.
fn resolve(dir: &Path, name: &str, policy: Conflict) -> Option<PathBuf> {
    let target = dir.join(name);
    if target.symlink_metadata().is_err() {
        return Some(target);
    }
    match policy {
        Conflict::Rename => Some(unique_path(dir, name)),
        Conflict::Overwrite => Some(target),
        Conflict::Skip => None,
    }
}

/// Names in `sources` that already exist in `dest` (for the conflict prompt).
/// Pasting into the source's own folder never counts: that always keeps both.
pub fn conflicts(sources: &[Location], dest: &Dest) -> Vec<String> {
    let existing: HashSet<String> =
        vfs::list(&dest.location()).map(|v| v.into_iter().map(|e| e.name).collect()).unwrap_or_default();
    sources
        .iter()
        .filter(|s| s.parent().map(|(p, _)| p) != Some(dest.location()))
        .filter_map(|s| s.parent().map(|(_, n)| n))
        .filter(|n| existing.contains(n))
        .collect()
}

fn transfer(sources: &[Location], dest: &Dest, mv: bool, policy: Conflict, job: &Job) -> io::Result<()> {
    let mut total = 0;
    for s in sources {
        total += match s {
            Location::Local(p) => tree_size(p),
            Location::Archive { archive, inner } => archive_subtree_size(archive, inner),
            Location::Search { .. } | Location::Steam(_) => 0,
        };
    }
    job.total.store(total, Ordering::Relaxed);
    match dest {
        Dest::Dir(d) => transfer_to_dir(sources, d, mv, policy, job),
        Dest::Archive { archive, inner } => transfer_to_archive(sources, archive, inner, mv, policy, job),
    }
}

fn transfer_to_dir(sources: &[Location], dest: &Path, mv: bool, policy: Conflict, job: &Job) -> io::Result<()> {
    // Archive sources of a move are removed from their archives at the end.
    let mut moved_out: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for s in sources {
        let Some((parent, name)) = s.parent() else { continue };
        let same_dir = parent == Location::Local(dest.to_path_buf());
        match s {
            Location::Local(src) => {
                if dest.starts_with(src) {
                    return Err(io::Error::other(format!("cannot put {} inside itself", src.display())));
                }
                if same_dir {
                    if !mv {
                        copy_tree(src, &unique_path(dest, &name), job)?;
                    }
                    continue;
                }
                let Some(target) = resolve(dest, &name, policy) else {
                    job.tick(tree_size(src))?;
                    continue;
                };
                if mv { move_path(src, &target, job)? } else { copy_tree(src, &target, job)? }
            }
            Location::Archive { archive, inner } => {
                // Stage first so name clashes get resolved like local copies.
                let stage = staging_dir(dest)?;
                let base = job.done.load(Ordering::Relaxed);
                let r = vfs::extract(archive, std::slice::from_ref(inner), &stage, |n| {
                    job.done.store(base + n, Ordering::Relaxed);
                    !job.cancel.load(Ordering::Relaxed)
                });
                let r = r.and_then(|_| place_from_stage(&stage, dest, policy, job));
                let _ = std::fs::remove_dir_all(&stage);
                r?;
                if mv {
                    moved_out.entry(archive.clone()).or_default().push(inner.clone());
                }
            }
            Location::Search { .. } | Location::Steam(_) => {}
        }
    }
    for (archive, remove) in moved_out {
        archive_rw::rewrite(&archive, &Plan { remove, ..Default::default() }, |_| true)?;
    }
    Ok(())
}

fn transfer_to_archive(
    sources: &[Location],
    archive: &Path,
    dir: &str,
    mv: bool,
    policy: Conflict,
    job: &Job,
) -> io::Result<()> {
    let here = Location::Archive { archive: archive.to_path_buf(), inner: dir.to_string() };
    let existing: HashMap<String, bool> =
        vfs::list(&here)?.into_iter().map(|e| (e.name, e.is_dir)).collect();
    let mut taken: HashSet<String> = existing.keys().cloned().collect();
    let stage = vfs::temp_root().join(format!("stage-{}-{:x}", std::process::id(), job as *const Job as usize));
    let mut plan = Plan::default();
    let mut local_moves = Vec::new();
    let mut moved_out: HashMap<PathBuf, Vec<String>> = HashMap::new();

    let r = (|| -> io::Result<()> {
        for s in sources {
            let Some((parent, name)) = s.parent() else { continue };
            let same_dir = parent == here;
            if same_dir && mv {
                continue;
            }
            let pol = if same_dir { Conflict::Rename } else { policy };
            let target = if !taken.contains(&name) {
                name.clone()
            } else {
                match pol {
                    Conflict::Rename => unique_name(&name, |n| taken.contains(n)),
                    Conflict::Overwrite => name.clone(),
                    Conflict::Skip => continue,
                }
            };
            let target_inner = join_inner(dir, &target);
            let src_is_dir = match s {
                Location::Local(p) => p.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false),
                Location::Archive { archive: a, inner } => {
                    vfs::archive_index(a)?.dirs.contains_key(&vfs::normalize_inner(inner))
                }
                Location::Search { .. } | Location::Steam(_) => false,
            };
            // Replacing a folder with a file (or back) drops the old subtree.
            if existing.get(&target).is_some_and(|d| *d != src_is_dir) {
                plan.remove.push(target_inner.clone());
            }
            taken.insert(target.clone());

            match s {
                Location::Archive { archive: a, inner } if a == archive && mv => {
                    if dir == inner || dir.starts_with(&format!("{inner}/")) {
                        return Err(io::Error::other(format!("cannot put {name} inside itself")));
                    }
                    if existing.contains_key(&target) {
                        plan.remove.push(target_inner.clone());
                    }
                    plan.rename.push((inner.clone(), target_inner));
                }
                Location::Archive { archive: a, inner } => {
                    let sub = stage.join(plan.add.len().to_string());
                    std::fs::create_dir_all(&sub)?;
                    vfs::extract(a, std::slice::from_ref(inner), &sub, |_| !job.cancel.load(Ordering::Relaxed))?;
                    plan.add.push((sub.join(&name), target_inner));
                    if mv {
                        moved_out.entry(a.clone()).or_default().push(inner.clone());
                    }
                }
                Location::Local(p) => {
                    plan.add.push((p.clone(), target_inner));
                    if mv {
                        local_moves.push(p.clone());
                    }
                }
                Location::Search { .. } | Location::Steam(_) => {}
            }
        }
        job.total.store(archive_rw::estimate(archive, &plan), Ordering::Relaxed);
        archive_rw::rewrite(archive, &plan, |n| {
            job.done.store(n, Ordering::Relaxed);
            !job.cancel.load(Ordering::Relaxed)
        })
    })();
    let _ = std::fs::remove_dir_all(&stage);
    r?;
    for p in local_moves {
        remove_any(&p)?;
    }
    for (a, remove) in moved_out {
        archive_rw::rewrite(&a, &Plan { remove, ..Default::default() }, |_| true)?;
    }
    Ok(())
}

fn staging_dir(dest: &Path) -> io::Result<PathBuf> {
    let stage = dest.join(format!(".polka-stage-{}", std::process::id()));
    std::fs::create_dir_all(&stage)?;
    Ok(stage)
}

fn place_from_stage(stage: &Path, dest: &Path, policy: Conflict, job: &Job) -> io::Result<()> {
    for e in std::fs::read_dir(stage)?.flatten() {
        if let Some(target) = resolve(dest, &e.file_name().to_string_lossy(), policy) {
            // Progress was already counted while extracting.
            move_path(&e.path(), &target, &Job::detached())?;
        }
    }
    let _ = job;
    Ok(())
}

/// Move `src` to `dst`, merging into an existing folder and replacing
/// existing files. Falls back to copy+delete across file systems.
fn move_path(src: &Path, dst: &Path, job: &Job) -> io::Result<()> {
    if let Ok(dm) = dst.symlink_metadata() {
        let sm = src.symlink_metadata()?;
        if sm.is_dir() && dm.is_dir() {
            for e in std::fs::read_dir(src)?.flatten() {
                move_path(&e.path(), &dst.join(e.file_name()), job)?;
            }
            return std::fs::remove_dir(src);
        }
        remove_any(dst)?;
    }
    let size = tree_size(src);
    match std::fs::rename(src, dst) {
        Ok(()) => job.tick(size),
        Err(e) if e.raw_os_error() == Some(18) => {
            // EXDEV
            copy_tree(src, dst, job)?;
            remove_any(src)
        }
        Err(e) => Err(e),
    }
}

fn archive_subtree_size(archive: &Path, inner: &str) -> u64 {
    let Ok(idx) = vfs::archive_index(archive) else { return 0 };
    fn walk(idx: &vfs::ArchiveIndex, dir: &str) -> u64 {
        idx.dirs.get(dir).map_or(0, |es| {
            es.iter()
                .map(|e| if e.is_dir { walk(idx, &join_inner(dir, &e.name)) } else { e.size })
                .sum()
        })
    }
    let inner = vfs::normalize_inner(inner);
    if idx.dirs.contains_key(&inner) {
        return walk(&idx, &inner);
    }
    let (parent, name) = inner.rsplit_once('/').unwrap_or(("", &inner));
    idx.dirs
        .get(parent)
        .and_then(|es| es.iter().find(|e| e.name == name))
        .map_or(0, |e| e.size)
}

/// Copy a file or tree. An existing `dst` is merged into (folders) or
/// replaced (everything else).
fn copy_tree(src: &Path, dst: &Path, job: &Job) -> io::Result<()> {
    let m = src.symlink_metadata()?;
    let ft = m.file_type();
    if let Ok(dm) = dst.symlink_metadata() {
        if !(ft.is_dir() && dm.is_dir()) {
            remove_any(dst)?;
        }
    }
    if ft.is_symlink() {
        let target = std::fs::read_link(src)?;
        std::os::unix::fs::symlink(target, dst)?;
    } else if ft.is_dir() {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dst.join(e.file_name()), job)?;
        }
        let _ = std::fs::set_permissions(dst, m.permissions());
    } else {
        job.now_on(src);
        let mut r = File::open(src)?;
        let mut w = File::create(dst)?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n])?;
            if let Err(e) = job.tick(n as u64) {
                drop(w);
                let _ = std::fs::remove_file(dst);
                return Err(e);
            }
        }
        let _ = std::fs::set_permissions(dst, m.permissions());
    }
    Ok(())
}

// ---------------------------------------------------------------- archiver

struct Counting<'a, R> {
    inner: R,
    job: &'a Job,
}

impl<R: Read> Read for Counting<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.job.tick(n as u64)?;
        Ok(n)
    }
}

/// Walk `root` yielding (absolute path, path inside the archive).
fn walk(root: &Path, base: &Path, out: &mut Vec<(PathBuf, String)>) {
    let rel = root.strip_prefix(base).unwrap_or(root).to_string_lossy().into_owned();
    out.push((root.to_path_buf(), rel));
    if root.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false) {
        if let Ok(rd) = std::fs::read_dir(root) {
            let mut kids: Vec<_> = rd.flatten().map(|e| e.path()).collect();
            kids.sort();
            for k in kids {
                walk(&k, base, out);
            }
        }
    }
}

fn compress(sources: &[PathBuf], out: &Path, opts: &CompressOpts, job: &Job) -> io::Result<()> {
    job.total.store(sources.iter().map(|s| tree_size(s)).sum(), Ordering::Relaxed);
    let mut items = Vec::new();
    for s in sources {
        let base = s.parent().unwrap_or(Path::new("/"));
        walk(s, base, &mut items);
    }
    let tmp = out.with_file_name(format!(
        ".{}.part",
        out.file_name().unwrap_or_default().to_string_lossy()
    ));
    let lvl = |fast: i32, normal: i32, max: i32| match opts.level {
        Level::Store => 0,
        Level::Fast => fast,
        Level::Normal => normal,
        Level::Max => max,
    };
    let r = (|| -> io::Result<()> {
        if opts.format == ArchiveFormat::SevenZ {
            return compress_7z(sources, &tmp, opts, job);
        }
        let file = File::create(&tmp)?;
        match opts.format {
            ArchiveFormat::Zip => write_zip(file, &items, opts, job),
            ArchiveFormat::TarGz => {
                let enc = flate2::write::GzEncoder::new(file, flate2::Compression::new(lvl(1, 6, 9) as u32));
                write_tar(enc, &items, job)?.finish()?;
                Ok(())
            }
            ArchiveFormat::TarXz => {
                let enc = xz2::write::XzEncoder::new(file, lvl(1, 6, 9) as u32);
                write_tar(enc, &items, job)?.finish()?;
                Ok(())
            }
            ArchiveFormat::TarZst => {
                let mut enc = zstd::Encoder::new(file, lvl(1, 6, 19))?;
                let _ = enc.multithread(std::thread::available_parallelism().map_or(1, |n| n.get() as u32));
                write_tar(enc, &items, job)?.finish()?;
                Ok(())
            }
            ArchiveFormat::SevenZ => unreachable!(),
        }
    })();
    let r = r.and_then(|()| match opts.split {
        Some(size) if std::fs::metadata(&tmp)?.len() > size => split_volumes(&tmp, out, size),
        _ => std::fs::rename(&tmp, out),
    });
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// Whether the `7z` tool is on PATH (needed to create 7z archives).
pub fn have_7z() -> bool {
    static HAVE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAVE.get_or_init(|| {
        std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("7z").is_file()))
    })
}

/// Volume `n` (1-based) of a split archive: `name.7z.001`.
pub fn volume_path(out: &Path, n: usize) -> PathBuf {
    let mut s = out.as_os_str().to_owned();
    s.push(format!(".{n:03}"));
    PathBuf::from(s)
}

/// Cut `tmp` into `out.001`, `out.002`… (the layout 7-Zip and others read).
fn split_volumes(tmp: &Path, out: &Path, size: u64) -> io::Result<()> {
    let mut src = File::open(tmp)?;
    let mut made = Vec::new();
    let r = (|| {
        for n in 1.. {
            let path = volume_path(out, n);
            let mut part = File::create(&path)?;
            made.push(path);
            let copied = io::copy(&mut (&mut src).take(size), &mut part)?;
            if copied < size {
                if copied == 0 && n > 1 {
                    let _ = std::fs::remove_file(made.pop().unwrap());
                }
                break;
            }
        }
        std::fs::remove_file(tmp)
    })();
    if r.is_err() {
        for p in made {
            let _ = std::fs::remove_file(p);
        }
    }
    r
}

/// 7z archives are made by the `7z` tool (libarchive can't encrypt them).
fn compress_7z(sources: &[PathBuf], tmp: &Path, opts: &CompressOpts, job: &Job) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let mx = match opts.level {
        Level::Store => 0,
        Level::Fast => 1,
        Level::Normal => 5,
        Level::Max => 9,
    };
    let mut cmd = Command::new("7z");
    cmd.args(["a", "-t7z", "-y", "-snl", "-bsp1", "-bso0", "-bse1"]).arg(format!("-mx={mx}"));
    if opts.level != Level::Store {
        cmd.arg(format!("-m0={}", opts.method.seven_z()));
    }
    if let Some(pw) = &opts.password {
        cmd.arg(format!("-p{pw}"));
        if opts.encrypt_names {
            cmd.arg("-mhe=on");
        }
    }
    cmd.arg(tmp).arg("--").args(sources);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| io::Error::other(format!("7z: {e} ({})", tr("install 7zip to create 7z archives"))))?;
    // Progress comes as "  42% 3 + name" redrawn with backspaces.
    let total = job.total.load(Ordering::Relaxed);
    let mut stdout = child.stdout.take().unwrap();
    let err_reader = {
        let mut stderr = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            s
        })
    };
    std::thread::scope(|sc| {
        sc.spawn(|| {
            let mut buf = [0u8; 4096];
            let mut digits = String::new();
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for &b in &buf[..n] {
                    match b {
                        b'0'..=b'9' => digits.push(b as char),
                        b'%' => {
                            if let Ok(pct) = digits.parse::<u64>() {
                                job.done.store(total * pct.min(100) / 100, Ordering::Relaxed);
                            }
                            digits.clear();
                        }
                        _ => digits.clear(),
                    }
                }
            }
        });
        let status = loop {
            if job.cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            if let Some(st) = child.try_wait()? {
                break st;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let err = err_reader.join().unwrap_or_default();
        if !status.success() {
            let msg = err.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("; ");
            return Err(io::Error::other(format!("7z: {}", if msg.is_empty() { "failed".into() } else { msg })));
        }
        Ok(())
    })
}

fn write_tar<W: Write>(w: W, items: &[(PathBuf, String)], job: &Job) -> io::Result<W> {
    let mut b = tar::Builder::new(w);
    b.follow_symlinks(false);
    for (abs, rel) in items {
        job.now_on(&abs);
        let m = abs.symlink_metadata()?;
        if m.is_file() {
            let mut h = tar::Header::new_gnu();
            h.set_metadata(&m);
            let f = Counting { inner: File::open(abs)?, job };
            b.append_data(&mut h, rel, f)?;
        } else {
            b.append_path_with_name(abs, rel)?;
        }
    }
    b.into_inner()
}

fn write_zip(file: File, items: &[(PathBuf, String)], opts: &CompressOpts, job: &Job) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod as M;
    let mut z = zip::ZipWriter::new(file);
    let zerr = |e: zip::result::ZipError| io::Error::other(e.to_string());
    let (method, level) = match (opts.level, opts.method) {
        (Level::Store, _) => (M::Stored, None),
        (l, Method::Bzip2) => (M::Bzip2, Some(match l { Level::Fast => 1, Level::Max => 9, _ => 6 })),
        (l, Method::Zstd) => (M::Zstd, Some(match l { Level::Fast => 1, Level::Max => 19, _ => 6 })),
        (l, _) => (M::Deflated, Some(match l { Level::Fast => 1, Level::Max => 9, _ => 6 })),
    };
    for (abs, rel) in items {
        job.now_on(&abs);
        let m = abs.symlink_metadata()?;
        let mut o = SimpleFileOptions::default()
            .unix_permissions(m.permissions().mode() & 0o777)
            .large_file(m.len() >= 0xFFFF_FFFF);
        if m.is_dir() {
            z.add_directory(rel.as_str(), o).map_err(zerr)?;
        } else if m.file_type().is_symlink() {
            let target = std::fs::read_link(abs)?;
            z.add_symlink(rel.as_str(), target.to_string_lossy().as_ref(), o).map_err(zerr)?;
        } else {
            o = o.compression_method(method).compression_level(level);
            if let Some(pw) = &opts.password {
                o = o.with_aes_encryption(zip::AesMode::Aes256, pw);
            }
            z.start_file(rel.as_str(), o).map_err(zerr)?;
            io::copy(&mut Counting { inner: File::open(abs)?, job }, &mut z)?;
        }
    }
    z.finish().map_err(zerr)?;
    Ok(())
}

/// Extract a whole archive next to it. A single top-level folder is extracted
/// as is; otherwise everything goes into a new folder named after the archive.
fn extract_all(archive: &Path, dest: &Path, job: &Job) -> io::Result<()> {
    let idx = vfs::archive_index(archive)?;
    job.total.store(archive_subtree_size(archive, ""), Ordering::Relaxed);
    let root = idx.dirs.get("").cloned().unwrap_or_default();
    let stage = staging_dir(dest)?;
    let r = (|| {
        let single_dir = root.len() == 1 && root[0].is_dir;
        let into = if single_dir {
            stage.clone()
        } else {
            let name = crate::volumes::archive_name(archive);
            let d = stage.join(split_ext(&name).0);
            std::fs::create_dir_all(&d)?;
            d
        };
        vfs::extract(archive, &[String::new()], &into, |n| {
            job.done.store(n, Ordering::Relaxed);
            !job.cancel.load(Ordering::Relaxed)
        })?;
        place_from_stage(&stage, dest, Conflict::Rename, job)
    })();
    let _ = std::fs::remove_dir_all(&stage);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(job: &Job) -> Result<(), String> {
        while job.finished_at.lock().unwrap().is_none() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        job.result.lock().unwrap().clone().unwrap()
    }

    fn run_op(op: Op) {
        wait(&spawn(op, || {})).unwrap();
    }

    fn names(loc: &Location) -> Vec<String> {
        let mut v: Vec<String> = vfs::list(loc).unwrap().into_iter().map(|e| e.name).collect();
        v.sort();
        v
    }

    fn read_inner(archive: &Path, inner: &str) -> String {
        std::fs::read_to_string(vfs::extract_to_temp_fresh(archive, inner).unwrap()).unwrap()
    }

    fn arc(a: &Path, inner: &str) -> Location {
        Location::Archive { archive: a.to_path_buf(), inner: inner.into() }
    }

    fn dest(a: &Path, inner: &str) -> Dest {
        Dest::Archive { archive: a.to_path_buf(), inner: inner.into() }
    }

    #[test]
    fn archive_editing() {
        let mut formats: Vec<&str> = vec![".zip", ".tar.gz", ".tar.zst", ".tar.xz"];
        let have_bsdtar = std::process::Command::new("bsdtar").arg("--version").output().is_ok();
        if have_bsdtar {
            formats.push(".7z");
        }
        for ext in formats {
            let dir = std::env::temp_dir().join(format!("polka-edit-{}-{ext}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("seed/sub")).unwrap();
            std::fs::write(dir.join("seed/a.txt"), "A").unwrap();
            std::fs::write(dir.join("seed/sub/b.txt"), "B").unwrap();
            let a = dir.join(format!("t{ext}"));
            if ext == ".7z" {
                let ok = std::process::Command::new("bsdtar")
                    .args(["--format", "7zip", "-cf"])
                    .arg(&a)
                    .args(["-C", dir.join("seed").to_str().unwrap(), "a.txt", "sub"])
                    .status()
                    .unwrap()
                    .success();
                assert!(ok);
            } else {
                let fmt = ArchiveFormat::ALL.into_iter().find(|f| f.ext() == ext).unwrap();
                let srcs = vec![dir.join("seed/a.txt"), dir.join("seed/sub")];
                run_op(Op::Compress { sources: srcs, out: a.clone(), opts: CompressOpts::new(fmt) });
            }
            let magic = |p: &Path| std::fs::read(p).unwrap()[..4].to_vec();
            let before = magic(&a);
            assert_eq!(names(&arc(&a, "")), ["a.txt", "sub"], "{ext}");

            // Add a local file, then the same name with each conflict policy.
            std::fs::write(dir.join("new.txt"), "N1").unwrap();
            run_op(Op::Copy { sources: vec![Location::Local(dir.join("new.txt"))], dest: dest(&a, "sub"), policy: Conflict::Rename });
            assert_eq!(read_inner(&a, "sub/new.txt"), "N1", "{ext}");
            std::fs::write(dir.join("new.txt"), "N2").unwrap();
            let src = vec![Location::Local(dir.join("new.txt"))];
            assert_eq!(conflicts(&src, &dest(&a, "sub")), ["new.txt"]);
            run_op(Op::Copy { sources: src.clone(), dest: dest(&a, "sub"), policy: Conflict::Skip });
            assert_eq!(read_inner(&a, "sub/new.txt"), "N1");
            run_op(Op::Copy { sources: src.clone(), dest: dest(&a, "sub"), policy: Conflict::Rename });
            assert_eq!(read_inner(&a, "sub/new (2).txt"), "N2");
            run_op(Op::Copy { sources: src.clone(), dest: dest(&a, "sub"), policy: Conflict::Overwrite });
            assert_eq!(read_inner(&a, "sub/new.txt"), "N2");
            assert_eq!(names(&arc(&a, "sub")), ["b.txt", "new (2).txt", "new.txt"], "{ext}");

            // Rename a folder, make one, delete a file.
            let edit = |plan| run_op(Op::ArchiveEdit { archive: a.clone(), plan, label: "" });
            edit(Plan { rename: vec![("sub".into(), "moved".into())], ..Default::default() });
            edit(Plan { mkdir: vec!["moved/empty".into()], ..Default::default() });
            edit(Plan { remove: vec!["moved/new (2).txt".into()], ..Default::default() });
            assert_eq!(names(&arc(&a, "")), ["a.txt", "moved"], "{ext}");
            assert_eq!(names(&arc(&a, "moved")), ["b.txt", "empty", "new.txt"], "{ext}");
            assert_eq!(read_inner(&a, "moved/b.txt"), "B");

            // Move within the archive, then out of it.
            run_op(Op::Move { sources: vec![arc(&a, "a.txt")], dest: dest(&a, "moved"), policy: Conflict::Rename });
            assert_eq!(names(&arc(&a, "")), ["moved"], "{ext}");
            let out = dir.join("out");
            std::fs::create_dir_all(&out).unwrap();
            run_op(Op::Move { sources: vec![arc(&a, "moved/b.txt")], dest: Dest::Dir(out.clone()), policy: Conflict::Rename });
            assert_eq!(std::fs::read_to_string(out.join("b.txt")).unwrap(), "B");
            assert_eq!(names(&arc(&a, "moved")), ["a.txt", "empty", "new.txt"], "{ext}");

            // Same container format and compression as before.
            assert_eq!(magic(&a), before, "{ext}");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn local_conflicts_merge_and_skip() {
        let dir = std::env::temp_dir().join(format!("polka-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (p, c) in [("src/d/new.txt", "new"), ("src/d/same.txt", "src"), ("dst/d/old.txt", "old"), ("dst/d/same.txt", "dst")] {
            std::fs::create_dir_all(dir.join(p).parent().unwrap()).unwrap();
            std::fs::write(dir.join(p), c).unwrap();
        }
        let src = vec![Location::Local(dir.join("src/d"))];
        let dst = Dest::Dir(dir.join("dst"));
        run_op(Op::Copy { sources: src.clone(), dest: dst.clone(), policy: Conflict::Skip });
        assert_eq!(std::fs::read_to_string(dir.join("dst/d/same.txt")).unwrap(), "dst");
        run_op(Op::Copy { sources: src.clone(), dest: dst.clone(), policy: Conflict::Overwrite });
        assert_eq!(std::fs::read_to_string(dir.join("dst/d/same.txt")).unwrap(), "src");
        assert!(dir.join("dst/d/old.txt").exists() && dir.join("dst/d/new.txt").exists());
        // Move-merge removes the source.
        run_op(Op::Move { sources: src, dest: dst, policy: Conflict::Overwrite });
        assert!(!dir.join("src/d").exists());
        // Copy into its own folder always keeps both.
        let f = vec![Location::Local(dir.join("dst/d/old.txt"))];
        run_op(Op::Copy { sources: f, dest: Dest::Dir(dir.join("dst/d")), policy: Conflict::Overwrite });
        assert!(dir.join("dst/d/old (2).txt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
