//! The trash (freedesktop.org layout): how full it is, and emptying it —
//! the home trash and the ones on other mounted disks.

use std::path::PathBuf;

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
