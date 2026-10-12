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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarZst,
    TarGz,
    TarXz,
}

impl ArchiveFormat {
    pub const ALL: [ArchiveFormat; 4] =
        [ArchiveFormat::Zip, ArchiveFormat::TarZst, ArchiveFormat::TarGz, ArchiveFormat::TarXz];
    pub fn ext(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => ".zip",
            ArchiveFormat::TarZst => ".tar.zst",
            ArchiveFormat::TarGz => ".tar.gz",
            ArchiveFormat::TarXz => ".tar.xz",
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

pub enum Op {
    Copy { sources: Vec<Location>, dest: Dest, policy: Conflict },
    Move { sources: Vec<Location>, dest: Dest, policy: Conflict },
    Trash { paths: Vec<PathBuf> },
    Delete { paths: Vec<PathBuf> },
    Compress { sources: Vec<PathBuf>, out: PathBuf, format: ArchiveFormat },
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
        Op::Compress { out, .. } => {
            (tr("Compressing"), vec![Location::Local(out.parent().unwrap_or(Path::new("/")).into())])
        }
        Op::Extract { dest, .. } => (tr("Extracting"), vec![Location::Local(dest.clone())]),
        Op::ArchiveEdit { archive, label, .. } => {
            (*label, vec![Location::Archive { archive: archive.clone(), inner: String::new() }])
        }
    };
    let job = Arc::new(Job {
        label: label.to_string(),
        done: AtomicU64::new(0),
        total: AtomicU64::new(0),
        cancel: AtomicBool::new(false),
        result: Mutex::new(None),
        finished_at: Mutex::new(None),
        touches,
    });
    let j = job.clone();
    std::thread::spawn(move || {
        let r = run(op, &j).map_err(|e| e.to_string());
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
            job.total.store(paths.len() as u64, Ordering::Relaxed);
            trash::delete_all(&paths).map_err(|e| io::Error::other(e.to_string()))?;
            job.done.store(paths.len() as u64, Ordering::Relaxed);
            Ok(())
        }
        Op::Delete { paths } => {
            job.total.store(paths.len() as u64, Ordering::Relaxed);
            for p in paths {
                remove_any(&p)?;
                job.tick(1)?;
            }
            Ok(())
        }
        Op::Compress { sources, out, format } => compress(&sources, &out, format, job),
        Op::Extract { archive, dest } => extract_all(&archive, &dest, job),
        Op::ArchiveEdit { archive, plan, .. } => {
            job.total.store(archive_rw::estimate(&archive, &plan), Ordering::Relaxed);
            archive_rw::rewrite(&archive, &plan, |n| {
                job.done.store(n, Ordering::Relaxed);
                !job.cancel.load(Ordering::Relaxed)
            })
        }
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
            Location::Search { .. } => 0,
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
            Location::Search { .. } => {}
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
                Location::Search { .. } => false,
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
                Location::Search { .. } => {}
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

fn compress(sources: &[PathBuf], out: &Path, format: ArchiveFormat, job: &Job) -> io::Result<()> {
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
    let r = (|| -> io::Result<()> {
        let file = File::create(&tmp)?;
        match format {
            ArchiveFormat::Zip => write_zip(file, &items, job),
            ArchiveFormat::TarGz => {
                let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
                write_tar(enc, &items, job)?.finish()?;
                Ok(())
            }
            ArchiveFormat::TarXz => {
                let enc = xz2::write::XzEncoder::new(file, 6);
                write_tar(enc, &items, job)?.finish()?;
                Ok(())
            }
            ArchiveFormat::TarZst => {
                let mut enc = zstd::Encoder::new(file, 6)?;
                let _ = enc.multithread(std::thread::available_parallelism().map_or(1, |n| n.get() as u32));
                write_tar(enc, &items, job)?.finish()?;
                Ok(())
            }
        }
    })();
    match r {
        Ok(()) => std::fs::rename(&tmp, out),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn write_tar<W: Write>(w: W, items: &[(PathBuf, String)], job: &Job) -> io::Result<W> {
    let mut b = tar::Builder::new(w);
    b.follow_symlinks(false);
    for (abs, rel) in items {
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

fn write_zip(file: File, items: &[(PathBuf, String)], job: &Job) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use zip::write::SimpleFileOptions;
    let mut z = zip::ZipWriter::new(file);
    let zerr = |e: zip::result::ZipError| io::Error::other(e.to_string());
    for (abs, rel) in items {
        let m = abs.symlink_metadata()?;
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(m.permissions().mode() & 0o777)
            .large_file(m.len() >= 0xFFFF_FFFF);
        if m.is_dir() {
            z.add_directory(rel.as_str(), opts).map_err(zerr)?;
        } else if m.file_type().is_symlink() {
            let target = std::fs::read_link(abs)?;
            z.add_symlink(rel.as_str(), target.to_string_lossy().as_ref(), opts).map_err(zerr)?;
        } else {
            z.start_file(rel.as_str(), opts).map_err(zerr)?;
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
            let name = archive.file_name().unwrap_or_default().to_string_lossy().into_owned();
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
                run_op(Op::Compress { sources: srcs, out: a.clone(), format: fmt });
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
