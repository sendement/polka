//! A small log in ~/.cache/polka/polka.log: how Polka was started (where
//! things differ between launchers, like Steam's game mode), video playback
//! figures and crashes — something to send along with a bug report.

use std::io::Write;
use std::path::PathBuf;

const MAX: u64 = 512 * 1024;

fn path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("polka/polka.log"))
}

pub fn line(s: impl AsRef<str>) {
    let Some(p) = path() else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    // Kept short: the older half goes to polka.log.old.
    if std::fs::metadata(&p).is_ok_and(|m| m.len() > MAX) {
        let _ = std::fs::rename(&p, p.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{} {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), s.as_ref());
    }
}

/// What this run is: version, sandbox, session, locale, launcher.
pub fn startup() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut parts = vec![format!("start polka {}", env!("CARGO_PKG_VERSION"))];
    if crate::sandbox::flatpak() {
        parts.push("flatpak".into());
    }
    for k in ["XDG_SESSION_TYPE", "WAYLAND_DISPLAY", "DISPLAY", "GAMESCOPE_WAYLAND_DISPLAY", "SteamDeck", "SteamGameId", "LANG", "LC_ALL", "LC_CTYPE"] {
        if let Some(v) = env(k) {
            parts.push(format!("{k}={v}"));
        }
    }
    line(parts.join(" "));
}

/// Panics go to the log too (then on to the usual message).
pub fn catch_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let what = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        line(format!("panic in {}: {what} at {at}", std::thread::current().name().unwrap_or("?")));
        default(info);
    }));
}
