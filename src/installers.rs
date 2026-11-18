//! Windows installers unpacked without Wine, by Linux programs: Inno Setup
//! by innoextract; NSIS, MSI, cabinets and self-extracting 7z / RAR / zip
//! by 7z (MSI by msiextract when it's there: real file names); InstallShield
//! cabinets by unshield. They stream, so a set of any size goes through.
//!
//! Repacks (FitGirl, DODI…) are Inno Setup outside only: their data is in
//! FreeArc archives squeezed by compressors that exist for Windows alone
//! (lolz, xtool, Oodle…). Those go through Wine, but only the unpacker the
//! repack ships, one archive at a time (see repack.rs), not the installer.

use crate::i18n::tr;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;

/// What kind of installer, and the program that unpacks it.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Inno,
    /// What 7z called it (Nsis, 7z, Rar, zip, Cab, Compound…).
    SevenZip(String),
    Msi,
    InstallShield,
    /// A repack: its FreeArc archives, in order.
    Repack(Vec<PathBuf>),
}

impl Kind {
    pub fn tool(&self) -> &'static str {
        match self {
            Kind::Inno => "innoextract",
            Kind::SevenZip(_) => "7z",
            Kind::Msi => "msiextract",
            Kind::InstallShield => "unshield",
            Kind::Repack(_) => "wine",
        }
    }

    /// The package that has the program (as on Arch).
    pub fn package(&self) -> &'static str {
        match self {
            Kind::Inno => "innoextract",
            Kind::SevenZip(_) => "7zip",
            Kind::Msi => "msitools",
            Kind::InstallShield => "unshield",
            Kind::Repack(_) => "wine",
        }
    }
}

/// Worth offering "Unpack installer" for, by its name.
pub fn is_candidate(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".exe", ".msi", ".cab"].iter().any(|e| lower.ends_with(e))
}

