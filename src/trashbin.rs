//! The trash (freedesktop.org layout): how full it is, and emptying it —
//! the home trash and the ones on other mounted disks.

use std::path::{Path, PathBuf};

/// Trash folders (each with files/ and info/) that exist.
pub fn roots() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = dirs::data_dir().map(|d| d.join("Trash")).into_iter().collect();
    // Tests only ever touch their own home trash.
    if !cfg!(test) {
        let uid = unsafe { libc::getuid() };
        for m in crate::usage::mounts() {
            v.push(m.path.join(format!(".Trash-{uid}")));
            v.push(m.path.join(".Trash").join(uid.to_string()));
        }
        for d in crate::drives::list() {
            if let Some(m) = d.mount {
                v.push(m.join(format!(".Trash-{uid}")));
            }
        }
    }
    v.sort();
    v.dedup();
    v.retain(|r| r.join("files").is_dir());
    v
}

/// Items in all the trash folders.
pub fn count() -> usize {
    roots().iter().map(|r| std::fs::read_dir(r.join("files")).map_or(0, |d| d.count())).sum()
}

/// Delete everything in the trash for good: (items removed, failures).
pub fn empty() -> (usize, usize) {
    let (mut done, mut failed) = (0, 0);
    for r in roots() {
        for sub in ["files", "info"] {
            let Ok(rd) = std::fs::read_dir(r.join(sub)) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let res = if e.file_type().is_ok_and(|t| t.is_dir()) { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                match (res, sub) {
                    (Ok(()), "files") => done += 1,
                    (Err(_), "files") => failed += 1,
                    _ => {}
                }
            }
        }
        let _ = std::fs::remove_file(r.join("directorysizes"));
    }
    (done, failed)
}

/// Whether `dir` is the files/ folder of a trash, where things can be put
/// back.
pub fn is_files_dir(dir: &Path) -> bool {
    dir.file_name().is_some_and(|n| n == "files") && dir.parent().is_some_and(|r| r.join("info").is_dir())
}

/// The .trashinfo of an item in a trash's files/ folder.
pub fn info_of(item: &Path) -> Option<PathBuf> {
    let files = item.parent().filter(|d| is_files_dir(d))?;
    let mut name = item.file_name()?.to_os_string();
    name.push(".trashinfo");
    Some(files.parent()?.join("info").join(name))
}

/// Where a trashed item came from. A disk's own trash keeps paths relative
/// to the disk (.Trash-1000 or .Trash/1000 at its top).
pub fn origin(item: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(info_of(item)?).ok()?;
    let raw = text.lines().map(str::trim).find_map(|l| l.strip_prefix("Path="))?;
    let p = crate::app::percent_decoded(raw);
    if p.is_absolute() {
        return Some(p);
    }
    let root = item.parent()?.parent()?;
    let top = if root.parent().is_some_and(|d| d.file_name().is_some_and(|n| n == ".Trash")) { root.parent()?.parent()? } else { root.parent()? };
    Some(top.join(p))
}

/// The item in a trash that came from `original` (the last trashed, when
/// there are several).
pub fn find(original: &Path) -> Option<PathBuf> {
    let when = |item: &Path| -> String {
        info_of(item)
            .and_then(|i| std::fs::read_to_string(i).ok())
            .and_then(|t| t.lines().map(str::trim).find_map(|l| l.strip_prefix("DeletionDate=")).map(str::to_string))
            .unwrap_or_default()
    };
    // Its name in the trash is its own, or that with a number added.
    let name = original.file_name()?.to_string_lossy().into_owned();
    roots()
        .into_iter()
        .filter_map(|r| std::fs::read_dir(r.join("files")).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()))
        .filter(|item| item.file_name().is_some_and(|n| n.to_string_lossy().starts_with(name.split('.').next().unwrap_or(&name))))
        .filter(|item| origin(item).as_deref() == Some(original))
        .max_by_key(|item| when(item))
}

#[cfg(test)]
mod tests {
    #[test]
    fn origins() {
        let t = std::env::temp_dir().join(format!("polka-trash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        for (root, info) in [
            (t.join("home/Trash"), "Path=/home/me/My%20File.txt"),
            (t.join("disk/.Trash-1000"), "Path=docs/a.txt"),
            (t.join("disk2/.Trash/1000"), "Path=b.txt"),
        ] {
            std::fs::create_dir_all(root.join("files")).unwrap();
            std::fs::create_dir_all(root.join("info")).unwrap();
            std::fs::write(root.join("files/x"), "").unwrap();
            std::fs::write(root.join("info/x.trashinfo"), format!("[Trash Info]\n{info}\nDeletionDate=2026-10-19T10:00:00\n")).unwrap();
        }
        assert_eq!(super::origin(&t.join("home/Trash/files/x")), Some("/home/me/My File.txt".into()));
        assert_eq!(super::origin(&t.join("disk/.Trash-1000/files/x")), Some(t.join("disk/docs/a.txt")));
        assert_eq!(super::origin(&t.join("disk2/.Trash/1000/files/x")), Some(t.join("disk2/b.txt")));
        assert_eq!(super::origin(&t.join("elsewhere/x")), None);
        let _ = std::fs::remove_dir_all(&t);
    }
}
