//! "Add to Steam": a Windows .exe or a .desktop launcher becomes a
//! non-Steam game in the library (userdata/<user>/config/shortcuts.vdf),
//! with its icon, and Proton for an .exe. Steam keeps the list in memory
//! while it runs, so it's restarted to show it, or the shortcut shows after
//! its next start.

use std::path::{Path, PathBuf};

/// Binary key/values as Steam writes them (shortcuts.vdf).
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Map(Vec<(String, Node)>),
    Str(String),
    Int(u32),
    /// Other types kept as they were: the type and its bytes.
    Raw(u8, Vec<u8>),
}

fn cstr(b: &[u8], at: &mut usize) -> Option<String> {
    let end = b.get(*at..)?.iter().position(|&c| c == 0)?;
    let s = String::from_utf8_lossy(&b[*at..*at + end]).into_owned();
    *at += end + 1;
    Some(s)
}

fn parse_map(b: &[u8], at: &mut usize) -> Option<Vec<(String, Node)>> {
    let mut out = Vec::new();
    loop {
        let ty = *b.get(*at)?;
        *at += 1;
        if ty == 0x08 {
            return Some(out);
        }
        let key = cstr(b, at)?;
        let node = match ty {
            0x00 => Node::Map(parse_map(b, at)?),
            0x01 => Node::Str(cstr(b, at)?),
            0x02 => {
                let v = u32::from_le_bytes(b.get(*at..*at + 4)?.try_into().ok()?);
                *at += 4;
                Node::Int(v)
            }
            0x03 | 0x04 | 0x06 => {
                let v = b.get(*at..*at + 4)?.to_vec();
                *at += 4;
                Node::Raw(ty, v)
            }
            0x07 | 0x0a => {
                let v = b.get(*at..*at + 8)?.to_vec();
                *at += 8;
                Node::Raw(ty, v)
            }
            _ => return None,
        };
        out.push((key, node));
    }
}

/// The whole file: one root map.
pub fn parse(b: &[u8]) -> Option<Vec<(String, Node)>> {
    let mut at = 0;
    let m = parse_map(b, &mut at)?;
    Some(m)
}

fn write_map(out: &mut Vec<u8>, m: &[(String, Node)]) {
    for (k, v) in m {
        let ty = match v {
            Node::Map(_) => 0x00,
            Node::Str(_) => 0x01,
            Node::Int(_) => 0x02,
            Node::Raw(t, _) => *t,
        };
        out.push(ty);
        out.extend(k.as_bytes());
        out.push(0);
        match v {
            Node::Map(inner) => write_map(out, inner),
            Node::Str(s) => {
                out.extend(s.as_bytes());
                out.push(0);
            }
            Node::Int(i) => out.extend(i.to_le_bytes()),
            Node::Raw(_, bytes) => out.extend(bytes),
        }
    }
    out.push(0x08);
}

/// The whole file (the root map's own end closes it: `… 08 08`).
pub fn write(root: &[(String, Node)]) -> Vec<u8> {
    let mut out = Vec::new();
    write_map(&mut out, root);
    out
}

/// A program to add: its name, what to run, where, and its picture.
#[derive(Clone, Debug)]
pub struct Shortcut {
    pub name: String,
    pub exe: String,
    pub start_dir: String,
    pub options: String,
    pub icon: Option<PathBuf>,
    /// A Windows program: run with Proton.
    pub windows: bool,
}

/// What Steam calls a shortcut: from its exe and name.
pub fn shortcut_id(exe: &str, name: &str) -> u32 {
    crc32(format!("{exe}{name}").as_bytes()) | 0x8000_0000
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xedb8_8320 } else { c >> 1 };
        }
    }
    !c
}

fn quoted(p: &str) -> String {
    format!("\"{p}\"")
}

