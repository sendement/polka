//! Virtual file system: a [`Location`] is either a real directory or a
//! directory inside an archive. Archives are indexed once (headers only, no
//! decompression) and then browsed like ordinary folders.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use compress_tools::{ArchiveContents, ArchiveIteratorBuilder};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Location {
    Local(PathBuf),
    /// `inner` has no leading or trailing slash; "" is the archive root.
    Archive { archive: PathBuf, inner: String },
}

impl Location {
    pub fn display(&self) -> String {
        match self {
            Location::Local(p) => p.display().to_string(),
            Location::Archive { archive, inner } if inner.is_empty() => {
                archive.display().to_string()
            }
            Location::Archive { archive, inner } => format!("{}/{}", archive.display(), inner),
        }
    }

    /// Last component, for tab titles.
    pub fn title(&self) -> String {
        let s = match self {
            Location::Local(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into()),
            Location::Archive { archive, inner } => match inner.rsplit('/').next() {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            },
        };
        if let (Location::Local(p), Some(home)) = (self, dirs::home_dir()) {
            if *p == home {
                return "~".into();
            }
        }
        s
    }

    pub fn is_archive(&self) -> bool {
        matches!(self, Location::Archive { .. })
    }

    /// Parent location and the name of `self` inside it (to restore the cursor).
    pub fn parent(&self) -> Option<(Location, String)> {
        match self {
            Location::Local(p) => {
                let name = p.file_name()?.to_string_lossy().into_owned();
                Some((Location::Local(p.parent()?.to_path_buf()), name))
            }
            Location::Archive { archive, inner } if inner.is_empty() => {
                let name = archive.file_name()?.to_string_lossy().into_owned();
                Some((Location::Local(archive.parent()?.to_path_buf()), name))
            }
            Location::Archive { archive, inner } => {
                let (parent, name) = match inner.rsplit_once('/') {
                    Some((p, n)) => (p.to_string(), n.to_string()),
                    None => (String::new(), inner.clone()),
                };
                Some((Location::Archive { archive: archive.clone(), inner: parent }, name))
            }
        }
    }

    pub fn join(&self, name: &str) -> Location {
        match self {
            Location::Local(p) => Location::Local(p.join(name)),
            Location::Archive { archive, inner } => Location::Archive {
                archive: archive.clone(),
                inner: if inner.is_empty() { name.to_string() } else { format!("{inner}/{name}") },
            },
        }
    }

    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Location::Local(p) => Some(p),
            Location::Archive { .. } => None,
        }
    }

    /// Breadcrumb segments: (label, location).
    pub fn crumbs(&self) -> Vec<(String, Location)> {
        let mut out = Vec::new();
        let mut cur = Some(self.clone());
        while let Some(loc) = cur {
            let label = match &loc {
                Location::Local(p) if p.as_os_str() == "/" => "/".to_string(),
                _ => loc.title(),
            };
            let stop = matches!(&loc, Location::Local(p) if Some(p.clone()) == dirs::home_dir());
            cur = if stop { None } else { loc.parent().map(|(p, _)| p) };
            out.push((label, loc));
        }
        out.reverse();
        out
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub mtime: Option<i64>,
    pub mode: u32,
}

impl Entry {
    pub fn hidden(&self) -> bool {
        self.name.starts_with('.')
    }
    pub fn executable(&self) -> bool {
        !self.is_dir && self.mode & 0o111 != 0
    }
}

const ARCHIVE_SUFFIXES: &[&str] = &[
    ".zip", ".7z", ".rar", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz",
    ".tar.zst", ".tzst", ".tar.lz4", ".tar.lzma", ".cab", ".iso", ".jar", ".apk", ".cpio",
    ".xpi", ".deb", ".pkg.tar.zst", ".pkg.tar.xz", ".lha", ".lzh", ".ar", ".a", ".cbz", ".cbr", ".cb7",
];

pub fn is_archive_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    ARCHIVE_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

