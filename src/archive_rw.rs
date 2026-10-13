//! Editing archives in place. libarchive can't modify an archive, so the
//! archive is streamed into a new one next to it (same format and filters),
//! dropping / renaming entries on the way and appending new ones, then the
//! new file atomically replaces the old.

use std::collections::HashSet;
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_void};
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::vfs::normalize_inner;

#[repr(C)]
struct Archive {
    _p: [u8; 0],
}
#[repr(C)]
struct ArchiveEntry {
    _p: [u8; 0],
}

#[link(name = "archive")]
unsafe extern "C" {
    fn archive_read_new() -> *mut Archive;
    fn archive_read_support_filter_all(a: *mut Archive) -> c_int;
    fn archive_read_support_format_all(a: *mut Archive) -> c_int;
    fn archive_read_open_filename(a: *mut Archive, name: *const c_char, block: usize) -> c_int;
    fn archive_read_set_callback_data(a: *mut Archive, data: *mut c_void) -> c_int;
    fn archive_read_set_read_callback(a: *mut Archive, f: ReadCb) -> c_int;
    fn archive_read_set_seek_callback(a: *mut Archive, f: SeekCb) -> c_int;
    fn archive_read_open1(a: *mut Archive) -> c_int;
    fn archive_read_next_header(a: *mut Archive, e: *mut *mut ArchiveEntry) -> c_int;
    fn archive_read_data(a: *mut Archive, buf: *mut c_void, len: usize) -> isize;
    fn archive_read_free(a: *mut Archive) -> c_int;
    fn archive_format(a: *mut Archive) -> c_int;
    fn archive_filter_count(a: *mut Archive) -> c_int;
    fn archive_filter_code(a: *mut Archive, n: c_int) -> c_int;
    fn archive_error_string(a: *mut Archive) -> *const c_char;

    fn archive_write_new() -> *mut Archive;
    fn archive_write_set_format(a: *mut Archive, code: c_int) -> c_int;
    fn archive_write_add_filter(a: *mut Archive, code: c_int) -> c_int;
    fn archive_write_set_format_filter_by_ext(a: *mut Archive, name: *const c_char) -> c_int;
    fn archive_write_open_filename(a: *mut Archive, name: *const c_char) -> c_int;
    fn archive_write_header(a: *mut Archive, e: *mut ArchiveEntry) -> c_int;
    fn archive_write_data(a: *mut Archive, buf: *const c_void, len: usize) -> isize;
    fn archive_write_finish_entry(a: *mut Archive) -> c_int;
    fn archive_write_close(a: *mut Archive) -> c_int;
    fn archive_write_free(a: *mut Archive) -> c_int;

    fn archive_entry_new() -> *mut ArchiveEntry;
    fn archive_entry_free(e: *mut ArchiveEntry);
    fn archive_entry_pathname(e: *mut ArchiveEntry) -> *const c_char;
    fn archive_entry_pathname_utf8(e: *mut ArchiveEntry) -> *const c_char;
    fn archive_entry_update_pathname_utf8(e: *mut ArchiveEntry, p: *const c_char) -> c_int;
    fn archive_entry_hardlink(e: *mut ArchiveEntry) -> *const c_char;
    fn archive_entry_update_hardlink_utf8(e: *mut ArchiveEntry, p: *const c_char) -> c_int;
    fn archive_entry_update_symlink_utf8(e: *mut ArchiveEntry, p: *const c_char) -> c_int;
    fn archive_entry_filetype(e: *mut ArchiveEntry) -> c_uint;
    fn archive_entry_set_filetype(e: *mut ArchiveEntry, t: c_uint);
    fn archive_entry_set_perm(e: *mut ArchiveEntry, p: c_uint);
    fn archive_entry_set_size(e: *mut ArchiveEntry, s: i64);
    fn archive_entry_size_is_set(e: *mut ArchiveEntry) -> c_int;
    fn archive_entry_size(e: *mut ArchiveEntry) -> i64;
    fn archive_entry_set_mtime(e: *mut ArchiveEntry, t: i64, ns: c_long);
    fn archive_entry_is_encrypted(e: *mut ArchiveEntry) -> c_int;
}

type ReadCb = unsafe extern "C" fn(*mut Archive, *mut c_void, *mut *const c_void) -> isize;
type SeekCb = unsafe extern "C" fn(*mut Archive, *mut c_void, i64, c_int) -> i64;