/// A shortcut for a Windows program or a .desktop launcher.
pub fn shortcut_for(path: &Path, icon: Option<PathBuf>) -> Option<Shortcut> {
    let host = crate::sandbox::host_path(path);
    let lower = path.to_string_lossy().to_lowercase();
    let dir = host.parent().map(|d| d.display().to_string()).unwrap_or_default();
    if lower.ends_with(".desktop") {
        let text = std::fs::read_to_string(path).ok()?;
        let mut in_entry = false;
        let (mut name, mut exec, mut workdir) = (None, None, None);
        for l in text.lines() {
            let l = l.trim();
            if l.starts_with('[') {
                in_entry = l == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            match l.split_once('=') {
                Some(("Name", v)) if name.is_none() => name = Some(v.trim().to_string()),
                Some(("Exec", v)) if exec.is_none() => exec = Some(v.trim().to_string()),
                Some(("Path", v)) if workdir.is_none() => workdir = Some(v.trim().to_string()),
                _ => {}
            }
        }
        // Without the field codes (%f, %U…).
        let exec = exec?;
        let words: Vec<&str> = exec.split_whitespace().filter(|w| !(w.len() == 2 && w.starts_with('%'))).collect();
        let (prog, rest) = words.split_first()?;
        return Some(Shortcut {
            name: name.unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
            exe: quoted(prog.trim_matches('"')),
            start_dir: quoted(&workdir.unwrap_or_else(|| dirs::home_dir().map(|h| h.display().to_string()).unwrap_or_default())),
            options: rest.join(" "),
            icon,
            windows: false,
        });
    }
    let name = path.file_stem()?.to_string_lossy().into_owned();
    Some(Shortcut { name, exe: quoted(&host.display().to_string()), start_dir: quoted(&dir), options: String::new(), icon, windows: lower.ends_with(".exe") || lower.ends_with(".bat") || lower.ends_with(".msi") })
}

/// The Steam account last signed in (its userdata folder).
pub fn user_dir(root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(root.join("config/loginusers.vdf")).unwrap_or_default();
    // "7656…" { … "MostRecent" "1" … }: the 64-bit id, as a folder number.
    let mut current: Option<u64> = None;
    let mut recent: Option<u64> = None;
    for l in text.lines() {
        let t = l.trim().trim_matches('"');
        if t.len() == 17 && t.bytes().all(|b| b.is_ascii_digit()) {
            current = t.parse().ok();
        } else if l.contains("\"MostRecent\"") && l.contains("\"1\"") {
            recent = current;
        }
    }
    let folder = |id: u64| root.join("userdata").join((id - 76_561_197_960_265_728).to_string());
    if let Some(d) = recent.or(current).map(folder).filter(|d| d.is_dir()) {
        return Some(d);
    }
    // Else the one used last (not the anonymous 0).
    std::fs::read_dir(root.join("userdata"))
        .ok()?
        .flatten()
        .filter(|e| e.file_name() != "0" && e.path().is_dir())
        .max_by_key(|e| e.path().join("config").metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

/// Added already? (the same program)
pub fn present(root: &Path, s: &Shortcut) -> bool {
    let Some(user) = user_dir(root) else { return false };
    let Some(tree) = std::fs::read(user.join("config/shortcuts.vdf")).ok().and_then(|b| parse(&b)) else { return false };
    let Some((_, Node::Map(list))) = tree.iter().find(|(k, _)| k.eq_ignore_ascii_case("shortcuts")) else { return false };
    list.iter().any(|(_, e)| match e {
        Node::Map(f) => f.iter().any(|(k, v)| k.eq_ignore_ascii_case("exe") && *v == Node::Str(s.exe.clone())) && f.iter().any(|(k, v)| k.eq_ignore_ascii_case("LaunchOptions") && *v == Node::Str(s.options.clone())),
        _ => false,
    })
}

/// Writes the shortcut into the library (and Proton for it, when Steam
/// doesn't use it for everything already). Steam should not be running.
pub fn add(root: &Path, s: &Shortcut) -> Result<u32, String> {
    let user = user_dir(root).ok_or("no Steam account here")?;
    let file = user.join("config/shortcuts.vdf");
    let mut tree = match std::fs::read(&file) {
        Ok(b) => parse(&b).ok_or("shortcuts.vdf isn't readable")?,
        Err(_) => vec![("shortcuts".to_string(), Node::Map(Vec::new()))],
    };
    let id = shortcut_id(&s.exe, &s.name);
    // A stable copy of the picture (caches come and go).
    let icon = s.icon.as_ref().and_then(|i| {
        let to = dirs::data_dir()?.join(format!("polka/steam-icons/{id}.{}", i.extension().and_then(|e| e.to_str()).unwrap_or("png")));
        std::fs::create_dir_all(to.parent()?).ok()?;
        std::fs::copy(i, &to).ok()?;
        Some(crate::sandbox::host_path(&to).display().to_string())
    });
    let entry = vec![
        ("appid".to_string(), Node::Int(id)),
        ("AppName".into(), Node::Str(s.name.clone())),
        ("Exe".into(), Node::Str(s.exe.clone())),
        ("StartDir".into(), Node::Str(s.start_dir.clone())),
        ("icon".into(), Node::Str(icon.unwrap_or_default())),
        ("ShortcutPath".into(), Node::Str(String::new())),
        ("LaunchOptions".into(), Node::Str(s.options.clone())),
        ("IsHidden".into(), Node::Int(0)),
        ("AllowDesktopConfig".into(), Node::Int(1)),
        ("AllowOverlay".into(), Node::Int(1)),
        ("OpenVR".into(), Node::Int(0)),
        ("Devkit".into(), Node::Int(0)),
        ("DevkitGameID".into(), Node::Str(String::new())),
        ("DevkitOverrideAppID".into(), Node::Int(0)),
        ("LastPlayTime".into(), Node::Int(0)),
        ("FlatpakAppID".into(), Node::Str(String::new())),
        ("tags".into(), Node::Map(Vec::new())),
    ];
    let list = match tree.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case("shortcuts")) {
        Some((_, Node::Map(l))) => l,
        _ => return Err("shortcuts.vdf isn't readable".into()),
    };
    let next = list.iter().filter_map(|(k, _)| k.parse::<usize>().ok()).max().map_or(0, |m| m + 1);
    list.push((next.to_string(), Node::Map(entry)));
    std::fs::create_dir_all(file.parent().ok_or("no folder")?).map_err(|e| e.to_string())?;
    let tmp = file.with_extension("vdf.polka");
    std::fs::write(&tmp, write(&tree)).and_then(|_| std::fs::rename(&tmp, &file)).map_err(|e| e.to_string())?;
    if s.windows {
        proton_for(root, id);
    }
    Ok(id)
}

/// Proton Experimental for a Windows shortcut, unless Steam Play is on for
/// every title already (a default tool, "0").
fn proton_for(root: &Path, id: u32) {
    let cfg = root.join("config/config.vdf");
    let Ok(text) = std::fs::read_to_string(&cfg) else { return };
    let Some(at) = text.find("\"CompatToolMapping\"") else { return };
    let Some(open) = text[at..].find('{').map(|i| at + i + 1) else { return };
    // The block's own entries: is there a "0" (the default for all)?
    let block_end = {
        let mut depth = 1;
        let mut end = None;
        for (i, ch) in text[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(e) => e,
            None => return,
        }
    };
    let block = &text[open..block_end];
    if block.contains("\"0\"") || block.contains(&format!("\"{id}\"")) {
        return;
    }
    let have_experimental = crate::steam::libraries().iter().any(|l| l.join("steamapps/common/Proton - Experimental").is_dir());
    if !have_experimental {
        return;
    }
    let indent = text[..at].rsplit('\n').next().unwrap_or("").to_string() + "\t";
    let entry = format!("\n{indent}\"{id}\"\n{indent}{{\n{indent}\t\"name\"\t\t\"proton_experimental\"\n{indent}\t\"config\"\t\t\"\"\n{indent}\t\"priority\"\t\t\"250\"\n{indent}}}");
    let mut out = text.clone();
    out.insert_str(open, &entry);
    let tmp = cfg.with_extension("vdf.polka");
    if std::fs::write(&tmp, out).is_ok() {
        let _ = std::fs::rename(&tmp, &cfg);
    }
}

/// Is Steam running? (on the host, from a sandbox)
pub fn running() -> bool {
    if let Some(v) = std::env::var_os("POLKA_STEAM_RUNNING") {
        return v == "1";
    }
    let mut c = std::process::Command::new("pgrep");
    c.args(["-x", "steam"]);
    crate::sandbox::host(c).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
}

/// In Steam's game mode (Steam is the whole session: restarting it would
/// close everything, this too).
pub fn game_mode() -> bool {
    std::env::var_os("SteamGamepadUI").is_some() || std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.eq_ignore_ascii_case("gamescope"))
}

/// The command that starts this Steam (a Flatpak one too).
fn steam_cmd(root: &Path) -> Vec<&'static str> {
    if root.to_string_lossy().contains("com.valvesoftware.Steam") { vec!["flatpak", "run", "com.valvesoftware.Steam"] } else { vec!["steam"] }
}