pub fn list(loc: &Location) -> std::io::Result<Vec<Entry>> {
    match loc {
        Location::Local(p) => list_local(p),
        Location::Archive { archive, inner } => {
            let idx = archive_index(archive)?;
            Ok(idx.dirs.get(inner).cloned().unwrap_or_default())
        }
    }
}

fn list_local(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for de in std::fs::read_dir(dir)? {
        let Ok(de) = de else { continue };
        let name = de.file_name().to_string_lossy().into_owned();
        let Ok(lmeta) = de.path().symlink_metadata() else { continue };
        let is_link = lmeta.file_type().is_symlink();
        // Broken links still show up, with the link's own metadata.
        let meta = if is_link { std::fs::metadata(de.path()).unwrap_or(lmeta) } else { lmeta };
        out.push(Entry {
            name,
            is_dir: meta.is_dir(),
            is_link,
            size: meta.len(),
            mtime: Some(meta.mtime()),
            mode: meta.mode(),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- archives

pub struct ArchiveIndex {
    /// inner dir ("" = root) -> entries
    pub dirs: HashMap<String, Vec<Entry>>,
}

type IndexCache = Mutex<HashMap<PathBuf, (Option<SystemTime>, Arc<ArchiveIndex>)>>;

fn cache() -> &'static IndexCache {
    static C: OnceLock<IndexCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

pub fn normalize_inner(name: &str) -> String {
    let mut s = name.trim_start_matches("./").trim_start_matches('/');
    s = s.trim_end_matches('/');
    s.split('/').filter(|c| !c.is_empty() && *c != ".").collect::<Vec<_>>().join("/")
}

fn io_err(e: compress_tools::Error) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

pub fn archive_index(archive: &Path) -> std::io::Result<Arc<ArchiveIndex>> {
    let mtime = std::fs::metadata(archive).and_then(|m| m.modified()).ok();
    if let Some((t, idx)) = cache().lock().unwrap().get(archive) {
        if *t == mtime {
            return Ok(idx.clone());
        }
    }

    let headers: Arc<Mutex<Vec<(String, u32, u64, i64)>>> = Default::default();
    let sink = headers.clone();
    let file = File::open(archive)?;
    // Rejecting every entry in the filter makes libarchive skip the data, so
    // indexing only reads headers.
    let mut iter = ArchiveIteratorBuilder::new(file)
        .filter(move |name, st| {
            sink.lock().unwrap().push((
                name.to_string(),
                st.st_mode,
                st.st_size as u64,
                st.st_mtime,
            ));
            false
        })
        .build()
        .map_err(io_err)?;
    for item in &mut iter {
        if let ArchiveContents::Err(e) = item {
            return Err(io_err(e));
        }
    }
    drop(iter);

    let mut files: HashMap<String, Entry> = HashMap::new();
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    dirs.insert(String::new());
    for (raw, mode, size, mtime) in headers.lock().unwrap().drain(..) {
        let path = normalize_inner(&raw);
        if path.is_empty() {
            continue;
        }
        let is_dir = mode & libc_s_ifmt() == 0o040000 || raw.ends_with('/');
        // Every ancestor is an implicit directory.
        let mut acc = String::new();
        for comp in path.split('/').collect::<Vec<_>>().iter().rev().skip(1).rev() {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(comp);
            dirs.insert(acc.clone());
        }
        if is_dir {
            dirs.insert(path.clone());
        }
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        files.insert(
            path,
            Entry {
                name,
                is_dir,
                is_link: mode & libc_s_ifmt() == 0o120000,
                size: if is_dir { 0 } else { size },
                mtime: Some(mtime),
                mode,
            },
        );
    }

    let mut out: HashMap<String, Vec<Entry>> = HashMap::new();
    for d in &dirs {
        out.entry(d.clone()).or_default();
        if d.is_empty() {
            continue;
        }
        let (parent, name) = match d.rsplit_once('/') {
            Some((p, n)) => (p.to_string(), n.to_string()),
            None => (String::new(), d.clone()),
        };
        let e = files.remove(d).unwrap_or(Entry {
            name: name.clone(),
            is_dir: true,
            is_link: false,
            size: 0,
            mtime: None,
            mode: 0o040755,
        });
        out.entry(parent).or_default().push(Entry { is_dir: true, name, ..e });
    }
    for (path, e) in files {
        let parent = path.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default();
        out.entry(parent).or_default().push(e);
    }

    let idx = Arc::new(ArchiveIndex { dirs: out });
    cache().lock().unwrap().insert(archive.to_path_buf(), (mtime, idx.clone()));
    Ok(idx)
}

fn libc_s_ifmt() -> u32 {
    0o170000
}

/// Extract `items` (inner paths, files or dirs) from `archive` into `dest`.
/// Each item lands directly in `dest` under its own name, with its subtree.
/// `progress` gets the number of bytes written so far; returning `false`
/// cancels.
pub fn extract(
    archive: &Path,
    items: &[String],
    dest: &Path,
    mut progress: impl FnMut(u64) -> bool,
) -> std::io::Result<()> {
    // Map item prefix -> length of its parent part (stripped on output).
    let roots: Vec<(String, usize)> = items
        .iter()
        .map(|i| {
            let n = normalize_inner(i);
            let strip = n.rfind('/').map(|p| p + 1).unwrap_or(0);
            (n, strip)
        })
        .collect();
    let whole = roots.iter().any(|(r, _)| r.is_empty());
    let roots_f = roots.clone();
    let matches = move |name: &str| -> Option<usize> {
        let n = normalize_inner(name);
        if whole {
            return if n.is_empty() { None } else { Some(0) };
        }
        roots_f
            .iter()
            .find(|(r, _)| n == *r || (n.len() > r.len() && n.starts_with(r.as_str()) && n.as_bytes()[r.len()] == b'/'))
            .map(|(_, strip)| *strip)
    };
    let m2 = matches.clone();
    let mut iter = ArchiveIteratorBuilder::new(File::open(archive)?)
        .filter(move |name, _| m2(name).is_some())
        .build()
        .map_err(io_err)?;

    let mut written = 0u64;
    let mut out: Option<File> = None;
    let mut pending_mode = 0u32;
    let mut out_path = PathBuf::new();
    for item in &mut iter {
        match item {
            ArchiveContents::StartOfEntry(name, st) => {
                let n = normalize_inner(&name);
                let Some(strip) = matches(&name) else { continue };
                let rel = &n[strip.min(n.len())..];
                if rel.split('/').any(|c| c == "..") {
                    continue; // path traversal
                }
                let target = dest.join(rel);
                let is_dir = st.st_mode & libc_s_ifmt() == 0o040000 || name.ends_with('/');
                if is_dir {
                    std::fs::create_dir_all(&target)?;
                    out = None;
                } else if st.st_mode & libc_s_ifmt() == 0o120000 {
                    out = None; // symlinks are not reproduced
                } else {
                    if let Some(p) = target.parent() {
                        std::fs::create_dir_all(p)?;
                    }
                    out = Some(File::create(&target)?);
                    pending_mode = st.st_mode & 0o777;
                    out_path = target;
                }
            }
            ArchiveContents::DataChunk(chunk) => {
                if let Some(f) = out.as_mut() {
                    f.write_all(&chunk)?;
                    written += chunk.len() as u64;
                    if !progress(written) {
                        return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
                    }
                }
            }
            ArchiveContents::EndOfEntry => {
                if out.take().is_some() && pending_mode != 0 {
                    let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(pending_mode));
                }
            }
            ArchiveContents::Err(e) => return Err(io_err(e)),
        }
    }
    Ok(())
}

/// Extract one entry to a private temp dir so it can be opened by other apps
/// (or browsed, for nested archives). Returns the extracted path.
pub fn extract_to_temp(archive: &Path, inner: &str) -> std::io::Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    archive.hash(&mut h);
    inner.hash(&mut h);
    let dir = temp_root().join(format!("{:016x}", h.finish()));
    let name = normalize_inner(inner).rsplit('/').next().unwrap_or("file").to_string();
    let target = dir.join(&name);
    if target.exists() {
        return Ok(target);
    }
    std::fs::create_dir_all(&dir)?;
    extract(archive, &[inner.to_string()], &dir, |_| true)?;
    Ok(target)
}

pub fn temp_root() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("polka")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{self, ArchiveFormat, Op};

    fn tree(root: &Path) {
        std::fs::create_dir_all(root.join("src/a/b")).unwrap();
        std::fs::write(root.join("src/top.txt"), "top").unwrap();
        std::fs::write(root.join("src/a/mid.txt"), "mid").unwrap();
        std::fs::write(root.join("src/a/b/deep.txt"), "deep").unwrap();
    }

    fn wait(job: &ops::Job) {
        while job.finished_at.lock().unwrap().is_none() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if let Some(Err(e)) = job.result.lock().unwrap().as_ref() {
            panic!("job failed: {e}");
        }
    }

    #[test]
    fn archive_roundtrip_all_formats() {
        for f in ArchiveFormat::ALL {
            let dir = std::env::temp_dir().join(format!("polka-test-{}-{}", std::process::id(), f.ext()));
            let _ = std::fs::remove_dir_all(&dir);
            tree(&dir);
            let out = dir.join(format!("pack{}", f.ext()));
            wait(&ops::spawn(Op::Compress { sources: vec![dir.join("src")], out: out.clone(), format: f }, || {}));

            // Browse it like a folder.
            let root = list(&Location::Archive { archive: out.clone(), inner: String::new() }).unwrap();
            assert_eq!(root.len(), 1, "{f:?}: {root:?}");
            assert!(root[0].is_dir && root[0].name == "src");
            let mut a: Vec<_> = list(&Location::Archive { archive: out.clone(), inner: "src/a".into() })
                .unwrap()
                .into_iter()
                .map(|e| (e.name, e.is_dir))
                .collect();
            a.sort();
            assert_eq!(a, vec![("b".into(), true), ("mid.txt".into(), false)]);

            // Copy a subfolder out of the archive.
            let dest = dir.join("dest");
            std::fs::create_dir_all(&dest).unwrap();
            let src = Location::Archive { archive: out.clone(), inner: "src/a".into() };
            wait(&ops::spawn(Op::Copy { sources: vec![src.clone()], dest: dest.clone() }, || {}));
            assert_eq!(std::fs::read_to_string(dest.join("a/b/deep.txt")).unwrap(), "deep");
            // Second copy gets a unique name.
            wait(&ops::spawn(Op::Copy { sources: vec![src], dest: dest.clone() }, || {}));
            assert!(dest.join("a (2)/mid.txt").exists());

            // Extract-all with a single top folder lands as-is.
            let ex = dir.join("ex");
            std::fs::create_dir_all(&ex).unwrap();
            wait(&ops::spawn(Op::Extract { archive: out.clone(), dest: ex.clone() }, || {}));
            assert_eq!(std::fs::read_to_string(ex.join("src/top.txt")).unwrap(), "top");

            // Single file to temp, for opening.
            let t = extract_to_temp(&out, "src/a/b/deep.txt").unwrap();
            assert_eq!(std::fs::read_to_string(t).unwrap(), "deep");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn locations() {
        let l = Location::Archive { archive: "/x/y.zip".into(), inner: "a/b".into() };
        let (p, n) = l.parent().unwrap();
        assert_eq!(n, "b");
        assert_eq!(p, Location::Archive { archive: "/x/y.zip".into(), inner: "a".into() });
        let (p, n) = Location::Archive { archive: "/x/y.zip".into(), inner: "".into() }.parent().unwrap();
        assert_eq!((p, n.as_str()), (Location::Local("/x".into()), "y.zip"));
        assert_eq!(normalize_inner("./a//b/"), "a/b");
        assert_eq!(ops::split_ext("foo.tar.gz"), ("foo", ".tar.gz"));
        assert_eq!(ops::split_ext(".bashrc"), (".bashrc", ""));
    }
}