/// A Rust reader handed to libarchive (for split volumes read as one).
struct Feed {
    src: Box<dyn crate::volumes::Source>,
    buf: Vec<u8>,
}

unsafe extern "C" fn feed_read(_: *mut Archive, data: *mut c_void, out: *mut *const c_void) -> isize {
    let f = unsafe { &mut *(data as *mut Feed) };
    match f.src.read(&mut f.buf) {
        Ok(n) => {
            unsafe { *out = f.buf.as_ptr() as *const c_void };
            n as isize
        }
        Err(_) => -1,
    }
}

unsafe extern "C" fn feed_seek(_: *mut Archive, data: *mut c_void, off: i64, whence: c_int) -> i64 {
    use std::io::{Seek, SeekFrom};
    let f = unsafe { &mut *(data as *mut Feed) };
    let to = match whence {
        0 => SeekFrom::Start(off.max(0) as u64),
        1 => SeekFrom::Current(off),
        _ => SeekFrom::End(off),
    };
    f.src.seek(to).map_or(-30, |p| p as i64) // ARCHIVE_FATAL
}

const ARCHIVE_EOF: c_int = 1;
const ARCHIVE_WARN: c_int = -20;
const ARCHIVE_FAILED: c_int = -25;
const AE_IFMT: c_uint = 0o170000;
const AE_IFREG: c_uint = 0o100000;
const AE_IFDIR: c_uint = 0o040000;
const AE_IFLNK: c_uint = 0o120000;

/// Formats libarchive can't write.
const READ_ONLY_SUFFIXES: &[&str] =
    &[".rar", ".cbr", ".iso", ".deb", ".apk", ".cab", ".lha", ".lzh", ".a", ".ar", ".xpi", ".msi", ".exe"];

pub fn writable_name(name: &str) -> bool {
    // Split volumes are as writable as the archive they make up; RAR never.
    if let Some(v) = crate::volumes::parse(name) {
        return v.kind == crate::volumes::Kind::Split && writable_name(&v.archive);
    }
    let l = name.to_lowercase();
    crate::vfs::is_archive_name(&l) && !READ_ONLY_SUFFIXES.iter().any(|s| l.ends_with(s))
}

#[derive(Clone, Default, Debug)]
pub struct Plan {
    /// Inner paths to drop, with their subtrees.
    pub remove: Vec<String>,
    /// Prefix renames (from, to), applied to paths and their subtrees.
    pub rename: Vec<(String, String)>,
    /// Disk files/trees to add: (disk path, inner path of its root).
    /// Existing entries at the same paths are replaced.
    pub add: Vec<(PathBuf, String)>,
    pub mkdir: Vec<String>,
}

fn under(path: &str, root: &str) -> bool {
    path == root || (path.len() > root.len() && path.starts_with(root) && path.as_bytes()[root.len()] == b'/')
}

fn cstr(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|_| io::Error::other("NUL in path"))
}

fn err(a: *mut Archive, what: &str) -> io::Error {
    let msg = unsafe {
        let p = archive_error_string(a);
        if p.is_null() { "unknown error".into() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    };
    io::Error::other(format!("{what}: {msg}"))
}

/// Owns reader/writer handles so every early return frees them.
struct Handles {
    r: *mut Archive,
    w: *mut Archive,
    w_open: bool,
}

/// A fresh entry per added item: fields like the symlink target would
/// otherwise leak from one item into the next.
struct OwnedEntry(*mut ArchiveEntry);

impl OwnedEntry {
    fn new() -> io::Result<Self> {
        Ok(Self(ptr_or(unsafe { archive_entry_new() })?))
    }
}

impl Drop for OwnedEntry {
    fn drop(&mut self) {
        unsafe { archive_entry_free(self.0) }
    }
}

impl Drop for Handles {
    fn drop(&mut self) {
        unsafe {
            if !self.w.is_null() {
                if self.w_open {
                    archive_write_close(self.w);
                }
                archive_write_free(self.w);
            }
            if !self.r.is_null() {
                archive_read_free(self.r);
            }
        }
    }
}

/// Walk disk trees for `plan.add` into (disk path, inner path) pairs.
fn expand_adds(plan: &Plan) -> Vec<(PathBuf, String)> {
    fn walk(p: &Path, inner: String, out: &mut Vec<(PathBuf, String)>) {
        let is_dir = p.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false);
        out.push((p.to_path_buf(), inner.clone()));
        if is_dir {
            if let Ok(rd) = std::fs::read_dir(p) {
                let mut kids: Vec<_> = rd.flatten().collect();
                kids.sort_by_key(|e| e.file_name());
                for k in kids {
                    walk(&k.path(), format!("{inner}/{}", k.file_name().to_string_lossy()), out);
                }
            }
        }
    }
    let mut out = Vec::new();
    for (p, inner) in &plan.add {
        walk(p, normalize_inner(inner), &mut out);
    }
    out
}

