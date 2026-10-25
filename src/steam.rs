//! Steam in the sidebar: the installed games (from every library), the
//! Proton prefixes, the screenshots and the game recordings, each by game,
//! named after their games instead of the app ids Steam gives their folders.
//! A game's screenshots are their real folder; its recordings (DASH videos,
//! a session.mpd and its chunks each) are listed one by one, to play.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::vfs::{Entry, Location};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Root,
    Games,
    Prefixes,
    Screenshots,
    /// Games with recordings…
    Recordings,
    /// …and one's recordings (by app id).
    Recorded(u32),
}

/// A game folder or a prefix, under the name shown for it.
#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub path: PathBuf,
    pub appid: Option<u32>,
    /// A file (a recording): its size.
    pub size: Option<u64>,
}

/// Steam installs found (native, ~/.steam links, Flatpak), each once.
pub fn roots() -> Vec<PathBuf> {
    // Tests bring a Steam of their own.
    if let Some(r) = std::env::var_os("POLKA_STEAM_ROOT") {
        let r = PathBuf::from(r);
        return if r.join("steamapps").is_dir() { vec![r] } else { Vec::new() };
    }
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let candidates = [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".steam/root"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
    ];
    let mut out: Vec<PathBuf> = Vec::new();
    for c in candidates {
        let Ok(real) = c.canonicalize() else { continue };
        if real.join("steamapps").is_dir() && !out.contains(&real) {
            out.push(real);
        }
    }
    out
}

pub fn root() -> Option<PathBuf> {
    roots().into_iter().next()
}

/// The value of `"key"  "value"` on a line of Valve's text format.
fn kv_line(line: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = line.trim().split('"').collect();
    // "" key "\t\t" value ""
    if parts.len() >= 5 && parts[0].is_empty() {
        Some((parts[1].to_string(), parts[3].replace("\\\\", "\\")))
    } else {
        None
    }
}

/// Every library folder (with a steamapps folder in it).
pub fn libraries() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for r in roots() {
        let mut libs = vec![r.clone()];
        if let Ok(text) = std::fs::read_to_string(r.join("steamapps/libraryfolders.vdf")) {
            libs.extend(text.lines().filter_map(kv_line).filter(|(k, _)| k == "path").map(|(_, v)| PathBuf::from(v)));
        }
        for l in libs {
            let l = l.canonicalize().unwrap_or(l);
            if l.join("steamapps").is_dir() && !out.contains(&l) {
                out.push(l);
            }
        }
    }
    out
}

struct Manifest {
    appid: u32,
    name: String,
    dir: PathBuf,
}

fn manifests(lib: &Path) -> Vec<Manifest> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(lib.join("steamapps")) else { return out };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !(n.starts_with("appmanifest_") && n.ends_with(".acf")) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let mut appid = None;
        let mut name = None;
        let mut dir = None;
        for (k, v) in text.lines().filter_map(kv_line) {
            match k.as_str() {
                "appid" if appid.is_none() => appid = v.parse().ok(),
                "name" if name.is_none() => name = Some(v),
                "installdir" if dir.is_none() => dir = Some(v),
                _ => {}
            }
        }
        if let (Some(appid), Some(dir)) = (appid, dir) {
            let name = name.unwrap_or_else(|| dir.clone());
            out.push(Manifest { appid, name, dir: lib.join("steamapps/common").join(dir) });
        }
    }
    out
}

// ---------------------------------------------------------------- binary KV

/// Reads Valve's binary key/values, calling `f(path, value)` for strings
/// and integers. `key` reads a key (a string, or an index into a table).
struct Kv<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Kv<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.at)?;
        self.at += 1;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.b.get(self.at..self.at + 4)?;
        self.at += 4;
        Some(u32::from_le_bytes(s.try_into().ok()?))
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        (self.at + n <= self.b.len()).then(|| self.at += n)
    }
    fn cstr(&mut self) -> Option<&'a str> {
        let rest = self.b.get(self.at..)?;
        let end = rest.iter().position(|&c| c == 0)?;
        self.at += end + 1;
        std::str::from_utf8(&rest[..end]).ok()
    }

    fn walk(&mut self, path: &mut Vec<String>, table: Option<&[String]>, f: &mut dyn FnMut(&[String], Value)) -> Option<()> {
        loop {
            let ty = self.u8()?;
            if ty == 0x08 {
                return Some(());
            }
            let key = match table {
                Some(t) => t.get(self.u32()? as usize)?.clone(),
                None => self.cstr()?.to_string(),
            };
            path.push(key);
            match ty {
                0x00 => self.walk(path, table, f)?,
                0x01 => {
                    let v = self.cstr()?;
                    f(path, Value::Str(v));
                }
                0x02 => {
                    let v = self.u32()?;
                    f(path, Value::Int(v));
                }
                0x03 | 0x04 | 0x06 => self.skip(4)?,
                0x07 | 0x0a => self.skip(8)?,
                0x05 => {
                    // UTF-16, ends in two zero bytes.
                    while self.u8()? != 0 || self.u8()? != 0 {}
                }
                _ => return None,
            }
            path.pop();
        }
    }
}