/// Does `hay` hold `needle`?
fn holds(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// The start of the file (where installers say what they are).
fn head(path: &Path, n: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(n.min(1 << 20));
    std::fs::File::open(path)?.take(n as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// A repack's data beside the installer: FreeArc archives ("ArC\x01"),
/// by name (fg-01.bin, fg-02.bin…).
pub fn repack_beside(path: &Path) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.path() != path && e.path().is_file())
        .take(400)
        .map(|e| e.path())
        .filter(|p| head(p, 4).is_ok_and(|h| h == b"ArC\x01"))
        .collect();
    v.sort_by(|a, b| crate::app::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    v
}

/// What 7z sees the file as ("Type = …" of the archive itself).
fn seven_zip_type(path: &Path) -> Option<String> {
    let mut c = Command::new("7z");
    c.args(["l", "-slt", "-p", "-bso0", "-bse0", "-bsp0"]).arg("--").arg(path);
    let mut c = crate::sandbox::host(c);
    let out = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    // The archive's own block comes first (before the "----------" of its
    // items); in it the last "Type =" is the innermost (the payload of an
    // exe, not "PE").
    let block = text.split("\n----------").next().unwrap_or(&text);
    block.lines().filter_map(|l| l.strip_prefix("Type = ")).map(|t| t.trim().to_string()).last()
}

/// What `path` is, or why it can't be unpacked.
pub fn detect(path: &Path) -> Result<Kind, String> {
    let h = head(path, 8 << 20).map_err(|e| e.to_string())?;
    if h.starts_with(b"ISc(") {
        return Ok(Kind::InstallShield);
    }
    if holds(&h, b"Inno Setup Setup Data") || holds(&h, b"Inno Setup Messages") {
        let archives = repack_beside(path);
        return Ok(if archives.is_empty() { Kind::Inno } else { Kind::Repack(archives) });
    }
    let msi = h.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    if !crate::convert::has("7z") {
        return if msi && crate::convert::has("msiextract") { Ok(Kind::Msi) } else { Err(needs(&Kind::SevenZip(String::new()))) };
    }
    match seven_zip_type(path) {
        Some(t) if t == "Compound" || msi => Ok(if crate::convert::has("msiextract") { Kind::Msi } else { Kind::SevenZip(t) }),
        // (Only what installers come as: a program's own sections are not.)
        Some(t) if ["Nsis", "7z", "Rar", "Rar5", "zip", "Cab", "Arj", "gzip", "xz", "bzip2"].contains(&t.as_str()) => Ok(Kind::SevenZip(t)),
        _ => Err(tr("Not an installer Polka can unpack").to_string()),
    }
}

/// "Needs innoextract (the innoextract package)".
pub fn needs(k: &Kind) -> String {
    tr("Needs %t (the %p package)").replace("%t", k.tool()).replace("%p", k.package())
}

/// The program's arguments to unpack `path` into `dest`.
fn command(k: &Kind, path: &Path, dest: &Path) -> Command {
    let mut c = Command::new(k.tool());
    match k {
        Kind::Inno => {
            c.args(["-e", "-m", "-p1", "-c0", "--collisions=rename", "-d"]).arg(dest).arg("--").arg(path);
        }
        Kind::SevenZip(_) => {
            c.args(["x", "-y", "-bsp1", "-bso0", "-p"]).arg(format!("-o{}", dest.display())).arg("--").arg(path);
        }
        Kind::Msi => {
            c.arg("-C").arg(dest).arg(path);
        }
        Kind::InstallShield => {
            c.arg("-d").arg(dest).arg("x").arg(path);
        }
        Kind::Repack(_) => {}
    }
    c
}

/// The last "12.3%" in a piece of output.
pub fn percent(s: &str) -> Option<f32> {
    s.split(|c: char| c.is_whitespace() || c == '[' || c == ']').filter_map(|w| w.strip_suffix('%')).filter_map(|n| n.parse::<f32>().ok()).filter(|p| (0.0..=100.0).contains(p)).last()
}

/// Unpack `path` (its kind found out first) into the new folder `dest`,
/// progress in `job` (in the installer's bytes); cancelled, the program is
/// stopped.
pub fn unpack(path: &Path, dest: &Path, job: &crate::ops::Job) -> io::Result<()> {
    let kind = detect(path).map_err(io::Error::other)?;
    if !crate::convert::has(kind.tool()) {
        return Err(io::Error::other(needs(&kind)));
    }
    if let Kind::Repack(archives) = &kind {
        if !crate::convert::has("innoextract") {
            return Err(io::Error::other(needs(&Kind::Inno)));
        }
        return crate::repack::unpack(path, archives, dest, job);
    }
    std::fs::create_dir_all(dest)?;
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0).max(1);
    job.total.store(size, Ordering::Relaxed);
    job.now_on(path);
    let mut c = crate::sandbox::host(command(&kind, path, dest));
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).env("LC_ALL", "C.UTF-8");
    let mut child = c.spawn().map_err(|e| io::Error::other(format!("{}: {e}", kind.tool())))?;
    // Both streams read as they come: the percentages (after \r, \n or
    // backspaces) move the bar, the last lines are kept for an error.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let mut readers = Vec::new();
    for s in [child.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>), child.stderr.take().map(|e| Box::new(e) as Box<dyn Read + Send>)].into_iter().flatten() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            let mut s = s;
            let mut buf = [0u8; 8192];
            let mut part = Vec::new();
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for &b in &buf[..n] {
                    if b == b'\r' || b == b'\n' || b == 8 {
                        if !part.is_empty() {
                            let _ = tx.send(String::from_utf8_lossy(&part).into_owned());
                            part.clear();
                        }
                    } else {
                        part.push(b);
                    }
                }
            }
            if !part.is_empty() {
                let _ = tx.send(String::from_utf8_lossy(&part).into_owned());
            }
        }));
    }
    drop(tx);
    let mut tail: Vec<String> = Vec::new();
    let status = loop {
        while let Ok(line) = rx.try_recv() {
            if let Some(p) = percent(&line) {
                job.done.store((size as f64 * p as f64 / 100.0) as u64, Ordering::Relaxed);
            }
            // (innoextract's " - "file"": what it's on.)
            let t = line.trim().trim_start_matches("\u{1b}[K").trim();
            if let Some(name) = t.strip_prefix("- \"").and_then(|r| r.split('"').next()) {
                *job.current.lock().unwrap() = name.to_string();
            }
            if !t.is_empty() && percent(t).is_none() && !t.starts_with("- \"") {
                tail.push(t.to_string());
                if tail.len() > 8 {
                    tail.remove(0);
                }
            }
        }
        if job.cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        match child.try_wait()? {
            Some(s) => break s,
            None => std::thread::sleep(std::time::Duration::from_millis(80)),
        }
    };
    for r in readers {
        let _ = r.join();
    }
    while let Ok(line) = rx.try_recv() {
        let t = line.trim().to_string();
        if !t.is_empty() && percent(&t).is_none() && !t.starts_with("- \"") {
            tail.push(t);
        }
    }
    // (7z's 1 is a warning: it unpacked.)
    if status.success() || (matches!(kind, Kind::SevenZip(_)) && status.code() == Some(1)) {
        job.done.store(size, Ordering::Relaxed);
        // innoextract puts the program under app/ ({app}): its contents
        // are what's wanted, when that's all there is.
        if kind == Kind::Inno {
            let _ = flatten_app(dest);
        }
        return Ok(());
    }
    let why = tail.iter().rev().find(|l| {
        let l = l.to_lowercase();
        l.contains("error") || l.contains("not a supported") || l.contains("unsupported") || l.contains("can't") || l.contains("cannot")
    });
    let mut msg = why.or(tail.last()).cloned().unwrap_or_else(|| format!("{}: {status}", kind.tool()));
    // innoextract 1.9 stops at Inno Setup 6.0.5; newer ones need its
    // development version.
    if kind == Kind::Inno && msg.to_lowercase().contains("not a supported") {
        msg = format!("{msg} — {}", tr("a newer Inno Setup: innoextract's development version (innoextract-git) unpacks it"));
    }
    Err(io::Error::other(msg))
}

