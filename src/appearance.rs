//! Light or dark: the user's choice, or the desktop's colour scheme read
//! from xdg-desktop-portal (and followed when it changes).

use std::io::BufRead;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};

/// The desktop's scheme: 0 unknown, 1 dark, 2 light.
static SYSTEM: AtomicU8 = AtomicU8::new(0);

/// Settings values (0 follows the desktop).
pub const DARK: u8 = 1;
pub const LIGHT: u8 = 2;

/// The portal's `color-scheme`: 1 prefers dark, 2 prefers light, 0 none.
fn scheme_from(text: &str) -> Option<u8> {
    let n = text.split("uint32").nth(1)?.trim_start().chars().next()?.to_digit(10)?;
    Some(match n {
        1 => DARK,
        2 => LIGHT,
        _ => 0,
    })
}

fn read_portal() -> Option<u8> {
    let out = Command::new("gdbus")
        .args(["call", "--session", "--dest", "org.freedesktop.portal.Desktop", "--object-path", "/org/freedesktop/portal/desktop"])
        .args(["--method", "org.freedesktop.portal.Settings.ReadOne", "org.freedesktop.appearance", "color-scheme"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    scheme_from(&String::from_utf8_lossy(&out.stdout))
}

/// No portal answer: GNOME's setting, then the GTK theme's name.
fn read_gsettings() -> Option<u8> {
    let get = |key: &str| {
        let out = Command::new("gsettings").args(["get", "org.gnome.desktop.interface", key]).stderr(Stdio::null()).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).to_lowercase())
    };
    match get("color-scheme").as_deref() {
        Some(s) if s.contains("dark") => return Some(DARK),
        Some(s) if s.contains("light") => return Some(LIGHT),
        _ => {}
    }
    get("gtk-theme").map(|t| if t.contains("dark") { DARK } else { LIGHT })
}

/// Start following the desktop (not in tests: they pick their theme).
pub fn watch(ctx: egui::Context) {
    if cfg!(test) {
        return;
    }
    std::thread::spawn(move || {
        let first = match read_portal() {
            Some(s) if s != 0 => s,
            // "No preference" is light by the portal's own definition.
            Some(_) => read_gsettings().unwrap_or(LIGHT),
            None => read_gsettings().unwrap_or(0),
        };
        SYSTEM.store(first, Ordering::Relaxed);
        ctx.request_repaint();
        // Changes arrive as SettingChanged signals.
        let Ok(mut child) = Command::new("gdbus")
            .args(["monitor", "--session", "--dest", "org.freedesktop.portal.Desktop", "--object-path", "/org/freedesktop/portal/desktop"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return;
        };
        let Some(out) = child.stdout.take() else { return };
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if line.contains("org.freedesktop.appearance") && line.contains("color-scheme") {
                if let Some(s) = scheme_from(&line) {
                    SYSTEM.store(if s == 0 { LIGHT } else { s }, Ordering::Relaxed);
                    ctx.request_repaint();
                }
            }
        }
    });
}

/// Dark or not, for the choice in the settings.
pub fn is_dark(choice: u8) -> bool {
    match choice {
        DARK => true,
        LIGHT => false,
        _ => SYSTEM.load(Ordering::Relaxed) != LIGHT,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_portal_answers() {
        use super::*;
        assert_eq!(scheme_from("(<uint32 1>,)"), Some(DARK));
        assert_eq!(scheme_from("(<<uint32 2>>,)"), Some(LIGHT));
        assert_eq!(
            scheme_from("/org/freedesktop/portal/desktop: org.freedesktop.portal.Settings.SettingChanged ('org.freedesktop.appearance', 'color-scheme', <uint32 0>)"),
            Some(0)
        );
        assert_eq!(scheme_from("nothing"), None);
    }
}
