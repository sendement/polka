//! Repacks (FitGirl, DODI…) unpacked: their installer is Inno Setup, their
//! data FreeArc archives squeezed by compressors made for Windows alone
//! (lolz, xtool, Oodle…). The installer itself isn't run: innoextract takes
//! out what it carries (unarc.dll, the compressors, their settings), and
//! Polka's own little unarc-cli.exe (tools/unarc-cli) unpacks the archives
//! one by one with them under Wine, in a prefix of Polka's own where no
//! window can show.

use crate::i18n::tr;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;

/// Built from tools/unarc-cli/unarc-cli.c (see its README).
const UNARC_CLI: &[u8] = include_bytes!("../tools/unarc-cli/unarc-cli.exe");

/// Polka's Wine prefix: its own, set to draw no windows.
fn prefix() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("polka/wine")
}

/// Wine with the prefix and quiet.
fn wine(args: &[&std::ffi::OsStr], cwd: &Path) -> Command {
    let mut c = Command::new("wine");
    c.args(args).current_dir(cwd).env("WINEPREFIX", prefix()).env("WINEDEBUG", "-all").env("WINEDLLOVERRIDES", "mscoree,mshtml=");
    crate::sandbox::host(c)
}

/// The prefix made (the first time) with the null graphics driver: a
/// message box from a compressor goes nowhere instead of onto the screen.
fn ready_prefix(cwd: &Path) -> io::Result<()> {
    let mark = prefix().join(".polka-null-graphics");
    if mark.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(prefix())?;
    let words = ["reg", "add", "HKCU\\Software\\Wine\\Drivers", "/v", "Graphics", "/d", "null", "/f"];
    let args: Vec<&std::ffi::OsStr> = words.iter().map(std::ffi::OsStr::new).collect();
    let ok = wine(&args, cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status()?.success();
    if !ok {
        return Err(io::Error::other(tr("Wine couldn't start")));
    }
    std::fs::write(mark, "")
}

/// Stop whatever is left in the prefix (a compressor waiting for nothing).
fn stop_wine(cwd: &Path) {
    let mut c = Command::new("wineserver");
    c.arg("-k").current_dir(cwd).env("WINEPREFIX", prefix());
    let _ = crate::sandbox::host(c).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// How far into `file` some process reading it is (from /proc).
pub fn read_position(file: &Path) -> Option<u64> {
    let want = std::fs::canonicalize(file).ok()?;
    let mut best = None;
    for p in std::fs::read_dir("/proc").ok()?.flatten() {
        let name = p.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(p.path().join("fd")) else { continue };
        for fd in fds.flatten() {
            if std::fs::read_link(fd.path()).is_ok_and(|l| l == want) {
                let info = std::fs::read_to_string(p.path().join("fdinfo").join(fd.file_name())).unwrap_or_default();
                if let Some(pos) = info.lines().find_map(|l| l.strip_prefix("pos:")).and_then(|v| v.trim().parse::<u64>().ok()) {
                    best = Some(best.map_or(pos, |b: u64| b.max(pos)));
                }
            }
        }
    }
    best
}

/// Windows' name for a path (through Wine's Z: drive).
fn win_path(p: &Path) -> String {
    format!("Z:{}", p.to_string_lossy().replace('/', "\\"))
}

/// Unpack the repack whose installer is `setup` (its archives in order) into
/// the new folder `dest`.
pub fn unpack(setup: &Path, archives: &[PathBuf], dest: &Path, job: &crate::ops::Job) -> io::Result<()> {
    let work = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join(format!("polka/repack-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let r = run(setup, archives, dest, &work, job);
    stop_wine(&work);
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(dest.join(".polka-temp"));
    r
}

fn run(setup: &Path, archives: &[PathBuf], dest: &Path, work: &Path, job: &crate::ops::Job) -> io::Result<()> {
    let sizes: Vec<u64> = archives.iter().map(|a| std::fs::metadata(a).map(|m| m.len()).unwrap_or(0)).collect();
    let total: u64 = sizes.iter().sum::<u64>().max(1);
    job.total.store(total, Ordering::Relaxed);
    job.now_on(setup);
    std::fs::create_dir_all(dest)?;
    // 1. What the installer carries: its own files ({app}: into dest) and
    // its temporary ones (the unpacker and the compressors).
    let inno = work.join("inno");
    let mut c = Command::new("innoextract");
    c.args(["-e", "-q", "-p0", "-c0", "--collisions=rename", "-d"]).arg(&inno).arg("--").arg(setup);
    let out = crate::sandbox::host(c).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output()?;
    if !out.status.success() {
        let e = String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or_default().to_string();
        return Err(io::Error::other(format!("innoextract: {e}")));
    }
    let tmp = inno.join("tmp");
    if !tmp.join("unarc.dll").is_file() {
        return Err(io::Error::other(tr("This repack's installer has no unarc.dll: it isn't unpacked this way")));
    }
    job.check()?;
    // 2. The working folder: the compressors, unarc-cli, and plain names
    // (unarc takes no others) for where the archives are and where to put
    // what's in them.
    let bin = work.join("bin");
    std::fs::rename(&tmp, &bin)?;
    std::fs::write(bin.join("unarc-cli.exe"), UNARC_CLI)?;
    let temp = dest.join(".polka-temp");
    std::fs::create_dir_all(&temp)?;
    std::os::unix::fs::symlink(&temp, bin.join("t"))?;
    std::os::unix::fs::symlink(dest, bin.join("dst"))?;
    // Their settings say {app} for the installer's folder (scratch room).
    for ini in ["CLS.ini", "cls.ini", "arc.ini"] {
        let p = bin.join(ini);
        if let Ok(text) = std::fs::read_to_string(&p) {
            let _ = std::fs::write(&p, text.replace("{app}", &win_path(&bin.join("t"))));
        }
    }
    ready_prefix(&bin)?;
    // 3. Each archive in turn.
    let mut done_before = 0u64;
    for (i, a) in archives.iter().enumerate() {
        job.check()?;
        job.now_on(a);
        let dir = a.parent().unwrap_or(Path::new("/"));
        let link = bin.join("src");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(dir, &link)?;
        let name = a.file_name().unwrap_or_default();
        // (A non-ASCII name: a link with a plain one.)
        let plain = if name.to_string_lossy().is_ascii() {
            name.to_string_lossy().into_owned()
        } else {
            let n = format!("archive-{i}.bin");
            std::os::unix::fs::symlink(a, bin.join(&n))?;
            n
        };
        let arg = if plain.starts_with("archive-") { plain.clone() } else { format!("src\\{plain}") };
        let words = ["unarc-cli.exe", arg.as_str(), "dst"];
        let args: Vec<&std::ffi::OsStr> = words.iter().map(std::ffi::OsStr::new).collect();
        let mut child = wine(&args, &bin).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| io::Error::other(format!("wine: {e}")))?;
        let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let reader = child.stdout.take().map(|o| {
            let errors = errors.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(o).lines().map_while(Result::ok) {
                    if let Some(e) = line.strip_prefix("error ") {
                        // ("error -3 CLS: text": a compressor's message.)
                        let text = e.split_once(' ').map_or(e, |(_, t)| t).trim().to_string();
                        errors.lock().unwrap().push(text);
                    }
                }
            })
        });
        let mut last_pos = 0u64;
        let mut ticks = 0u32;
        let status = loop {
            if job.cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                stop_wine(&bin);
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            if let Some(s) = child.try_wait()? {
                break s;
            }
            // Where in the archive it reads (once a second).
            ticks += 1;
            if ticks % 5 == 0 {
                if let Some(pos) = read_position(a) {
                    last_pos = pos.min(sizes[i]);
                    job.done.store(done_before + last_pos, Ordering::Relaxed);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        };
        if let Some(r) = reader {
            let _ = r.join();
        }
        let errs = errors.lock().unwrap().clone();
        if !status.success() || !errs.is_empty() {
            let why = errs.last().cloned().unwrap_or_else(|| format!("unarc: {status}"));
            return Err(io::Error::other(format!("{}: {why}", a.file_name().unwrap_or_default().to_string_lossy())));
        }
        let _ = last_pos;
        done_before += sizes[i];
        job.done.store(done_before, Ordering::Relaxed);
    }
    // 4. The installer's own files ({app}) beside what came out.
    let app = inno.join("app");
    if app.is_dir() {
        merge(&app, dest)?;
    }
    // A list of what there should be (FitGirl's): anything missing said.
    let missing = missing_listed(dest);
    if missing > 0 {
        *job.note.lock().unwrap() = Some(tr("Unpacked, but %n of the files the repack lists are missing").replace("%n", &missing.to_string()));
    }
    Ok(())
}

/// Move `from`'s contents into `to`, folders merged (what's there kept).
fn merge(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() && target.is_dir() {
            merge(&e.path(), &target)?;
        } else if target.symlink_metadata().is_err() {
            std::fs::rename(e.path(), &target)?;
        }
    }
    Ok(())
}

/// The files a FitGirl list (_Redist/fitgirl.md5) names that aren't there.
pub fn missing_listed(dest: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(dest.join("_Redist/fitgirl.md5")) else { return 0 };
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with(';'))
        .filter_map(|l| l.split_once(" *").map(|(_, p)| p.trim()))
        .map(|p| p.trim_start_matches("..\\").replace('\\', "/"))
        .filter(|p| !dest.join(p).is_file())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_files() {
        let d = std::env::temp_dir().join(format!("polka-repack-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("_Redist")).unwrap();
        std::fs::create_dir_all(d.join("Game/Bin")).unwrap();
        std::fs::write(d.join("Game/Bin/game.exe"), "x").unwrap();
        std::fs::write(d.join("_Redist/fitgirl.md5"), "; comment\n0123 *..\\Game\\Bin\\game.exe\n4567 *..\\Game\\Data\\gone.pak\n").unwrap();
        assert_eq!(missing_listed(&d), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A real repack (POLKA_REPACK_DIR: its setup.exe and one archive),
    /// unpacked as the menu would, its files checked against its list.
    #[test]
    #[ignore]
    fn real_repack() {
        let Some(src) = std::env::var_os("POLKA_REPACK_DIR").map(PathBuf::from) else { return };
        let archive = std::env::var("POLKA_REPACK_ARCHIVE").unwrap_or_else(|_| "fg-03.bin".into());
        let tools = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/tools/usr/bin"));
        let path = std::env::var_os("PATH").unwrap_or_default();
        unsafe { std::env::set_var("PATH", std::env::join_paths(std::iter::once(tools).chain(std::env::split_paths(&path))).unwrap()) };
        // (Its own Wine prefix and scratch room, not the user's cache.)
        unsafe { std::env::set_var("XDG_CACHE_HOME", concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-cache")) };
        let d = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/repack-test"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::os::unix::fs::symlink(src.join("setup.exe"), d.join("setup.exe")).unwrap();
        std::os::unix::fs::symlink(src.join(&archive), d.join(&archive)).unwrap();
        let kind = crate::installers::detect(&d.join("setup.exe")).unwrap();
        assert!(matches!(&kind, crate::installers::Kind::Repack(a) if a.len() == 1), "{kind:?}");
        let dest = d.join("setup");
        let job = crate::ops::spawn(crate::ops::Op::Unpack { installer: d.join("setup.exe"), dest: dest.clone() }, || {});
        let mut last = 0;
        while job.finished_at.lock().unwrap().is_none() {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let now = job.done.load(Ordering::Relaxed);
            if now != last {
                eprintln!("progress {now} / {}", job.total.load(Ordering::Relaxed));
                last = now;
            }
        }
        assert_eq!(job.result.lock().unwrap().clone().unwrap(), Ok(()));
        assert!(job.done.load(Ordering::Relaxed) >= job.total.load(Ordering::Relaxed));
        // Every file out of it as listed (by MD5), the rest of the list not
        // there (its other archives weren't given).
        let list = std::fs::read_to_string(dest.join("_Redist/fitgirl.md5")).expect("the list");
        let (mut ok, mut bad) = (0, Vec::new());
        for l in list.lines().filter(|l| !l.starts_with(';')) {
            let Some((h, p)) = l.split_once(" *") else { continue };
            let p = dest.join(p.trim().trim_start_matches("..\\").replace('\\', "/"));
            if !p.is_file() {
                continue;
            }
            let out = std::process::Command::new("md5sum").arg(&p).output().unwrap();
            if String::from_utf8_lossy(&out.stdout).starts_with(h.trim()) { ok += 1 } else { bad.push(p) }
        }
        eprintln!("checked {ok}, wrong {bad:?}; note {:?}", job.note.lock().unwrap());
        assert!(ok > 0 && bad.is_empty());
        assert!(!dest.join(".polka-temp").exists());
        // Nothing of Wine left running.
        let left = std::fs::read_dir("/proc").unwrap().flatten().filter(|p| std::fs::read_to_string(p.path().join("cmdline")).is_ok_and(|c| c.starts_with("Z:\\") && c.contains("polka"))).count();
        assert_eq!(left, 0, "Wine processes left");
    }

    #[test]
    fn windows_paths() {
        assert_eq!(win_path(Path::new("/home/u/x")), "Z:\\home\\u\\x");
    }

    #[test]
    fn read_positions() {
        use std::io::{Read, Seek};
        let p = std::env::temp_dir().join(format!("polka-repack-pos-{}", std::process::id()));
        std::fs::write(&p, vec![0u8; 10_000]).unwrap();
        let mut f = std::fs::File::open(&p).unwrap();
        f.seek(std::io::SeekFrom::Start(4096)).unwrap();
        let mut b = [0u8; 4];
        f.read_exact(&mut b).unwrap();
        assert_eq!(read_position(&p), Some(4100));
        drop(f);
        let _ = std::fs::remove_file(&p);
    }
}
