//! Tags on files and folders. They live in the `user.xdg.tags` extended
//! attribute (comma separated), as KDE's Dolphin and Baloo keep them, so
//! both see the same tags; where the filesystem has no such attributes
//! (FAT, some network ones) Polka keeps them in a list of its own.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const ATTR: &[u8] = b"user.xdg.tags\0";

/// Polka's own list: tags of paths that can't hold them, and every tag
/// given so far (offered when tagging).
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Db {
    paths: BTreeMap<PathBuf, Vec<String>>,
    known: BTreeSet<String>,
}

static DB: Mutex<Option<Db>> = Mutex::new(None);

fn db_path() -> Option<PathBuf> {
    // Tests never touch the real list.
    if cfg!(test) {
        return Some(std::env::temp_dir().join(format!("polka-test-tags-{}.json", std::process::id())));
    }
    Some(dirs::data_dir()?.join("polka/tags.json"))
}

fn with_db<R>(f: impl FnOnce(&mut Db) -> R) -> R {
    let mut g = DB.lock().unwrap_or_else(|e| e.into_inner());
    let db = g.get_or_insert_with(|| db_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default());
    f(db)
}

fn save(db: &Db) {
    let Some(p) = db_path() else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(s) = serde_json::to_string_pretty(db) {
        let _ = std::fs::write(p, s);
    }
}

/// "a, b,c" as tags.
pub fn parse(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    v.dedup();
    v
}

/// What the attribute says: Some(tags) (maybe none), or None when the
/// filesystem doesn't keep such attributes.
fn read_attr(p: &Path) -> Option<Vec<String>> {
    let c = CString::new(p.as_os_str().as_bytes()).ok()?;
    let mut buf = vec![0u8; 1024];
    loop {
        let n = unsafe { libc::getxattr(c.as_ptr(), ATTR.as_ptr().cast(), buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Some(parse(&String::from_utf8_lossy(&buf[..n as usize])));
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ERANGE) if buf.len() < 1 << 16 => buf.resize(buf.len() * 4, 0),
            Some(libc::ENODATA) => return Some(Vec::new()),
            _ => return None,
        }
    }
}

/// A file's tags. `probe` false skips the attribute (a folder found not to
/// keep them), leaving Polka's own list.
pub fn read(p: &Path, probe: bool) -> (Vec<String>, bool) {
    let attr = if probe { read_attr(p) } else { None };
    let kept = attr.is_some();
    let mut tags = attr.unwrap_or_default();
    let own = with_db(|db| if db.paths.is_empty() { None } else { db.paths.get(p).cloned() });
    for t in own.into_iter().flatten() {
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
    (tags, kept)
}

/// Give a file these tags (none: take them all off).
pub fn write(p: &Path, tags: &[String]) -> std::io::Result<()> {
    let c = CString::new(p.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    let value = tags.join(",");
    let r = unsafe {
        if tags.is_empty() {
            libc::removexattr(c.as_ptr(), ATTR.as_ptr().cast())
        } else {
            libc::setxattr(c.as_ptr(), ATTR.as_ptr().cast(), value.as_ptr().cast(), value.len(), 0)
        }
    };
    let err = (r != 0).then(std::io::Error::last_os_error);
    let in_attr = match &err {
        None => true,
        // Nothing to take off is fine.
        Some(e) if tags.is_empty() && e.raw_os_error() == Some(libc::ENODATA) => true,
        // No such attributes here: Polka's own list.
        Some(e) if matches!(e.raw_os_error(), Some(libc::ENOTSUP) | Some(libc::EPERM)) => false,
        Some(e) => return Err(std::io::Error::new(e.kind(), e.to_string())),
    };
    with_db(|db| {
        let changed = if in_attr || tags.is_empty() {
            db.paths.remove(p).is_some()
        } else {
            db.paths.insert(p.to_path_buf(), tags.to_vec());
            true
        };
        let before = db.known.len();
        db.known.extend(tags.iter().cloned());
        if changed || db.known.len() != before {
            save(db);
        }
    });
    Ok(())
}

/// Every tag given so far, sorted.
pub fn known() -> Vec<String> {
    with_db(|db| db.known.iter().cloned().collect())
}

/// A tag's colour: the same for the same name, everywhere.
pub fn color(tag: &str, dark: bool) -> egui::Color32 {
    let h = tag.to_lowercase().bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    // Twelve hues far enough apart to tell, picked by the name.
    let hue = (h.wrapping_mul(2654435761) >> 16) % 12;
    let (s, v) = if dark { (0.6, 0.9) } else { (0.75, 0.7) };
    egui::ecolor::HsvaGamma { h: hue as f32 / 12.0, s, v, a: 1.0 }.into()
}

/// A search query's #tags and the rest (the name to look for).
pub fn split_query(query: &str) -> (Vec<String>, String) {
    let mut tags = Vec::new();
    let mut words = Vec::new();
    for w in query.split_whitespace() {
        match w.strip_prefix('#') {
            Some(t) if !t.is_empty() => tags.push(t.to_lowercase()),
            Some(_) => {}
            None => words.push(w),
        }
    }
    (tags, words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_on_files() {
        assert_eq!(parse(" work, photos ,,x"), ["work", "photos", "x"]);
        assert_eq!(split_query("#Work report #2026"), (vec!["work".to_string(), "2026".into()], "report".into()));
        assert_eq!(split_query("plain name"), (vec![], "plain name".into()));
        // A real file (tmpfs and ext4 keep user attributes).
        let dir = std::env::temp_dir().join(format!("polka-tags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        if read_attr(&f).is_some() {
            write(&f, &["work".into(), "2026".into()]).unwrap();
            assert_eq!(read(&f, true).0, ["work", "2026"]);
            write(&f, &[]).unwrap();
            assert!(read(&f, true).0.is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