enum Value<'a> {
    Str(&'a str),
    Int(u32),
}

/// What Steam's app cache says of an app.
#[derive(Default)]
struct Info {
    name: Option<String>,
    /// game, tool, config, application…
    kind: Option<String>,
    /// Its folder under steamapps/common.
    dir: Option<String>,
}

/// Everything Steam knows (installed or not) from its app cache.
fn appinfo(root: &Path) -> HashMap<u32, Info> {
    let mut out = HashMap::new();
    let Ok(b) = std::fs::read(root.join("appcache/appinfo.vdf")) else { return out };
    let mut r = Kv { b: &b, at: 0 };
    let Some(magic) = r.u32() else { return out };
    let _universe = r.u32();
    let v29 = magic == 0x0756_4429;
    if !v29 && magic != 0x0756_4428 {
        return out;
    }
    let mut table: Vec<String> = Vec::new();
    if v29 {
        let Some(off) = b.get(8..16).map(|s| u64::from_le_bytes(s.try_into().unwrap()) as usize) else { return out };
        r.at = 16;
        let mut t = Kv { b: &b, at: off };
        let n = t.u32().unwrap_or(0);
        for _ in 0..n {
            match t.cstr() {
                Some(s) => table.push(s.to_string()),
                None => break,
            }
        }
    }
    loop {
        let Some(appid) = r.u32() else { break };
        if appid == 0 {
            break;
        }
        let Some(size) = r.u32() else { break };
        let end = r.at + size as usize;
        // State, update time, token, sha1, change number, binary sha1.
        let header = 4 + 4 + 8 + 20 + 4 + 20;
        let mut kv = Kv { b: b.get(..end.min(b.len())).unwrap_or(&[]), at: r.at + header };
        let mut info = Info::default();
        let _ = kv.walk(&mut Vec::new(), v29.then_some(table.as_slice()), &mut |path, v| {
            if let (Value::Str(s), [_, common, key]) = (v, path) {
                if common == "common" && key == "name" && info.name.is_none() {
                    info.name = Some(s.to_string());
                } else if common == "common" && key == "type" && info.kind.is_none() {
                    info.kind = Some(s.to_lowercase());
                } else if common == "config" && key == "installdir" && info.dir.is_none() {
                    info.dir = Some(s.to_string());
                }
            }
        });
        out.insert(appid, info);
        r.at = end;
    }
    out
}

