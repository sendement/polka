//! Steam in the sidebar: two folders, the installed games (from every
//! library) and the Proton prefixes, named after their games instead of the
//! app ids Steam gives their folders.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::vfs::{Entry, Location};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Root,
    Games,
    Prefixes,
}

/// A game folder or a prefix, under the name shown for it.
#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub path: PathBuf,
    pub appid: Option<u32>,
}

/// Steam installs found (native, ~/.steam links, Flatpak), each once.
pub fn roots() -> Vec<PathBuf> {
    // Tests bring a Steam of their own.
    if let Some(r) = std::env::var_os("POLKA_STEAM_ROOT") {
        return vec![PathBuf::from(r)];
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
fn shortcut_names(root: &Path) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let Ok(users) = std::fs::read_dir(root.join("userdata")) else { return out };
    for u in users.flatten() {
        let Ok(b) = std::fs::read(u.path().join("config/shortcuts.vdf")) else { continue };
        let mut kv = Kv { b: &b, at: 0 };
        let mut cur: HashMap<String, (Option<u32>, Option<String>)> = HashMap::new();
        let _ = kv.walk(&mut Vec::new(), None, &mut |path, v| {
            if path.len() < 3 {
                return;
            }
            let e = cur.entry(path[1].clone()).or_default();
            match (path[2].to_lowercase().as_str(), v) {
                ("appid", Value::Int(i)) => e.0 = Some(i),
                ("appname", Value::Str(s)) => e.1 = Some(s.to_string()),
                _ => {}
            }
        });
        for (id, name) in cur.into_values() {
            if let (Some(id), Some(name)) = (id, name) {
                out.insert(id, name);
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
    /// Game folder or prefix → its icon file.
    icons: HashMap<PathBuf, PathBuf>,
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
                games.push(Item { name: m.name.clone(), path: m.dir.clone(), appid: Some(m.appid) });
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
                    Some((id, i)) => games.push(Item { name: i.name.clone().unwrap_or(dir), path: p, appid: Some(*id) }),
                    None => games.push(Item { name: dir, path: p, appid: None }),
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
    if ids.iter().any(|(id, _)| !names.contains_key(id)) {
        for r in &roots {
            for (k, v) in shortcut_names(r) {
                names.entry(k).or_insert(v);
            }
        }
    }
    for (id, path) in ids {
        let name = match (id, names.get(&id)) {
            (_, Some(n)) => n.clone(),
            (0, None) => crate::i18n::tr("Shared prefix").to_string(),
            (_, None) => format!("{} {id}", crate::i18n::tr("Unknown game")),
        };
        prefixes.push(Item { name, path, appid: Some(id) });
    }
    unique(&mut games);
    unique(&mut prefixes);
    let mut c = Cache { built: true, ..Default::default() };
    for (view, list) in [(View::Games, &games), (View::Prefixes, &prefixes)] {
        for i in list {
            c.by_path.insert(i.path.clone(), (view, i.name.clone()));
            if let Some(icon) = i.appid.and_then(|a| icon_file(&roots, a)) {
                c.icons.insert(i.path.clone(), icon);
            }
        }
    }
    c.lists.insert(View::Games, games);
    c.lists.insert(View::Prefixes, prefixes);
    c
}

fn ensure() {
    let mut c = cache().lock().unwrap();
    if !c.built {
        *c = build();
    }
}

pub fn view_name(v: View) -> &'static str {
    match v {
        View::Root => "Steam",
        View::Games => crate::i18n::tr("Games"),
        View::Prefixes => crate::i18n::tr("Prefixes"),
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
        }
    };
    if v == View::Root {
        return Ok(vec![dir(view_name(View::Games), None), dir(view_name(View::Prefixes), None)]);
    }
    let fresh = build();
    let items = fresh.lists.get(&v).cloned().unwrap_or_default();
    *cache().lock().unwrap() = fresh;
    if roots().is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Steam"));
    }
    Ok(items.iter().map(|i| dir(&i.name, Some(&i.path))).collect())
}

/// The real folder behind an entry.
pub fn resolve(v: View, name: &str) -> Option<PathBuf> {
    ensure();
    let c = cache().lock().unwrap();
    c.lists.get(&v)?.iter().find(|i| i.name == name).map(|i| i.path.clone())
}

/// A sub-folder of the root view by its shown name.
pub fn root_child(name: &str) -> Option<View> {
    [View::Games, View::Prefixes].into_iter().find(|v| view_name(*v) == name)
}

/// For a game folder or prefix: the Steam folder it's listed in, and its
/// name there.
pub fn listed_in(p: &Path) -> Option<(View, String)> {
    let c = cache().lock().ok()?;
    if !c.built {
        return None;
    }
    c.by_path.get(p).cloned()
}

/// The game's icon for its folder or prefix.
pub fn icon(p: &Path) -> Option<PathBuf> {
    let c = cache().lock().ok()?;
    c.icons.get(p).cloned()
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
    fn text_kv() {
        assert_eq!(kv_line("\t\t\"path\"\t\t\"/run/media/x/SteamLibrary\""), Some(("path".into(), "/run/media/x/SteamLibrary".into())));
        assert_eq!(kv_line("\t{"), None);
    }
}
