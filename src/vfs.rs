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
    /// Recursive name search under `root`. Entry names are paths relative
    /// to `root`.
    Search { root: PathBuf, query: String, hidden: bool },
}

/// Somewhere files can be put.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dest {
    Dir(PathBuf),
    Archive { archive: PathBuf, inner: String },
}

impl Dest {
    pub fn location(&self) -> Location {
        match self {
            Dest::Dir(p) => Location::Local(p.clone()),
            Dest::Archive { archive, inner } => Location::Archive { archive: archive.clone(), inner: inner.clone() },
        }
    }
}

impl Location {
    pub fn display(&self) -> String {
        match self {
            Location::Local(p) => p.display().to_string(),
            Location::Archive { archive, inner } if inner.is_empty() => {
                archive.display().to_string()
            }
            Location::Archive { archive, inner } => format!("{}/{}", archive.display(), inner),
            Location::Search { root, query, .. } => format!("{} — \u{1F50D} {query}", root.display()),
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
            Location::Search { query, .. } => format!("\u{1F50D} {query}"),
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

    /// The archive file this location is inside, if any.
    pub fn archive(&self) -> Option<&Path> {
        match self {
            Location::Archive { archive, .. } => Some(archive),
            _ => None,
        }
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
            Location::Search { root, .. } => Some((Location::Local(root.clone()), String::new())),
        }
    }

    pub fn join(&self, name: &str) -> Location {
        match self {
            Location::Local(p) => Location::Local(p.join(name)),
            Location::Archive { archive, inner } => Location::Archive {
                archive: archive.clone(),
                inner: if inner.is_empty() { name.to_string() } else { format!("{inner}/{name}") },
            },
            Location::Search { root, .. } => Location::Local(root.join(name)),
        }
    }

    /// The directory itself, when it is a real one.
    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Location::Local(p) => Some(p),
            _ => None,
        }
    }

    /// Real path of an entry listed here (None inside archives).
    pub fn entry_path(&self, name: &str) -> Option<PathBuf> {
        match self {
            Location::Local(p) => Some(p.join(name)),
            Location::Search { root, .. } => Some(root.join(name)),
            Location::Archive { .. } => None,
        }
    }

    /// Where pasted files would go, if this location accepts them.
    pub fn dest(&self) -> Option<Dest> {
        match self {
            Location::Local(p) => Some(Dest::Dir(p.clone())),
            Location::Archive { archive, inner } => {
                let name = archive.file_name()?.to_string_lossy();
                // Nested archives are temporary copies; edits would be lost.
                (crate::archive_rw::writable_name(&name) && !archive.starts_with(temp_root()))
                    .then(|| Dest::Archive { archive: archive.clone(), inner: inner.clone() })
            }
            Location::Search { .. } => None,
        }
    }

    /// Same place for refresh purposes (any folder of the same archive counts).
    pub fn affected_by(&self, changed: &Location) -> bool {
        match (self, changed) {
            (Location::Archive { archive: a, .. }, Location::Archive { archive: b, .. }) => a == b,
            (Location::Search { root, .. }, Location::Local(p)) => p.starts_with(root),
            _ => self == changed,
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
        self.name.rsplit('/').next().unwrap_or(&self.name).starts_with('.')
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
        Location::Search { root, query, hidden } => Ok(search(root, query, *hidden)),
    }
}

const SEARCH_MAX_RESULTS: usize = 5000;
const SEARCH_MAX_TIME: std::time::Duration = std::time::Duration::from_secs(30);

/// Case-insensitive substring match on names; `*` matches anything.
pub fn name_matches(name: &str, query: &str) -> bool {
    let name = name.to_lowercase();
    let mut rest = name.as_str();
    let parts: Vec<String> = query.to_lowercase().split('*').map(str::to_string).collect();
    if parts.len() == 1 {
        return name.contains(&parts[0]);
    }
    for p in &parts {
        match rest.find(p.as_str()) {
            Some(i) => rest = &rest[i + p.len()..],
            None => return false,
        }
    }
    true
}

