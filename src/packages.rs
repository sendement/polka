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
        assert_eq!(quote("it's"), "'it'\\''s'");
        let out = std::process::Command::new("sh").arg("-c").arg(format!("printf %s {}", quote("a 'b' $c"))).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "a 'b' $c");
    }
}
