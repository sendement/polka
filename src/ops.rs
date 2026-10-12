//! Background file operations with progress and cancellation.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::i18n::tr;
use crate::vfs::{self, Location};

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

pub enum Op {
    Copy { sources: Vec<Location>, dest: PathBuf },
    Move { sources: Vec<Location>, dest: PathBuf },
    Trash { paths: Vec<PathBuf> },
    Delete { paths: Vec<PathBuf> },
    Compress { sources: Vec<PathBuf>, out: PathBuf, format: ArchiveFormat },
    Extract { archive: PathBuf, dest: PathBuf },
}

pub fn spawn(op: Op, wake: impl Fn() + Send + 'static) -> Arc<Job> {
    let (label, touches) = match &op {
        Op::Copy { sources, dest } => (tr("Copying"), touched(sources, Some(dest))),
        Op::Move { sources, dest } => (tr("Moving"), touched(sources, Some(dest))),
        Op::Trash { paths } | Op::Delete { paths } => (
            tr("Deleting"),
            touched(&paths.iter().cloned().map(Location::Local).collect::<Vec<_>>(), None),
        ),
        Op::Compress { out, .. } => {
            (tr("Compressing"), vec![Location::Local(out.parent().unwrap_or(Path::new("/")).into())])
        }
        Op::Extract { dest, .. } => (tr("Extracting"), vec![Location::Local(dest.clone())]),
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

fn touched(sources: &[Location], dest: Option<&PathBuf>) -> Vec<Location> {
    let mut v: Vec<Location> = sources.iter().filter_map(|s| s.parent().map(|(p, _)| p)).collect();
    if let Some(d) = dest {
        v.push(Location::Local(d.clone()));
    }
    v.dedup();
    v
}

fn run(op: Op, job: &Job) -> io::Result<()> {
    match op {
        Op::Copy { sources, dest } => transfer(&sources, &dest, false, job),
        Op::Move { sources, dest } => transfer(&sources, &dest, true, job),
        Op::Trash { paths } => {
            job.total.store(paths.len() as u64, Ordering::Relaxed);
            trash::delete_all(&paths).map_err(|e| io::Error::other(e.to_string()))?;
            job.done.store(paths.len() as u64, Ordering::Relaxed);
            Ok(())
        }
        Op::Delete { paths } => {
            job.total.store(paths.len() as u64, Ordering::Relaxed);
            for p in paths {
                let m = p.symlink_metadata()?;
                if m.is_dir() { std::fs::remove_dir_all(&p)? } else { std::fs::remove_file(&p)? }
                job.tick(1)?;
            }
            Ok(())
        }
        Op::Compress { sources, out, format } => compress(&sources, &out, format, job),
        Op::Extract { archive, dest } => extract_all(&archive, &dest, job),
    }
}

/// `dir/name`, or `dir/name (2)`, `dir/name (3)`… if taken. Keeps compound
/// extensions like `.tar.gz` intact.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if first.symlink_metadata().is_err() {
        return first;
    }
    let (stem, ext) = split_ext(name);
    for i in 2.. {
        let p = dir.join(format!("{stem} ({i}){ext}"));
        if p.symlink_metadata().is_err() {
            return p;
        }
    }
    unreachable!()
}

pub fn split_ext(name: &str) -> (&str, &str) {
    let lower = name.to_lowercase();
    for compound in [".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".pkg.tar.zst"] {
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

fn transfer(sources: &[Location], dest: &Path, mv: bool, job: &Job) -> io::Result<()> {
    let mut total = 0;
    for s in sources {
        total += match s {
            Location::Local(p) => tree_size(p),
            Location::Archive { archive, inner } => archive_subtree_size(archive, inner),
        };
    }
    job.total.store(total, Ordering::Relaxed);

    for s in sources {
        match s {
            Location::Local(src) => {
                let name = src.file_name().ok_or_else(|| io::Error::other("bad source"))?;
                if dest.starts_with(src) {
                    return Err(io::Error::other(format!("cannot put {} inside itself", src.display())));
                }
                if mv && src.parent() == Some(dest) {
                    continue;
                }
                let target = unique_path(dest, &name.to_string_lossy());
                if mv && std::fs::rename(src, &target).is_ok() {
                    job.tick(tree_size(&target))?;
                    continue;
                }
                copy_tree(src, &target, job)?;
                if mv {
                    let m = src.symlink_metadata()?;
                    if m.is_dir() { std::fs::remove_dir_all(src)? } else { std::fs::remove_file(src)? }
                }
            }
            Location::Archive { archive, inner } => {
                if mv {
                    return Err(io::Error::other(tr("Archives are read-only for now")));
                }
                // Stage first so name clashes get resolved like local copies.
                let stage = staging_dir(dest)?;
                let base = job.done.load(Ordering::Relaxed);
                let r = vfs::extract(archive, std::slice::from_ref(inner), &stage, |n| {
                    job.done.store(base + n, Ordering::Relaxed);
                    !job.cancel.load(Ordering::Relaxed)
                });
                let r = r.and_then(|_| move_out_of_stage(&stage, dest));
                let _ = std::fs::remove_dir_all(&stage);
                r?;
            }
        }
    }
    Ok(())
}

fn staging_dir(dest: &Path) -> io::Result<PathBuf> {
    let stage = dest.join(format!(".polka-stage-{}", std::process::id()));
    std::fs::create_dir_all(&stage)?;
    Ok(stage)
}

fn move_out_of_stage(stage: &Path, dest: &Path) -> io::Result<()> {
    for e in std::fs::read_dir(stage)?.flatten() {
        let target = unique_path(dest, &e.file_name().to_string_lossy());
        std::fs::rename(e.path(), target)?;
    }
    Ok(())
}

fn archive_subtree_size(archive: &Path, inner: &str) -> u64 {
    let Ok(idx) = vfs::archive_index(archive) else { return 0 };
    fn walk(idx: &vfs::ArchiveIndex, dir: &str) -> u64 {
        idx.dirs.get(dir).map_or(0, |es| {
            es.iter()
                .map(|e| {
                    if e.is_dir {
                        walk(idx, &if dir.is_empty() { e.name.clone() } else { format!("{dir}/{}", e.name) })
                    } else {
                        e.size
                    }
                })
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

fn copy_tree(src: &Path, dst: &Path, job: &Job) -> io::Result<()> {
    let m = src.symlink_metadata()?;
    let ft = m.file_type();
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
        move_out_of_stage(&stage, dest)
    })();
    let _ = std::fs::remove_dir_all(&stage);
    r
}