/// Non-Steam games added to the library: prefix id → name.
fn shortcut_names(root: &Path) -> HashMap<u32, (String, Option<PathBuf>)> {
    let mut out = HashMap::new();
    let Ok(users) = std::fs::read_dir(root.join("userdata")) else { return out };
    for u in users.flatten() {
        let Ok(b) = std::fs::read(u.path().join("config/shortcuts.vdf")) else { continue };
        let mut kv = Kv { b: &b, at: 0 };
        let mut cur: HashMap<String, (Option<u32>, Option<String>, Option<PathBuf>)> = HashMap::new();
        let _ = kv.walk(&mut Vec::new(), None, &mut |path, v| {
            if path.len() < 3 {
                return;
            }
            let e = cur.entry(path[1].clone()).or_default();
            match (path[2].to_lowercase().as_str(), v) {
                ("appid", Value::Int(i)) => e.0 = Some(i),
                ("appname", Value::Str(s)) => e.1 = Some(s.to_string()),
                // Quoted, as Steam keeps it.
                ("exe", Value::Str(s)) => e.2 = Some(PathBuf::from(s.trim_matches('"'))),
                _ => {}
            }
        });
        for (id, name, exe) in cur.into_values() {
            if let (Some(id), Some(name)) = (id, name) {
                out.insert(id, (name, exe.filter(|e| e.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")))));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- the folders

/// Steam's own folders in steamapps/common.
const STEAM_OWN: &[&str] = &["Steam Controller Configs", "Steamworks Shared", "SteamLinuxRuntime", "SteamLinuxRuntime_soldier", "SteamLinuxRuntime_sniper"];

#[derive(Default)]
struct Cache {
    lists: HashMap<View, Vec<Item>>,
    /// Real folder → where it's listed (to go back up there, and its name).
    by_path: HashMap<PathBuf, (View, String)>,
    /// Game folder or prefix → the game's exe (its icon is the sharpest)
    /// and Steam's small icon.
    icons: HashMap<PathBuf, (Option<PathBuf>, Option<PathBuf>)>,
    built: bool,
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Cache::default()))
}

/// The small icon Steam keeps for an app.
fn icon_file(roots: &[PathBuf], appid: u32) -> Option<PathBuf> {
    for r in roots {
        let dir = r.join("appcache/librarycache").join(appid.to_string());
        // New layout: a folder per app, the icon is the file named by its hash.
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let icon = rd.flatten().map(|e| e.path()).find(|p| {
                let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                n.len() == 44 && n.ends_with(".jpg")
            });
            if icon.is_some() {
                return icon;
            }
        }
        let old = r.join("appcache/librarycache").join(format!("{appid}_icon.jpg"));
        if old.is_file() {
            return Some(old);
        }
    }
    None
}

/// Normalised for comparing names: letters and digits, lower case.
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The game's own exe in its folder: named like the game or its folder,
/// shallow, big; never an installer, crash reporter or redistributable.
pub fn main_exe(dir: &Path, name: &str) -> Option<PathBuf> {
    const SKIP_DIRS: &[&str] = &["_commonredist", "commonredist", "redist", "redistributables", "directx", "vcredist", "dotnet", "support", "installer", "__installer", "easyanticheat", "battleye", "engine"];
    const SKIP_NAMES: &[&str] = &["unins", "setup", "install", "crash", "redist", "vcredist", "dxsetup", "dotnet", "report", "helper", "eac", "battleye", "updater", "uninstall", "cefprocess", "webhelper", "ue4prereq", "prereq", "quicksfv", "touchup", "cleanup", "dxwebsetup"];
    let want = [squash(name), squash(&dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())];
    let mut best: Option<(i64, PathBuf)> = None;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut seen = 0;
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 4000 {
                break;
            }
            let p = e.path();
            let n = e.file_name().to_string_lossy().to_lowercase();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < 4 && !SKIP_DIRS.contains(&n.as_str()) {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            if !n.ends_with(".exe") || SKIP_NAMES.iter().any(|b| n.contains(b)) {
                continue;
            }
            let stem = squash(n.trim_end_matches(".exe").trim_end_matches("-win64-shipping").trim_end_matches("_x64"));
            let like = want.iter().filter(|w| !w.is_empty()).map(|w| {
                if stem == *w {
                    3
                } else if w.contains(&stem) && stem.len() >= 3 || stem.contains(w.as_str()) {
                    2
                } else {
                    0
                }
            });
            let like = like.max().unwrap_or(0);
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            // Likeness first, then shallow, then big (the game, not a tool).
            let score = like * 1_000_000_000 - depth as i64 * 100_000_000 + (size / 1024).min(99_999_999) as i64;
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Names unique within a list (a second "Foo" gets its id).
fn unique(items: &mut [Item]) {
    let mut count: HashMap<String, usize> = HashMap::new();
    for i in items.iter() {
        *count.entry(i.name.clone()).or_default() += 1;
    }
    for i in items.iter_mut() {
        if count[&i.name] > 1 {
            let tag = i.appid.map_or_else(|| i.path.display().to_string(), |a| a.to_string());
            i.name = format!("{} ({tag})", i.name);
        }
        // Names are folder entries: no slashes.
        i.name = i.name.replace('/', "∕");
    }
}

/// The accounts' folders (userdata/<id>) of every install.
fn userdata(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for r in roots {
        let Ok(rd) = std::fs::read_dir(r.join("userdata")) else { continue };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().parse::<u64>().is_ok() && e.path().is_dir() {
                out.push(e.path());
            }
        }
    }
    out
}

/// Games' screenshot folders (with something in them): app id, folder.
fn screenshot_dirs(users: &[PathBuf]) -> Vec<(u32, PathBuf)> {
    let mut out = Vec::new();
    for u in users {
        let Ok(rd) = std::fs::read_dir(u.join("760/remote")) else { continue };
        for e in rd.flatten() {
            let Ok(id) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
            let dir = e.path().join("screenshots");
            let any = std::fs::read_dir(&dir).is_ok_and(|mut r| r.any(|f| f.is_ok_and(|f| f.path().is_file())));
            if any {
                out.push((id, dir));
            }
        }
    }
    out
}

/// A game recording: one session.mpd.
#[derive(Clone, Debug)]
struct Rec {
    appid: u32,
    mpd: PathBuf,
    /// "2026-10-25 17:30:07".
    when: String,
    /// Saved as a clip (else the background recording).
    clip: bool,
}

/// Where an account's recordings are: gamerecordings, and a folder of its
/// own if one is set (a path under a "…record…" key of its settings).
fn recording_dirs(user: &Path) -> Vec<PathBuf> {
    let mut out = vec![user.join("gamerecordings")];
    if let Ok(t) = std::fs::read_to_string(user.join("config/localconfig.vdf")) {
        for (k, v) in t.lines().filter_map(kv_line) {
            let p = PathBuf::from(&v);
            if k.to_lowercase().contains("record") && p.is_absolute() && p.is_dir() && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// The app id and time from folders named like clip_<appid>_<date>_<time>
/// or bg_<appid>_<date>_<time> above a session.mpd (a clip's, else the
/// nearest), and whether it's in a clip's.
fn parse_rec(mpd: &Path, base: &Path) -> Option<Rec> {
    let mut found: Option<(u32, String)> = None;
    let mut clip = false;
    for dir in mpd.ancestors().skip(1).take_while(|d| d.starts_with(base) && *d != base) {
        let name = dir.file_name()?.to_string_lossy();
        let parts: Vec<&str> = name.split('_').collect();
        if parts.len() < 4 || !["clip", "bg", "timeline"].contains(&parts[0]) {
            continue;
        }
        let (Ok(id), d, t) = (parts[1].parse::<u32>(), parts[2], parts[3]) else { continue };
        // (A clip goes by its own time, not its recording's.)
        let is_clip = parts[0] == "clip";
        if is_clip && !clip {
            found = None;
        }
        clip |= is_clip;
        if found.is_none() && d.len() == 8 && t.len() >= 6 && d.bytes().chain(t.bytes()).all(|b| b.is_ascii_digit()) {
            found = Some((id, format!("{}-{}-{} {}:{}:{}", &d[..4], &d[4..6], &d[6..], &t[..2], &t[2..4], &t[4..6])));
        }
    }
    let (appid, when) = found?;
    Some(Rec { appid, mpd: mpd.to_path_buf(), when, clip })
}

fn recordings(users: &[PathBuf]) -> Vec<Rec> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_name() == "session.mpd" {
                out.push(p);
            } else if depth > 0 && e.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&p, depth - 1, out);
            }
        }
    }
    let mut out: Vec<Rec> = Vec::new();
    for u in users {
        for base in recording_dirs(u) {
            let mut mpds = Vec::new();
            walk(&base, 5, &mut mpds);
            for r in mpds.iter().filter_map(|m| parse_rec(m, &base)) {
                if !out.iter().any(|o| o.mpd == r.mpd) {
                    out.push(r);
                }
            }
        }
    }
    // Newest first.
    out.sort_by(|a, b| b.when.cmp(&a.when));
    out
}

/// What a recording's chunks take together.
fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()).unwrap_or(0)
}

fn build() -> Cache {
    let roots = roots();
    let libs = libraries();
    let mut games: Vec<Item> = Vec::new();
    let mut names: HashMap<u32, String> = HashMap::new();
    let info: HashMap<u32, Info> = roots.iter().flat_map(|r| appinfo(r)).collect();
    // Proton, runtimes and the like are installed too, but aren't games.
    let tool = |id: u32| info.get(&id).and_then(|i| i.kind.as_deref()).is_some_and(|k| k == "tool" || k == "config");
    let mut tool_dirs: Vec<PathBuf> = Vec::new();
    for l in &libs {
        let ms = manifests(l);
        for m in &ms {
            names.insert(m.appid, m.name.clone());
            if tool(m.appid) {
                tool_dirs.push(m.dir.clone());
                continue;
            }
            if m.dir.is_dir() {
                games.push(Item { name: m.name.clone(), path: m.dir.clone(), appid: Some(m.appid), size: None });
            }
        }
        // Folders in common without a manifest still show: named by the app
        // cache when it knows the folder, else by the folder.
        if let Ok(rd) = std::fs::read_dir(l.join("steamapps/common")) {
            for e in rd.flatten() {
                let p = e.path();
                let dir = e.file_name().to_string_lossy().into_owned();
                if !p.is_dir() || games.iter().any(|g| g.path == p) || tool_dirs.contains(&p) || STEAM_OWN.contains(&dir.as_str()) {
                    continue;
                }
                // Several apps may share a folder (a demo, a beta): the one named
                // like it, else a game, else the oldest.
                let known = info
                    .iter()
                    .filter(|(_, i)| i.dir.as_deref() == Some(dir.as_str()) && i.name.is_some())
                    .min_by_key(|(id, i)| (!i.name.as_deref().unwrap_or("").eq_ignore_ascii_case(&dir), i.kind.as_deref() != Some("game"), **id));
                if known.is_some_and(|(id, _)| tool(*id)) {
                    continue;
                }
                match known {
                    Some((id, i)) => games.push(Item { name: i.name.clone().unwrap_or(dir), path: p, appid: Some(*id), size: None }),
                    None => games.push(Item { name: dir, path: p, appid: None, size: None }),
                }
            }
        }
    }
    let mut prefixes: Vec<Item> = Vec::new();
    let mut ids: Vec<(u32, PathBuf)> = Vec::new();
    for l in &libs {
        let Ok(rd) = std::fs::read_dir(l.join("steamapps/compatdata")) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let (Ok(id), true) = (n.parse::<u32>(), e.path().is_dir()) {
                ids.push((id, e.path()));
            }
        }
    }
    // Prefixes of games no longer installed: the cache and shortcuts know.
    for (k, i) in &info {
        if let Some(n) = &i.name {
            names.entry(*k).or_insert_with(|| n.clone());
        }
    }
    let users = userdata(&roots);
    let shots = screenshot_dirs(&users);
    let recs = recordings(&users);
    let mut shortcut_exe: HashMap<u32, PathBuf> = HashMap::new();
    let unnamed = |id: &u32| !names.contains_key(id);
    if ids.iter().any(|(id, _)| unnamed(id)) || shots.iter().any(|(id, _)| unnamed(id)) || recs.iter().any(|r| unnamed(&r.appid)) {
        for r in &roots {
            for (k, (v, exe)) in shortcut_names(r) {
                names.entry(k).or_insert(v);
                if let Some(e) = exe {
                    shortcut_exe.insert(k, e);
                }
            }
        }
    }
    for (id, path) in ids {
        let name = match (id, names.get(&id)) {
            (_, Some(n)) => n.clone(),
            (0, None) => crate::i18n::tr("Shared prefix").to_string(),
            (_, None) => format!("{} {id}", crate::i18n::tr("Unknown game")),
        };
        prefixes.push(Item { name, path, appid: Some(id), size: None });
    }
    let game_name = |id: u32| names.get(&id).cloned().unwrap_or_else(|| format!("{} {id}", crate::i18n::tr("Unknown game")));
    let mut screenshots: Vec<Item> = shots.into_iter().map(|(id, path)| Item { name: game_name(id), path, appid: Some(id), size: None }).collect();
    screenshots.sort_by_key(|i| i.name.to_lowercase());
    // Recordings: the games that have some, then each one's.
    let mut recorded: Vec<Item> = Vec::new();
    let mut each: HashMap<u32, Vec<Item>> = HashMap::new();
    for r in recs {
        if !recorded.iter().any(|i| i.appid == Some(r.appid)) {
            recorded.push(Item { name: game_name(r.appid), path: PathBuf::new(), appid: Some(r.appid), size: None });
        }
        let name = if r.clip { format!("{} · {}", r.when, crate::i18n::tr("Clip")) } else { r.when.clone() };
        let size = r.mpd.parent().map_or(0, dir_size);
        each.entry(r.appid).or_default().push(Item { name, path: r.mpd, appid: Some(r.appid), size: Some(size) });
    }
    recorded.sort_by_key(|i| i.name.to_lowercase());
    unique(&mut games);
    unique(&mut prefixes);
    unique(&mut screenshots);
    unique(&mut recorded);
    for list in each.values_mut() {
        unique(list);
    }
    let mut c = Cache { built: true, ..Default::default() };
    // A game's exe gives the icon to its folder and its prefix.
    let mut exe_of: HashMap<u32, PathBuf> = shortcut_exe;
    for g in &games {
        if let Some(e) = main_exe(&g.path, &g.name) {
            if let Some(id) = g.appid {
                exe_of.insert(id, e.clone());
            }
            c.icons.insert(g.path.clone(), (Some(e), None));
        }
    }
    for (view, list) in [(View::Games, &games), (View::Prefixes, &prefixes), (View::Screenshots, &screenshots)] {
        for i in list {
            c.by_path.insert(i.path.clone(), (view, i.name.clone()));
            let exe = i.appid.and_then(|a| exe_of.get(&a).cloned());
            let img = i.appid.and_then(|a| icon_file(&roots, a));
            if exe.is_some() || img.is_some() {
                c.icons.insert(i.path.clone(), (exe, img));
            }
        }
    }
    c.lists.insert(View::Games, games);
    c.lists.insert(View::Prefixes, prefixes);
    c.lists.insert(View::Screenshots, screenshots);
    c.lists.insert(View::Recordings, recorded);
    for (id, list) in each {
        c.lists.insert(View::Recorded(id), list);
    }
    c
}