/// `dest/app/*` up into `dest` when app/ is all there is.
pub fn flatten_app(dest: &Path) -> io::Result<()> {
    let only: Vec<_> = std::fs::read_dir(dest)?.flatten().collect();
    if only.len() != 1 || only[0].file_name() != "app" || !only[0].file_type()?.is_dir() {
        return Ok(());
    }
    let app = only[0].path();
    // (A name clash with "app" inside: moved aside first.)
    let tmp = dest.join(format!(".polka-app-{}", std::process::id()));
    std::fs::rename(&app, &tmp)?;
    for e in std::fs::read_dir(&tmp)?.flatten() {
        std::fs::rename(e.path(), dest.join(e.file_name()))?;
    }
    std::fs::remove_dir(&tmp)
}

/// Where to unpack an installer: a new folder beside it, by its name.
pub fn dest_for(path: &Path) -> Option<PathBuf> {
    let dir = path.parent()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let stem = crate::ops::split_ext(&name).0.to_string();
    Some(crate::ops::unique_path(dir, &stem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages() {
        assert_eq!(percent("\u{1b}[K[=======>      ] 10.9%   244 MiB/s"), Some(10.9));
        assert_eq!(percent(" 42% 3 - big.bin"), Some(42.0));
        assert_eq!(percent(" - \"app/file.dat\""), None);
        assert_eq!(percent("100%"), Some(100.0));
    }

    #[test]
    fn candidates() {
        assert!(is_candidate("Setup.EXE"));
        assert!(is_candidate("pack.msi"));
        assert!(!is_candidate("readme.txt"));
    }
}
