//! Packages and scripts: a package file is offered for installing with the
//! system's own tool (only those there are: pacman, flatpak, apt / dpkg,
//! dnf / zypper / rpm, snap), a shell script for running; both in a terminal
//! window, where the tool's questions and sudo's password are answered.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pacman,
    Flatpak,
    Deb,
    Rpm,
    Snap,
    Script,
}

/// What kind of package (or script) a file name is.
pub fn kind(name: &str) -> Option<Kind> {
    let n = name.to_lowercase();
    let k = if [".pkg.tar.zst", ".pkg.tar.xz", ".pkg.tar.gz", ".pkg.tar"].iter().any(|s| n.ends_with(s)) {
        Kind::Pacman
    } else if n.ends_with(".flatpak") || n.ends_with(".flatpakref") {
        Kind::Flatpak
    } else if n.ends_with(".deb") {
        Kind::Deb
    } else if n.ends_with(".rpm") {
        Kind::Rpm
    } else if n.ends_with(".snap") {
        Kind::Snap
    } else if n.ends_with(".sh") {
        Kind::Script
    } else {
        return None;
    };
    Some(k)
}

/// Quoted for sh.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The command for it on this system, and the tool's name (None: nothing
/// here installs that kind).
pub fn command(k: Kind, path: &Path) -> Option<(String, &'static str)> {
    let p = quote(&crate::sandbox::host_path(path).to_string_lossy());
    let have = |t: &'static str| !crate::terminal::installed(&[t]).is_empty();
    let sudo = have("sudo");
    Some(match k {
        Kind::Pacman if have("pacman") && sudo => (format!("sudo pacman -U {p}"), "pacman"),
        Kind::Flatpak if have("flatpak") => (format!("flatpak install {p}"), "flatpak"),
        Kind::Deb if have("apt") && sudo => (format!("sudo apt install {p}"), "apt"),
        Kind::Deb if have("dpkg") && sudo => (format!("sudo dpkg -i {p}"), "dpkg"),
        Kind::Rpm if have("dnf") && sudo => (format!("sudo dnf install {p}"), "dnf"),
        Kind::Rpm if have("zypper") && sudo => (format!("sudo zypper install {p}"), "zypper"),
        Kind::Rpm if have("rpm") && sudo => (format!("sudo rpm -i {p}"), "rpm"),
        Kind::Snap if have("snap") && sudo => (format!("sudo snap install --dangerous {p}"), "snap"),
        // An executable script runs as itself (its own interpreter), else sh.
        Kind::Script => {
            use std::os::unix::fs::PermissionsExt;
            let exec = std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
            (if exec { p } else { format!("sh {p}") }, "sh")
        }
        _ => return None,
    })
}