fn ensure() {
    let mut c = cache().lock().unwrap();
    if !c.built {
        *c = build();
    }
}

/// A Steam folder's name (a game's, for its recordings).
pub fn view_name(v: View) -> String {
    match v {
        View::Root => "Steam".into(),
        View::Games => crate::i18n::tr("Games").into(),
        View::Prefixes => crate::i18n::tr("Prefixes").into(),
        View::Screenshots => crate::i18n::tr("Screenshots").into(),
        View::Recordings => crate::i18n::tr("Recordings").into(),
        View::Recorded(id) => {
            ensure();
            let c = cache().lock().unwrap();
            c.lists.get(&View::Recordings).and_then(|l| l.iter().find(|i| i.appid == Some(id))).map_or_else(|| id.to_string(), |i| i.name.clone())
        }
    }
}

/// Where a Steam folder sits, and its name there.
pub fn parent(v: View) -> Option<(Location, String)> {
    match v {
        View::Root => None,
        View::Recorded(_) => Some((Location::Steam(View::Recordings), view_name(v))),
        _ => Some((Location::Steam(View::Root), view_name(v))),
    }
}

/// An entry of a Steam folder, opened.
pub fn join(v: View, name: &str) -> Location {
    match v {
        View::Root => Location::Steam(root_child(name).unwrap_or(View::Root)),
        View::Recordings => {
            ensure();
            let c = cache().lock().unwrap();
            let id = c.lists.get(&View::Recordings).and_then(|l| l.iter().find(|i| i.name == name)).and_then(|i| i.appid);
            id.map_or(Location::Steam(View::Recordings), |id| Location::Steam(View::Recorded(id)))
        }
        _ => Location::Local(resolve(v, name).unwrap_or_default()),
    }
}

