//! `polka --diagnose`: what the app sees around it, for bug reports (and
//! for checking a sandboxed build without looking at its window).

use std::process::Command;

pub fn run() {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| "-".into());
    println!("polka {}", env!("CARGO_PKG_VERSION"));
    println!("flatpak: {}", crate::sandbox::flatpak());
    for k in ["XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_DIRS", "XDG_CURRENT_DESKTOP", "WAYLAND_DISPLAY", "DISPLAY", "LANG"] {
        println!("{k}: {}", env(k));
    }
    println!("language: {}", crate::i18n::current());
    println!("file system root: {}", crate::sandbox::root().display());
    println!("icon theme: {}", crate::icons::detect_theme().unwrap_or_else(|| "-".into()));
    let themes = crate::icons::installed_themes();
    println!("icon themes: {} ({})", themes.len(), themes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join(", "));
    for r in crate::trashbin::roots() {
        println!("trash: {}", r.display());
    }
    println!("trash items: {}", crate::trashbin::count());
    for m in crate::usage::mounts() {
        println!("mount: {} (read at {}) {} {}/{} bytes", m.host.display(), m.path.display(), m.fstype, m.used, m.total);
    }
    for d in crate::drives::list() {
        println!("drive: {} {:?} {} at {:?} removable={} udisks={}", d.dev, d.label, d.fstype, d.mount, d.removable, d.udisks.is_some());
    }
    let host = crate::sandbox::host(Command::new("true")).status();
    println!("run on host: {}", match host {
        Ok(s) => s.success().to_string(),
        Err(e) => e.to_string(),
    });
    for tool in ["7z", "bsdtar", "ffmpeg", "curl", "gdbus", "lsblk"] {
        let ok = Command::new(tool).arg("--help").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok();
        println!("tool {tool}: {}", if ok { "yes" } else { "missing" });
    }
    match gilrs::Gilrs::new() {
        Ok(mut g) => {
            // Devices come in as events: give them a moment.
            let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while std::time::Instant::now() < until {
                g.next_event_blocking(Some(std::time::Duration::from_millis(50)));
            }
            let pads: Vec<String> = g.gamepads().map(|(_, p)| p.name().to_string()).collect();
            println!("gamepads: {}", if pads.is_empty() { "-".into() } else { pads.join(", ") });
        }
        Err(e) => println!("gamepads: unavailable ({e})"),
    }
}
