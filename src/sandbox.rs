//! Running inside Flatpak: one build serves both, and only where the
//! sandbox differs (the user's real folders, the host's icons, starting the
//! host's programs) does it take another way.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Inside a Flatpak sandbox?
pub fn flatpak() -> bool {
    static F: OnceLock<bool> = OnceLock::new();
    *F.get_or_init(|| std::path::Path::new("/.flatpak-info").exists())
}

/// Before anything reads the environment: the user's real data, cache and
/// config folders (Flatpak points XDG_* into ~/.var/app — the trash, the
/// shared thumbnails, the icon themes in ~/.local/share would be wrong
/// there; settings are shared with a native install too), and the host's
/// icon themes.
pub fn fix_env() {
    if !flatpak() {
        return;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
    for (var, host, default) in [
        ("XDG_DATA_HOME", "HOST_XDG_DATA_HOME", ".local/share"),
        ("XDG_CACHE_HOME", "HOST_XDG_CACHE_HOME", ".cache"),
        ("XDG_CONFIG_HOME", "HOST_XDG_CONFIG_HOME", ".config"),
    ] {
        let real = std::env::var_os(host).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(default));
        unsafe { std::env::set_var(var, real) };
    }
    // Icon themes: the host's /usr/share (host-os) and its exported share.
    let mut dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/app/share:/usr/share".into());
    for d in ["/run/host/usr/share", "/run/host/share", "/run/host/user-share"] {
        if !dirs.split(':').any(|x| x == d) {
            dirs.push(':');
            dirs.push_str(d);
        }
    }
    unsafe { std::env::set_var("XDG_DATA_DIRS", dirs) };
}

/// A command run on the host instead (programs and launchers the user
/// starts live there, not in the sandbox).
pub fn host(cmd: std::process::Command) -> std::process::Command {
    if !flatpak() {
        return cmd;
    }
    let mut c = std::process::Command::new("flatpak-spawn");
    c.arg("--host");
    if let Some(dir) = cmd.get_current_dir() {
        c.arg(format!("--directory={}", dir.display()));
    }
    c.arg(cmd.get_program()).args(cmd.get_args());
    c
}

/// The host's file system root (the sandbox's own / is a stage set).
pub fn root() -> PathBuf {
    if flatpak() && std::path::Path::new("/run/host/root").is_dir() { "/run/host/root".into() } else { "/".into() }
}