/// The real file or folder behind an entry (none for what's only Polka's).
pub fn entry_path(v: View, name: &str) -> Option<PathBuf> {
    match v {
        View::Root | View::Recordings => None,
        _ => resolve(v, name),
    }
}

/// The folder's entries (looked up afresh each time).
pub fn list(v: View) -> std::io::Result<Vec<Entry>> {
    let dir = |name: &str, path: Option<&Path>| {
        let meta = path.and_then(|p| p.metadata().ok());
        Entry {
            name: name.to_string(),
            is_dir: true,
            is_link: false,
            size: 0,
            mtime: meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64),
            mode: 0o755,
            tags: Vec::new(),
        }
    };
    if v == View::Root {
        return Ok(ROOT_VIEWS.iter().map(|v| dir(&view_name(*v), None)).collect());
    }
    let fresh = build();
    let items = fresh.lists.get(&v).cloned().unwrap_or_default();
    *cache().lock().unwrap() = fresh;
    if roots().is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Steam"));
    }
    Ok(items
        .iter()
        .map(|i| match i.size {
            // A recording: a video to play.
            Some(size) => Entry { is_dir: false, size, mode: 0o644, ..dir(&i.name, Some(&i.path)) },
            None => dir(&i.name, (!i.path.as_os_str().is_empty()).then_some(i.path.as_path())),
        })
        .collect())
}