/// Steam closed, the shortcut written, Steam started again.
pub fn restart_and_add(root: &Path, s: &Shortcut) -> Result<u32, String> {
    let cmd = steam_cmd(root);
    let mut stop = std::process::Command::new(cmd[0]);
    stop.args(&cmd[1..]).arg("-shutdown");
    let _ = crate::sandbox::host(stop).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    for _ in 0..120 {
        if !running() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    if running() {
        return Err("Steam didn't close".into());
    }
    let id = add(root, s)?;
    let mut start = std::process::Command::new(cmd[0]);
    start.args(&cmd[1..]);
    let _ = crate::launch::spawn_cmd(start);
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vdf_round_trip_and_ids() {
        let tree = vec![(
            "shortcuts".to_string(),
            Node::Map(vec![("0".into(), Node::Map(vec![("appid".into(), Node::Int(7)), ("AppName".into(), Node::Str(String::from("Game"))), ("x".into(), Node::Raw(0x07, vec![1; 8]))]))]),
        )];
        let b = write(&tree);
        // Byte for byte as Steam has it: a map, a key, its entries, an end each.
        let mut want = vec![0x00];
        want.extend(b"shortcuts\0");
        want.push(0x00);
        want.extend(b"0\0");
        want.push(0x02);
        want.extend(b"appid\0");
        want.extend(7u32.to_le_bytes());
        want.push(0x01);
        want.extend(b"AppName\0Game\0");
        want.push(0x07);
        want.extend(b"x\0");
        want.extend([1; 8]);
        want.extend([0x08, 0x08, 0x08]);
        assert_eq!(b, want);
        assert_eq!(parse(&b), Some(tree));
        // As Steam (and Steam ROM Manager) compute it.
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert!(shortcut_id("\"/x.exe\"", "X") & 0x8000_0000 != 0);
    }
}

// ---------------------------------------------------------------- the app

impl crate::app::App {
    pub fn steam_add(&mut self, path: &Path) {
        use crate::app::{Cmd, Menu, Overlay, item};
        use crate::i18n::tr;
        let Some(root) = crate::steam::root() else { return };
        // Its picture: the exe's own, or the launcher's theme icon.
        let lower = path.to_string_lossy().to_lowercase();
        let icon = if lower.ends_with(".desktop") {
            crate::icons::desktop_icon(path).and_then(|i| match i.strip_prefix("file://") {
                Some(p) => Some(PathBuf::from(p)),
                None => self.icons.named(&[i.as_str()], 256).and_then(|u| u.strip_prefix("file://").map(PathBuf::from)),
            })
        } else {
            crate::icons::exe_icon(path)
        };
        let Some(s) = shortcut_for(path, icon) else {
            return self.toast(format!("{}: {}", tr("Failed"), path.display()), true);
        };
        if present(&root, &s) {
            return self.toast(tr("Already in Steam"), false);
        }
        if !running() {
            return self.steam_add_now(s, false);
        }
        // Steam keeps the list in memory while it runs.
        let mut items = Vec::new();
        if !game_mode() {
            items.push(item(tr("Restart Steam and add"), Cmd::SteamAddNow { s: s.clone(), restart: true }));
        }
        items.push(item(tr("Add (shows after Steam restarts)"), Cmd::SteamAddNow { s, restart: false }));
        items.push(item(tr("Cancel"), Cmd::Nothing));
        self.overlay = Some(Overlay::Menu(Menu::new(tr("Steam is running"), items)));
    }

    pub fn steam_add_now(&mut self, s: Shortcut, restart: bool) {
        use crate::i18n::tr;
        let Some(root) = crate::steam::root() else { return };
        if !restart {
            match add(&root, &s) {
                Ok(_) => self.toast(format!("{}: {}", tr("Added to Steam"), s.name), false),
                Err(e) => self.toast(format!("{}: {e}", tr("Failed")), true),
            }
            return;
        }
        self.toast(tr("Restarting Steam…"), false);
        let (tx, wake) = (self.msg_tx.clone(), self.wake());
        std::thread::spawn(move || {
            let msg = match restart_and_add(&root, &s) {
                Ok(_) => crate::app::Msg::Toast(format!("{}: {}", tr("Added to Steam"), s.name), false),
                Err(e) => crate::app::Msg::Toast(format!("{}: {e}", tr("Failed")), true),
            };
            let _ = tx.send(msg);
            wake();
        });
    }
}