/// Total uncompressed bytes the rewrite will push through (for progress).
pub fn estimate(archive: &Path, plan: &Plan) -> u64 {
    let existing = crate::vfs::archive_index(archive)
        .map(|idx| idx.dirs.values().flatten().filter(|e| !e.is_dir).map(|e| e.size).sum())
        .unwrap_or(0u64);
    let added: u64 = expand_adds(plan)
        .iter()
        .filter_map(|(p, _)| p.symlink_metadata().ok().filter(|m| m.is_file()).map(|m| m.len()))
        .sum();
    existing + added
}

/// Apply `plan` to `archive`. `progress(bytes)` returning false cancels.
pub fn rewrite(archive: &Path, plan: &Plan, mut progress: impl FnMut(u64) -> bool) -> io::Result<()> {
    let file_name = archive.file_name().unwrap_or_default().to_string_lossy().into_owned();
    if !writable_name(&file_name) {
        return Err(io::Error::other(crate::i18n::tr("This archive format is read-only")));
    }
    // Split volumes: read joined, write whole, then cut again.
    let split = crate::volumes::set_of(archive).filter(|s| s.kind == crate::volumes::Kind::Split);
    let name = crate::volumes::archive_name(archive);
    if !writable_name(&name) {
        return Err(io::Error::other(crate::i18n::tr("This archive format is read-only")));
    }
    let remove: Vec<String> = plan.remove.iter().map(|s| normalize_inner(s)).collect();
    let rename: Vec<(String, String)> =
        plan.rename.iter().map(|(a, b)| (normalize_inner(a), normalize_inner(b))).collect();
    let adds = expand_adds(plan);
    let replaced: HashSet<&str> = adds.iter().map(|(_, i)| i.as_str()).collect();
    let mkdirs: Vec<String> = plan.mkdir.iter().map(|s| normalize_inner(s)).collect();

    let tmp = archive.with_file_name(format!(".{file_name}.polka-tmp"));
    let tmp_c = CString::new(tmp.as_os_str().as_bytes()).map_err(|_| io::Error::other("bad path"))?;
    let src_c = CString::new(archive.as_os_str().as_bytes()).map_err(|_| io::Error::other("bad path"))?;

    let result = (|| -> io::Result<()> {
        // Declared before the handles so it outlives the reader.
        let mut feed = match &split {
            Some(s) => Some(Feed { src: Box::new(crate::volumes::Joined::open(&s.files)?), buf: vec![0; 1 << 16] }),
            None => None,
        };
        let mut h = Handles { r: ptr_or(unsafe { archive_read_new() })?, w: std::ptr::null_mut(), w_open: false };
        unsafe {
            archive_read_support_filter_all(h.r);
            archive_read_support_format_all(h.r);
            let rc = match feed.as_mut() {
                Some(f) => {
                    archive_read_set_callback_data(h.r, f as *mut Feed as *mut c_void);
                    archive_read_set_read_callback(h.r, feed_read);
                    archive_read_set_seek_callback(h.r, feed_seek);
                    archive_read_open1(h.r)
                }
                None => archive_read_open_filename(h.r, src_c.as_ptr(), 1 << 16),
            };
            if rc != 0 {
                return Err(err(h.r, "open"));
            }
        }
        let mut done = 0u64;
        let mut buf = vec![0u8; 1 << 16];
        let mut seen_dirs: HashSet<String> = HashSet::new();

        loop {
            let mut e: *mut ArchiveEntry = std::ptr::null_mut();
            let rc = unsafe { archive_read_next_header(h.r, &mut e) };
            if rc == ARCHIVE_EOF {
                break;
            }
            if rc < ARCHIVE_WARN {
                return Err(err(h.r, "read"));
            }
            if h.w.is_null() {
                // The format is only known once the first header is read.
                h.w = open_writer(h.r, &name, &tmp_c)?;
                h.w_open = true;
            }
            let raw = unsafe {
                let p = archive_entry_pathname_utf8(e);
                let p = if p.is_null() { archive_entry_pathname(e) } else { p };
                if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
            };
            let path = normalize_inner(&raw);
            let ftype = unsafe { archive_entry_filetype(e) } & AE_IFMT;
            if path.is_empty()
                || remove.iter().any(|r| under(&path, r))
                || replaced.contains(path.as_str())
                || (ftype == AE_IFDIR && seen_dirs.contains(&path))
            {
                continue; // data is skipped by the next read_next_header
            }
            if unsafe { archive_entry_is_encrypted(e) } != 0 {
                return Err(io::Error::other(crate::i18n::tr("Encrypted archives can't be modified")));
            }
            let renamed = rename_path(&path, &rename);
            unsafe {
                if renamed != path || raw != path {
                    let mut p = renamed.clone();
                    if ftype == AE_IFDIR {
                        p.push('/');
                    }
                    archive_entry_update_pathname_utf8(e, cstr(&p)?.as_ptr());
                }
                let hl = archive_entry_hardlink(e);
                if !hl.is_null() {
                    let target = normalize_inner(&CStr::from_ptr(hl).to_string_lossy());
                    if remove.iter().any(|r| under(&target, r)) {
                        continue; // its data lived in a removed entry
                    }
                    let nt = rename_path(&target, &rename);
                    if nt != target {
                        archive_entry_update_hardlink_utf8(e, cstr(&nt)?.as_ptr());
                    }
                }
            }
            if ftype == AE_IFDIR {
                seen_dirs.insert(renamed.clone());
            }

            // Entries without a size in the header (zip data descriptors)
            // must be buffered: tar & co. need the size up front.
            let sized = unsafe { archive_entry_size_is_set(e) } != 0;
            if ftype == AE_IFREG && !sized {
                let mut data = Vec::new();
                loop {
                    let n = unsafe { archive_read_data(h.r, buf.as_mut_ptr().cast(), buf.len()) };
                    if n < 0 {
                        return Err(err(h.r, "read data"));
                    }
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n as usize]);
                }
                unsafe { archive_entry_set_size(e, data.len() as i64) };
                write_header(h.w, e)?;
                write_all(h.w, &data)?;
                done += data.len() as u64;
            } else {
                write_header(h.w, e)?;
                if ftype == AE_IFREG && unsafe { archive_entry_size(e) } > 0 {
                    loop {
                        let n = unsafe { archive_read_data(h.r, buf.as_mut_ptr().cast(), buf.len()) };
                        if n < 0 {
                            return Err(err(h.r, "read data"));
                        }
                        if n == 0 {
                            break;
                        }
                        write_all(h.w, &buf[..n as usize])?;
                        done += n as u64;
                        if !progress(done) {
                            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                        }
                    }
                }
            }
            unsafe { archive_write_finish_entry(h.w) };
        }

        if h.w.is_null() {
            // Empty source archive: pick the format from the file name.
            let w = ptr_or(unsafe { archive_write_new() })?;
            h.w = w;
            unsafe {
                if archive_write_set_format_filter_by_ext(w, cstr(&name)?.as_ptr()) != 0 {
                    return Err(err(w, "format"));
                }
                if archive_write_open_filename(w, tmp_c.as_ptr()) != 0 {
                    return Err(err(w, "create"));
                }
            }
            h.w_open = true;
        }

        for d in &mkdirs {
            if seen_dirs.contains(d) {
                continue;
            }
            let e = OwnedEntry::new()?;
            new_entry(e.0, &format!("{d}/"), AE_IFDIR, 0o755, 0, now())?;
            write_header(h.w, e.0)?;
            unsafe { archive_write_finish_entry(h.w) };
        }
        for (disk, inner) in &adds {
            let m = disk.symlink_metadata()?;
            let perm = m.permissions().mode() & 0o7777;
            let e = OwnedEntry::new()?;
            if m.is_dir() {
                new_entry(e.0, &format!("{inner}/"), AE_IFDIR, perm, 0, m.mtime())?;
                write_header(h.w, e.0)?;
            } else if m.file_type().is_symlink() {
                new_entry(e.0, inner, AE_IFLNK, 0o777, 0, m.mtime())?;
                let target = std::fs::read_link(disk)?;
                unsafe { archive_entry_update_symlink_utf8(e.0, cstr(&target.to_string_lossy())?.as_ptr()) };
                write_header(h.w, e.0)?;
            } else if m.is_file() {
                new_entry(e.0, inner, AE_IFREG, perm, m.len() as i64, m.mtime())?;
                write_header(h.w, e.0)?;
                let mut f = File::open(disk)?;
                loop {
                    let n = f.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    write_all(h.w, &buf[..n])?;
                    done += n as u64;
                    if !progress(done) {
                        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                    }
                }
            } else {
                continue; // sockets, fifos, devices
            }
            unsafe { archive_write_finish_entry(h.w) };
        }
        unsafe {
            if archive_write_close(h.w) != 0 {
                return Err(err(h.w, "finish"));
            }
        }
        h.w_open = false;
        Ok(())
    })();

    match result {
        Ok(()) => {
            if let Some(set) = &split {
                // Same volume size as before (the first volume is a full one).
                let size = std::fs::metadata(&set.files[0])?.len().max(1);
                let r = crate::volumes::resplit(&tmp, set, size);
                if r.is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
                crate::vfs::invalidate(archive);
                return r;
            }
            if let Ok(m) = std::fs::metadata(archive) {
                let _ = std::fs::set_permissions(&tmp, m.permissions());
            }
            std::fs::rename(&tmp, archive)?;
            crate::vfs::invalidate(archive);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn ptr_or<T>(p: *mut T) -> io::Result<*mut T> {
    if p.is_null() { Err(io::Error::other("libarchive: out of memory")) } else { Ok(p) }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn rename_path(path: &str, rename: &[(String, String)]) -> String {
    for (from, to) in rename {
        if under(path, from) {
            return format!("{to}{}", &path[from.len()..]);
        }
    }
    path.to_string()
}

fn open_writer(r: *mut Archive, name: &str, tmp: &CStr) -> io::Result<*mut Archive> {
    unsafe {
        let w = ptr_or(archive_write_new())?;
        let fail = |what| {
            let e = err(w, what);
            archive_write_free(w);
            e
        };
        if archive_write_set_format(w, archive_format(r)) != 0 {
            // Some read-side format codes have no writer; fall back to the name.
            if archive_write_set_format_filter_by_ext(w, cstr(name)?.as_ptr()) != 0 {
                return Err(fail("format"));
            }
        } else {
            // Filters are listed outermost first; the last one is "none".
            let n = archive_filter_count(r);
            for i in (0..n).rev() {
                let code = archive_filter_code(r, i);
                if code != 0 && archive_write_add_filter(w, code) != 0 {
                    return Err(fail("filter"));
                }
            }
        }
        if archive_write_open_filename(w, tmp.as_ptr()) != 0 {
            return Err(fail("create"));
        }
        Ok(w)
    }
}

fn new_entry(e: *mut ArchiveEntry, path: &str, ftype: c_uint, perm: u32, size: i64, mtime: i64) -> io::Result<()> {
    unsafe {
        archive_entry_update_pathname_utf8(e, cstr(path)?.as_ptr());
        archive_entry_set_filetype(e, ftype);
        archive_entry_set_perm(e, perm);
        archive_entry_set_size(e, size);
        archive_entry_set_mtime(e, mtime, 0);
    }
    Ok(())
}

fn write_header(w: *mut Archive, e: *mut ArchiveEntry) -> io::Result<()> {
    let rc = unsafe { archive_write_header(w, e) };
    if rc <= ARCHIVE_FAILED { Err(err(w, "write")) } else { Ok(()) }
}

fn write_all(w: *mut Archive, data: &[u8]) -> io::Result<()> {
    let mut off = 0;
    while off < data.len() {
        let n = unsafe { archive_write_data(w, data[off..].as_ptr().cast(), data.len() - off) };
        if n <= 0 {
            return Err(err(w, "write data"));
        }
        off += n as usize;
    }
    Ok(())
}