/// The real folder behind an entry.
pub fn resolve(v: View, name: &str) -> Option<PathBuf> {
    ensure();
    let c = cache().lock().unwrap();
    c.lists.get(&v)?.iter().find(|i| i.name == name).map(|i| i.path.clone())
}

/// A sub-folder of the root view by its shown name.
pub fn root_child(name: &str) -> Option<View> {
    ROOT_VIEWS.into_iter().find(|v| view_name(*v) == name)
}

/// The Steam folder's folders.
const ROOT_VIEWS: [View; 4] = [View::Games, View::Prefixes, View::Screenshots, View::Recordings];

/// For a game folder or prefix: the Steam folder it's listed in, and its
/// name there.
pub fn listed_in(p: &Path) -> Option<(View, String)> {
    let c = cache().lock().ok()?;
    if !c.built {
        return None;
    }
    c.by_path.get(p).cloned()
}

/// The game's exe and Steam icon for its folder or prefix.
pub fn icon(p: &Path) -> Option<(Option<PathBuf>, Option<PathBuf>)> {
    let c = cache().lock().ok()?;
    c.icons.get(p).cloned()
}

/// A recording's name for a file of its own: the game's and its own
/// ("Portal 2026-10-25 17-30-07").
pub fn recording_title(mpd: &Path) -> Option<String> {
    ensure();
    let c = cache().lock().ok()?;
    let (view, item) = c.lists.iter().find_map(|(v, l)| matches!(v, View::Recorded(_)).then(|| l.iter().find(|i| i.path == mpd).map(|i| (*v, i)))?)?;
    let View::Recorded(id) = view else { return None };
    let game = c.lists.get(&View::Recordings)?.iter().find(|i| i.appid == Some(id))?.name.clone();
    Some(format!("{game} {}", item.name).replace(':', "-"))
}