/// The window's script: the command shown, run, and then, if it went
/// well, a short pause before the window closes; if not, it waits.
pub fn wrapper(cmd: &str) -> String {
    use crate::i18n::tr;
    let ok = quote(tr("Done"));
    let failed = quote(tr("Failed — Enter closes"));
    format!(
        "printf '\\033[1m$ %s\\033[0m\\n\\n' {shown}; {cmd}; s=$?; if [ $s -eq 0 ]; then printf '\\n\\033[32m✔ %s\\033[0m\\n' {ok}; sleep 1; else printf '\\n\\033[31m✘ %s (%s)\\033[0m\\n' {failed} $s; read _; fi; exit $s",
        shown = quote(cmd)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_quoting() {
        assert_eq!(kind("polka-0.19.3-1-x86_64.pkg.tar.zst"), Some(Kind::Pacman));
        assert_eq!(kind("app.flatpakref"), Some(Kind::Flatpak));
        assert_eq!(kind("x.DEB"), Some(Kind::Deb));
        assert_eq!(kind("setup.sh"), Some(Kind::Script));
        assert_eq!(kind("notes.txt"), None);
        // Checked by what's inside.
        let dir = std::env::temp_dir().join(format!("polka-pkg-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("p")).unwrap();
        std::fs::write(dir.join("p/.PKGINFO"), "pkgname = x\n").unwrap();
        std::fs::write(dir.join("p/x"), "x").unwrap();
        let ok = std::process::Command::new("tar").arg("czf").arg(dir.join("x-1-1-any.pkg.tar.gz")).arg("-C").arg(dir.join("p")).arg(".PKGINFO").arg("x").status().unwrap();
        assert!(ok.success());
        assert!(looks_right(&dir.join("x-1-1-any.pkg.tar.gz"), Kind::Pacman));
        let ok = std::process::Command::new("tar").arg("czf").arg(dir.join("fake-1-1-any.pkg.tar.gz")).arg("-C").arg(dir.join("p")).arg("x").status().unwrap();
        assert!(ok.success());
        assert!(!looks_right(&dir.join("fake-1-1-any.pkg.tar.gz"), Kind::Pacman), "a plain tar isn't a package");
        std::fs::write(dir.join("a.deb"), b"!<arch>\ndebian-binary   1 0 0 100644 4 `\n2.0\n").unwrap();
        assert!(looks_right(&dir.join("a.deb"), Kind::Deb));
        std::fs::write(dir.join("b.deb"), b"PK\x03\x04").unwrap();
        assert!(!looks_right(&dir.join("b.deb"), Kind::Deb));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(quote("it's"), "'it'\\''s'");
        let out = std::process::Command::new("sh").arg("-c").arg(format!("printf %s {}", quote("a 'b' $c"))).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "a 'b' $c");
    }
}

// ---------------------------------------------------------------- checking

/// Does the file look inside like what its name says? A pacman package
/// holds .PKGINFO, a deb is an ar archive starting with debian-binary, an
/// rpm has its lead, a snap is squashfs, a flatpakref a [Flatpak Ref] file.
pub fn looks_right(path: &Path, k: Kind) -> bool {
    use std::io::Read;
    let mut head = vec![0u8; 4096];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    let head = &head[..n];
    match k {
        Kind::Pacman => {
            let Ok(file) = std::fs::File::open(path) else { return false };
            let names: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
            let sink = names.clone();
            let Ok(mut it) = compress_tools::ArchiveIteratorBuilder::new(file)
                .filter(move |name, _| {
                    sink.lock().unwrap().push(name.to_string());
                    false
                })
                .build()
            else {
                return false;
            };
            // .PKGINFO comes among the first few entries.
            for _ in 0..64 {
                if it.next().is_none() || names.lock().unwrap().len() >= 6 {
                    break;
                }
            }
            drop(it);
            names.lock().unwrap().iter().any(|n| n.trim_start_matches("./") == ".PKGINFO")
        }
        Kind::Deb => head.starts_with(b"!<arch>\n") && head.windows(13).any(|w| w == b"debian-binary"),
        Kind::Rpm => head.starts_with(&[0xed, 0xab, 0xee, 0xdb]),
        Kind::Snap => head.starts_with(b"hsqs"),
        Kind::Flatpak if path.extension().is_some_and(|e| e == "flatpakref") => String::from_utf8_lossy(head).contains("[Flatpak Ref]"),
        Kind::Flatpak | Kind::Script => true,
    }
}

#[derive(Default)]
struct Checks {
    known: std::collections::HashMap<std::path::PathBuf, (i64, bool)>,
    queued: std::collections::VecDeque<(std::path::PathBuf, i64, Kind)>,
    wake: Option<egui::Context>,
}

fn checks() -> &'static (std::sync::Mutex<Checks>, std::sync::Condvar) {
    static S: std::sync::OnceLock<(std::sync::Mutex<Checks>, std::sync::Condvar)> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        std::thread::Builder::new().name("polka-packages".into()).spawn(check_worker).ok();
        Default::default()
    })
}

fn check_worker() {
    loop {
        let (path, mtime, k) = {
            let (m, cv) = checks();
            let mut st = m.lock().unwrap();
            loop {
                if let Some(job) = st.queued.pop_front() {
                    break job;
                }
                st = cv.wait(st).unwrap();
            }
        };
        let ok = looks_right(&path, k);
        let (m, _) = checks();
        let mut st = m.lock().unwrap();
        st.known.insert(path, (mtime, ok));
        if let Some(c) = &st.wake {
            c.request_repaint();
        }
    }
}

/// Repaint `ctx` when answers come in.
pub fn set_waker(ctx: egui::Context) {
    checks().0.lock().unwrap().wake = Some(ctx);
}

/// The package this file is, once checked (None: not one, or not known yet
/// — then it's queued).
pub fn package(path: &Path, name: &str, mtime: i64) -> Option<Kind> {
    let k = kind(name).filter(|k| *k != Kind::Script)?;
    let (m, cv) = checks();
    let mut st = m.lock().unwrap();
    match st.known.get(path) {
        Some(&(t, ok)) if t == mtime => ok.then_some(k),
        _ => {
            if !st.queued.iter().any(|(p, _, _)| p == path) {
                st.queued.push_back((path.to_path_buf(), mtime, k));
                cv.notify_one();
            }
            None
        }
    }
}

/// The badge for a package: its format's name and colour.
pub fn badge(k: Kind) -> (&'static str, [u8; 3]) {
    match k {
        Kind::Pacman => ("pacman", [0x17, 0x93, 0xd1]),
        Kind::Flatpak => ("flatpak", [0x4a, 0x86, 0xcf]),
        Kind::Deb => ("deb", [0xd7, 0x0a, 0x53]),
        Kind::Rpm => ("rpm", [0x3c, 0x6e, 0xb4]),
        Kind::Snap => ("snap", [0xe9, 0x54, 0x20]),
        Kind::Script => ("sh", [0x4e, 0x9a, 0x06]),
    }
}