/// Breadth-first, so near results come first; doesn't follow symlinks or
/// cross into other file systems.
fn search(root: &Path, query: &str, hidden: bool) -> Vec<Entry> {
    let started = std::time::Instant::now();
    let dev = std::fs::metadata(root).map(|m| m.dev()).ok();
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::from([PathBuf::new()]);
    while let Some(rel) = queue.pop_front() {
        if out.len() >= SEARCH_MAX_RESULTS || started.elapsed() > SEARCH_MAX_TIME {
            break;
        }
        let Ok(entries) = list_local(&root.join(&rel)) else { continue };
        for mut e in entries {
            if !hidden && e.hidden() {
                continue;
            }
            let rel_name = if rel.as_os_str().is_empty() { e.name.clone() } else { format!("{}/{}", rel.display(), e.name) };
            if e.is_dir && !e.is_link {
                let same_fs = std::fs::metadata(root.join(&rel_name)).map(|m| Some(m.dev()) == dev).unwrap_or(false);
                if same_fs {
                    queue.push_back(PathBuf::from(&rel_name));
                }
            }
            if name_matches(&e.name, query) {
                e.name = rel_name;
                out.push(e);
            }
        }
    }
    out
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

pub fn invalidate(archive: &Path) {
    cache().lock().unwrap().remove(archive);
}

// ---------------------------------------------------------------- passwords
//
// libarchive decrypts zip (ZipCrypto, AES) itself but not 7z or rar; those
// go through the 7z tool. Passwords live in memory only, per archive path.

/// The archive needs a password (`wrong`: the one we had didn't work).
#[derive(Debug)]
pub struct NeedPassword {
    pub archive: PathBuf,
    pub wrong: bool,
}

impl std::fmt::Display for NeedPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = if self.wrong { crate::i18n::tr("Wrong password") } else { crate::i18n::tr("Password required") };
        write!(f, "{what}: {}", self.archive.display())
    }
}

impl std::error::Error for NeedPassword {}

pub fn need_password(e: &std::io::Error) -> Option<&NeedPassword> {
    e.get_ref()?.downcast_ref::<NeedPassword>()
}

fn passwords() -> &'static Mutex<HashMap<PathBuf, String>> {
    static P: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

pub fn set_password(archive: &Path, pw: &str) {
    passwords().lock().unwrap().insert(archive.to_path_buf(), pw.to_string());
}

fn password(archive: &Path) -> Option<String> {
    passwords().lock().unwrap().get(archive).cloned()
}

enum Encrypted {
    /// libarchive wants a passphrase it didn't get (or got a wrong one).
    Passphrase,
    /// libarchive can't decrypt this format at all (7z, rar): use 7z.
    Unsupported,
}

fn classify(msg: &str) -> Option<Encrypted> {
    let m = msg.to_lowercase();
    if m.contains("passphrase") {
        Some(Encrypted::Passphrase)
    } else if m.contains("encrypt") {
        Some(Encrypted::Unsupported)
    } else {
        None
    }
}

fn need(archive: &Path, wrong: bool) -> std::io::Error {
    std::io::Error::other(NeedPassword { archive: archive.to_path_buf(), wrong })
}

/// Run the 7z tool; a "Wrong password" in its output becomes NeedPassword.
fn run_7z(archive: &Path, args: &[&std::ffi::OsStr]) -> std::io::Result<String> {
    let out = std::process::Command::new("7z")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| std::io::Error::other(format!("7z: {e} ({})", crate::i18n::tr("install 7zip for encrypted 7z/rar"))))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if text.contains("Wrong password") {
        return Err(need(archive, true));
    }
    if !out.status.success() {
        let err = text.lines().filter(|l| l.contains("ERROR")).collect::<Vec<_>>().join("; ");
        return Err(std::io::Error::other(format!("7z: {}", if err.is_empty() { "failed" } else { &err })));
    }
    Ok(text)
}

/// Headers via `7z l -slt` (for archives whose names are encrypted).
fn headers_7z(archive: &Path, pw: &str) -> std::io::Result<Vec<(String, u32, u64, i64)>> {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let p = format!("-p{pw}");
    let text = run_7z(archive, &["l".as_ref(), "-slt".as_ref(), p.as_ref(), archive.as_os_str()])?;
    let body = text.split_once("\n----------\n").map(|(_, b)| b).unwrap_or("");
    let mut out = Vec::new();
    for block in body.split("\n\n") {
        let field = |k: &str| block.lines().find_map(|l| l.strip_prefix(k)).map(str::trim);
        let Some(path) = field("Path = ") else { continue };
        let attrs = field("Attributes = ").unwrap_or("");
        let is_dir = attrs.starts_with('D') || field("Folder = ") == Some("+");
        // "-rw-r--r--" style permissions, if present.
        let perms = attrs.split_whitespace().find(|w| w.len() == 10).map_or(if is_dir { 0o755 } else { 0o644 }, |w| {
            w[1..].bytes().enumerate().fold(0u32, |m, (i, b)| if b != b'-' { m | 1 << (8 - i) } else { m })
        });
        let mode = perms | if is_dir { 0o040000 } else { 0o100000 };
        let size = field("Size = ").and_then(|s| s.parse().ok()).unwrap_or(0);
        let mtime = field("Modified = ")
            .and_then(|m| NaiveDateTime::parse_from_str(&m[..m.len().min(19)], "%Y-%m-%d %H:%M:%S").ok())
            .and_then(|d| Local.from_local_datetime(&d).single())
            .map_or(0, |d| d.timestamp());
        out.push((path.to_string(), mode, size, mtime));
    }
    Ok(out)
}