/// Is it a Steam recording (a DASH manifest)?
pub fn is_recording(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == "session.mpd")
}

/// Locations under a Steam view keep their way back to it.
pub fn parent_of(p: &Path) -> Option<(Location, String)> {
    listed_in(p).map(|(v, name)| (Location::Steam(v), name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_kv() {
        // { "shortcuts" { "0" { "appid" 7 "AppName" "Game" } } }
        let mut b = vec![0x00];
        b.extend(b"shortcuts\0");
        b.push(0x00);
        b.extend(b"0\0");
        b.push(0x02);
        b.extend(b"appid\0");
        b.extend(7u32.to_le_bytes());
        b.push(0x01);
        b.extend(b"AppName\0Game\0");
        b.extend([0x08, 0x08, 0x08]);
        let mut seen = Vec::new();
        let mut kv = Kv { b: &b, at: 0 };
        kv.walk(&mut Vec::new(), None, &mut |p, v| {
            seen.push((p.join("/"), match v {
                Value::Str(s) => s.to_string(),
                Value::Int(i) => i.to_string(),
            }))
        });
        assert_eq!(seen, vec![("shortcuts/0/appid".to_string(), "7".to_string()), ("shortcuts/0/AppName".to_string(), "Game".to_string())]);
    }

    #[test]
    fn recording_names() {
        let base = Path::new("/u/gamerecordings");
        let r = parse_rec(&base.join("clips/clip_2277560_20261025_173007/video/bg_2277560_20261025_172900/session.mpd"), base).unwrap();
        assert_eq!((r.appid, r.when.as_str(), r.clip), (2277560, "2026-10-25 17:30:07", true));
        let r = parse_rec(&base.join("video/bg_570_20261024_090102/session.mpd"), base).unwrap();
        assert_eq!((r.appid, r.when.as_str(), r.clip), (570, "2026-10-24 09:01:02", false));
        assert!(parse_rec(&base.join("video/other/session.mpd"), base).is_none());
    }

    #[test]
    fn text_kv() {
        assert_eq!(kv_line("\t\t\"path\"\t\t\"/run/media/x/SteamLibrary\""), Some(("path".into(), "/run/media/x/SteamLibrary".into())));
        assert_eq!(kv_line("\t{"), None);
    }
}