/// Extract with the 7z tool into `dest`, with the same "each item lands
/// under its own name" layout as `extract`.
fn extract_7z(archive: &Path, items: &[String], dest: &Path, pw: &str) -> std::io::Result<()> {
    let stage = dest.join(format!(".polka-7z-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;
    let r = (|| {
        let p = format!("-p{pw}");
        let o = format!("-o{}", stage.display());
        let roots: Vec<String> = items.iter().map(|i| normalize_inner(i)).collect();
        let whole = roots.iter().any(|r| r.is_empty());
        let mut args: Vec<&std::ffi::OsStr> = vec!["x".as_ref(), "-y".as_ref(), p.as_ref(), o.as_ref(), archive.as_os_str(), "--".as_ref()];
        if !whole {
            args.extend(roots.iter().map(|r| std::ffi::OsStr::new(r.as_str())));
        }
        run_7z(archive, &args)?;
        // libarchive may have left empty dirs / files before giving up on
        // the encrypted data: 7z's output replaces them.
        if whole {
            for e in std::fs::read_dir(&stage)?.flatten() {
                merge_into(&e.path(), &dest.join(e.file_name()))?;
            }
        } else {
            for r in &roots {
                let name = r.rsplit('/').next().unwrap_or(r);
                merge_into(&stage.join(r), &dest.join(name))?;
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&stage);
    r
}

/// Move `src` to `dst`, merging folders and replacing anything else.
fn merge_into(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Ok(dm) = dst.symlink_metadata() {
        if dm.is_dir() && src.is_dir() {
            for e in std::fs::read_dir(src)?.flatten() {
                merge_into(&e.path(), &dst.join(e.file_name()))?;
            }
            return Ok(());
        }
        if dm.is_dir() { std::fs::remove_dir_all(dst)? } else { std::fs::remove_file(dst)? }
    }
    std::fs::rename(src, dst)
}

pub fn archive_index(archive: &Path) -> std::io::Result<Arc<ArchiveIndex>> {
    let mtime = std::fs::metadata(archive).and_then(|m| m.modified()).ok();
    if let Some((t, idx)) = cache().lock().unwrap().get(archive) {
        if *t == mtime {
            return Ok(idx.clone());
        }
    }
    let headers = match headers_libarchive(archive) {
        Ok(h) => h,
        // Encrypted names (7z -mhe): only 7z with the password can list it.
        Err(e) if matches!(classify(&e.to_string()), Some(_)) => match password(archive) {
            Some(pw) => headers_7z(archive, &pw)?,
            None => return Err(need(archive, false)),
        },
        Err(e) => return Err(e),
    };
    let idx = Arc::new(build_index(headers));
    cache().lock().unwrap().insert(archive.to_path_buf(), (mtime, idx.clone()));
    Ok(idx)
}

fn headers_libarchive(archive: &Path) -> std::io::Result<Vec<(String, u32, u64, i64)>> {
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
    let v = std::mem::take(&mut *headers.lock().unwrap());
    Ok(v)
}

fn build_index(headers: Vec<(String, u32, u64, i64)>) -> ArchiveIndex {
    let mut files: HashMap<String, Entry> = HashMap::new();
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    dirs.insert(String::new());
    for (raw, mode, size, mtime) in headers {
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

    ArchiveIndex { dirs: out }
}

fn libc_s_ifmt() -> u32 {
    0o170000
}

/// Extract `items` (inner paths, files or dirs) from `archive` into `dest`.
/// Each item lands directly in `dest` under its own name, with its subtree.
/// `progress` gets the number of bytes written so far; returning `false`
/// cancels.
pub fn extract(archive: &Path, items: &[String], dest: &Path, progress: impl FnMut(u64) -> bool) -> std::io::Result<()> {
    let pw = password(archive);
    match extract_libarchive(archive, items, dest, progress, pw.as_deref()) {
        Ok(()) => Ok(()),
        Err(e) => match (classify(&e.to_string()), pw) {
            (Some(Encrypted::Passphrase), pw) => Err(need(archive, pw.is_some())),
            (Some(Encrypted::Unsupported), Some(pw)) => extract_7z(archive, items, dest, &pw),
            (Some(Encrypted::Unsupported), None) => Err(need(archive, false)),
            (None, _) => Err(e),
        },
    }
}

fn extract_libarchive(
    archive: &Path,
    items: &[String],
    dest: &Path,
    mut progress: impl FnMut(u64) -> bool,
    pw: Option<&str>,
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
    let mut builder = ArchiveIteratorBuilder::new(File::open(archive)?).filter(move |name, _| m2(name).is_some());
    if let Some(pw) = pw {
        builder = builder.with_password(compress_tools::ArchivePassword::new(pw).map_err(io_err)?);
    }
    let mut iter = builder.build().map_err(io_err)?;

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
    if let Err(e) = extract(archive, &[inner.to_string()], &dir, |_| true) {
        // A failed attempt (e.g. no password yet) must not look extracted.
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok(target)
}

/// Like `extract_to_temp` but never reuses an earlier extraction (tests
/// read the same path after the archive changed).
#[cfg(test)]
pub fn extract_to_temp_fresh(archive: &Path, inner: &str) -> std::io::Result<PathBuf> {
    let dir = temp_root().join(format!("fresh-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir)?;
    extract(archive, &[inner.to_string()], &dir, |_| true)?;
    Ok(dir.join(normalize_inner(inner).rsplit('/').next().unwrap_or("file")))
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
            wait(&ops::spawn(Op::Copy { sources: vec![src.clone()], dest: Dest::Dir(dest.clone()), policy: ops::Conflict::Rename }, || {}));
            assert_eq!(std::fs::read_to_string(dest.join("a/b/deep.txt")).unwrap(), "deep");
            // Second copy gets a unique name.
            wait(&ops::spawn(Op::Copy { sources: vec![src], dest: Dest::Dir(dest.clone()), policy: ops::Conflict::Rename }, || {}));
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

#[cfg(test)]
mod password_tests {
    use super::*;

    fn make(dir: &Path, name: &str, args: &[&str]) -> PathBuf {
        let out = dir.join(name);
        let ok = std::process::Command::new("7z")
            .arg("a")
            .args(args)
            .arg(&out)
            .arg("src")
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "7z a {name}");
        out
    }

    fn read(archive: &Path, inner: &str) -> std::io::Result<String> {
        let d = temp_root().join(format!("pwtest-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d)?;
        extract(archive, &[inner.to_string()], &d, |_| true)?;
        let name = inner.rsplit('/').next().unwrap();
        Ok(std::fs::read_to_string(d.join(name))?)
    }

    fn needs(r: std::io::Result<impl std::fmt::Debug>) -> Option<bool> {
        r.err().and_then(|e| need_password(&e).map(|n| n.wrong))
    }

    #[test]
    fn encrypted_archives() {
        if std::process::Command::new("7z").arg("i").output().is_err() {
            eprintln!("7z not installed, skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("polka-pw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/sub")).unwrap();
        std::fs::write(dir.join("src/a.txt"), "secret").unwrap();
        std::fs::write(dir.join("src/sub/b.txt"), "deep").unwrap();
        let cases = [
            make(&dir, "zc.zip", &["-tzip", "-pqwe", "-mem=ZipCrypto"]),
            make(&dir, "aes.zip", &["-tzip", "-pqwe", "-mem=AES256"]),
            make(&dir, "plain-names.7z", &["-pqwe"]),
            make(&dir, "hidden-names.7z", &["-pqwe", "-mhe=on"]),
        ];
        for a in &cases {
            let hidden = a.to_string_lossy().contains("hidden");
            if hidden {
                // Even the listing needs the password.
                assert_eq!(needs(archive_index(a).map(|_| ())), Some(false), "{a:?}");
            } else {
                assert!(archive_index(a).unwrap().dirs.contains_key("src/sub"), "{a:?}");
            }
            assert_eq!(needs(read(a, "src/a.txt")), Some(false), "{a:?} without password");
            set_password(a, "nope");
            assert_eq!(needs(read(a, "src/a.txt")), Some(true), "{a:?} wrong password");
            set_password(a, "qwe");
            if hidden {
                let idx = archive_index(a).unwrap();
                assert!(idx.dirs["src"].iter().any(|e| e.name == "a.txt" && e.size == 6), "{a:?}");
            }
            assert_eq!(read(a, "src/a.txt").unwrap(), "secret", "{a:?}");
            // A whole folder, too.
            let d = dir.join(format!("out-{}", a.file_name().unwrap().to_string_lossy()));
            std::fs::create_dir_all(&d).unwrap();
            extract(a, &["src/sub".into()], &d, |_| true).unwrap();
            assert_eq!(std::fs::read_to_string(d.join("sub/b.txt")).unwrap(), "deep", "{a:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
