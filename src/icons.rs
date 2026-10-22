//! Icon resolution: freedesktop icon theme lookups, `.desktop` Icon= keys and
//! icons extracted from Windows PE resources (.exe/.dll), all cached.
//! Results are `file://` URIs for egui's image loaders.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::vfs::Entry;

pub struct Icons {
    /// The theme in use: the chosen one, or its light/dark sibling.
    theme: String,
    /// The theme as chosen (or detected).
    chosen: String,
    dark: bool,
    /// The UI's text colour, given to KDE's monochrome icons (as Plasma
    /// does), which are otherwise dark grey on any background.
    text: egui::Color32,
    /// icon name -> resolved uri (None = not found in any theme)
    names: HashMap<String, Option<String>>,
    /// file path -> uri for per-file icons (.desktop, .exe)
    files: Arc<Mutex<HashMap<PathBuf, Option<String>>>>,
    pending: Arc<Mutex<HashSet<PathBuf>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    thumbs: crate::thumbs::Thumbs,
    /// Thumbnail textures handed to egui, to free them in bulk.
    thumb_uris: HashSet<String>,
    /// Folders in this colour variant (folder-<c>-*), where the theme has it.
    pub folder_color: Option<String>,
    /// Folder colours each theme comes in.
    colors: HashMap<String, Vec<String>>,
    /// The theme's own empty / half-full / full trash icons, per theme.
    trash: Option<(String, Option<[String; 3]>)>,
}

impl Icons {
    pub fn new(theme: Option<String>, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let w = wake.clone();
        let mut icons = Self {
            thumbs: crate::thumbs::Thumbs::new(move || w()),
            thumb_uris: HashSet::new(),
            trash: None,
            folder_color: None,
            colors: HashMap::new(),
            theme: String::new(),
            chosen: theme.or_else(detect_theme).unwrap_or_else(|| "hicolor".into()),
            dark: true,
            text: egui::Color32::from_gray(220),
            names: HashMap::new(),
            files: Default::default(),
            pending: Default::default(),
            wake,
        };
        icons.theme = icons.chosen.clone();
        icons
    }

    /// Thumbnail (uri, width/height) for a real file, if one is ready.
    pub fn thumb(&mut self, path: &Path, mtime: i64) -> Option<(String, f32)> {
        let t = self.thumbs.get(path, mtime)?;
        self.thumb_uris.insert(t.0.clone());
        Some(t)
    }

    /// Drop thumbnail textures once many have piled up (big photo folders);
    /// the visible ones simply load again from the disk cache.
    pub fn trim(&mut self, ctx: &egui::Context) {
        if self.thumb_uris.len() > 1500 {
            for u in self.thumb_uris.drain() {
                ctx.forget_image(&u);
            }
        }
    }

    /// First icon from `candidates` that the theme has.
    pub fn named(&mut self, candidates: &[&str], size: u16) -> Option<String> {
        let theme = self.theme.clone();
        let color = self.folder_color.clone();
        self.named_colored(&theme, candidates, size, color.as_deref())
    }

    /// As the names say, without the folder colour (the icon picker shows
    /// each variant as it is).
    pub fn named_plain(&mut self, candidates: &[&str], size: u16) -> Option<String> {
        let theme = self.theme.clone();
        self.named_in(&theme, candidates, size)
    }

    /// Like `named`, but from any installed theme (theme picker preview).
    pub fn named_in(&mut self, theme: &str, candidates: &[&str], size: u16) -> Option<String> {
        candidates.iter().find_map(|c| self.lookup_in(theme, c, size))
    }

    /// Folder icons in `color` where the theme draws them, else as named.
    pub fn named_colored(&mut self, theme: &str, candidates: &[&str], size: u16, color: Option<&str>) -> Option<String> {
        candidates.iter().find_map(|c| {
            color.and_then(|col| tinted(c, col)).and_then(|t| self.lookup_in(theme, &t, size)).or_else(|| self.lookup_in(theme, c, size))
        })
    }

    /// The folder colours `theme` (and what it inherits) comes in.
    pub fn folder_colors(&mut self, theme: &str) -> Vec<String> {
        if let Some(c) = self.colors.get(theme) {
            return c.clone();
        }
        let c: Vec<String> = place_icons_in(theme).into_iter().filter_map(|g| g.color).collect();
        self.colors.insert(theme.to_string(), c.clone());
        c
    }

    /// Icon for UI chrome (toolbar buttons). Tries `name` and
    /// `name-symbolic`; monochrome icons (GTK symbolic, KDE ColorScheme-Text)
    /// are recoloured to `color` so they read on any background.
    pub fn ui_icon(&mut self, candidates: &[&str], size: u16, color: egui::Color32) -> Option<String> {
        let theme = self.theme.clone();
        self.ui_icon_in(&theme, candidates, size, color)
    }

    /// The same from a given theme (and what it inherits).
    pub fn ui_icon_in(&mut self, theme: &str, candidates: &[&str], size: u16, color: egui::Color32) -> Option<String> {
        let key = format!("ui:{theme}/{}@{size}#{:?}", candidates.join(","), color.to_array());
        if let Some(v) = self.names.get(&key) {
            return v.clone();
        }
        let found = candidates.iter().find_map(|c| {
            [c.to_string(), format!("{c}-symbolic")].into_iter().find_map(|n| {
                freedesktop_icons::lookup(&n).with_theme(theme).with_size(size).with_cache().find()
            })
        });
        let uri = found.map(|p| recolored(&p, color).unwrap_or_else(|| uri(&p)));
        self.names.insert(key, uri.clone());
        uri
    }

    /// Folder-ish icons the current theme (and the themes it inherits)
    /// ships in its "places" context, grouped by colour variant. Papirus has
    /// folder-blue-*, folder-red-*…; themes without variants give one group.
    pub fn place_icons(&self) -> Vec<IconGroup> {
        place_icons_in(&self.theme)
    }

    /// A trash icon for how full it is (0 empty, 1 half, 2 full), when the
    /// theme (or one it inherits) draws all three — gnome-stock-trash-empty,
    /// gnome-stock-trash, gnome-stock-trash-full; else None, and
    /// user-trash(-full) serve.
    pub fn trash_level(&mut self, level: usize) -> Option<String> {
        if self.trash.as_ref().is_none_or(|(t, _)| *t != self.theme) {
            let mut names = HashMap::new();
            let mut queue = vec![self.theme.clone()];
            let mut seen = HashSet::new();
            while let Some(t) = queue.pop() {
                if t == "hicolor" || !seen.insert(t.clone()) {
                    continue;
                }
                for dir in theme_dirs(&t) {
                    collect_trash(&dir, 0, &mut names);
                    if let Ok(idx) = std::fs::read_to_string(dir.join("index.theme")) {
                        if let Some(inh) = idx.lines().find_map(|l| l.strip_prefix("Inherits=")) {
                            queue.extend(inh.split(',').map(|s| s.trim().to_string()));
                        }
                    }
                }
            }
            self.trash = Some((self.theme.clone(), trash_set(&names)));
        }
        self.trash.as_ref().and_then(|(_, set)| set.as_ref()).map(|set| set[level.min(2)].clone())
    }

    /// Switch themes; None follows the desktop's setting.
    pub fn set_theme(&mut self, theme: Option<String>) {
        self.chosen = theme.or_else(detect_theme).unwrap_or_else(|| "hicolor".into());
        self.theme = variant(&self.chosen, self.dark);
    }

    /// Light or dark look: themes that come in both (Papirus-Dark /
    /// Papirus, breeze-dark / breeze) switch to the matching one.
    pub fn set_dark(&mut self, dark: bool) {
        self.dark = dark;
        self.theme = variant(&self.chosen, dark);
    }

    fn lookup(&mut self, name: &str, size: u16) -> Option<String> {
        let theme = self.theme.clone();
        self.lookup_in(&theme, name, size)
    }

    fn lookup_in(&mut self, theme: &str, name: &str, size: u16) -> Option<String> {
        let key = format!("{theme}/{name}@{size}");
        if let Some(v) = self.names.get(&key) {
            return v.clone();
        }
        let found = freedesktop_icons::lookup(name)
            .with_theme(theme)
            .with_size(size)
            .with_cache()
            .find()
            .or_else(|| {
                // Some themes only ship symbolic variants for UI names.
                freedesktop_icons::lookup(name).with_size(size).with_cache().find()
            })
            .map(|p| recolored(&p, self.text).unwrap_or_else(|| uri(&p)));
        self.names.insert(key, found.clone());
        found
    }

    /// The text colour for monochrome icons; a change looks them up again.
    pub fn set_text_color(&mut self, c: egui::Color32) {
        if self.text != c {
            self.text = c;
            self.names.clear();
        }
    }

    /// Icon for a directory entry. `path` is the real file when local, which
    /// enables per-file icons.
    pub fn for_entry(&mut self, e: &Entry, path: Option<&Path>, size: u16) -> Option<String> {
        if e.is_dir {
            // A Steam game or prefix wears the game's icon.
            // The exe's icon is the sharp one; Steam's is a small picture.
            if let Some((exe, img)) = path.and_then(crate::steam::icon) {
                if let Some(u) = exe.and_then(|e| self.file_icon(&e, size)) {
                    return Some(u);
                }
                if let Some(i) = img {
                    return Some(uri(&i));
                }
            }
            let special = path.and_then(special_folder_icon);
            let mut c: Vec<&str> = special.into_iter().collect();
            c.extend(["folder", "inode-directory"]);
            return self.named(&c, size);
        }
        let lower = e.name.to_lowercase();
        if let Some(p) = path {
            if lower.ends_with(".desktop") || lower.ends_with(".exe") || lower.ends_with(".dll") {
                if let Some(u) = self.file_icon(p, size) {
                    return Some(u);
                }
            }
        }
        let mime = mime_guess::from_path(&e.name).first_raw().map(str::to_string);
        let mut owned: Vec<String> = Vec::new();
        if let Some(m) = &mime {
            owned.push(m.replace('/', "-"));
        }
        if crate::vfs::is_archive_name(&e.name) {
            if lower.contains(".tar.") || lower.ends_with(".tgz") || lower.ends_with(".tzst") || lower.ends_with(".txz") {
                owned.push("application-x-compressed-tar".into());
            }
            owned.push("application-x-archive".into());
            owned.push("package-x-generic".into());
        }
        if lower.ends_with(".exe") || lower.ends_with(".msi") {
            owned.push("application-x-ms-dos-executable".into());
            owned.push("application-x-msdownload".into());
            owned.push("wine".into());
        }
        if lower.ends_with(".appimage") {
            owned.push("application-x-appimage".into());
        }
        if e.executable() {
            owned.push("application-x-executable".into());
            owned.push("application-x-shellscript".into());
        }
        if let Some(m) = &mime {
            let top = m.split('/').next().unwrap_or("");
            owned.push(format!("{top}-x-generic"));
        }
        owned.push("text-x-generic".into());
        owned.push("unknown".into());
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        self.named(&refs, size)
    }

    /// Per-file icon, resolved in the background. Returns None until ready.
    fn file_icon(&mut self, path: &Path, size: u16) -> Option<String> {
        let known = self.files.lock().unwrap().get(path).cloned();
        if let Some(v) = known {
            return match v {
                // .desktop entries store an icon *name*; resolve it with the theme.
                Some(s) if !s.starts_with("file://") => self.lookup(&s, size),
                other => other,
            };
        }
        if !self.pending.lock().unwrap().insert(path.to_path_buf()) {
            return None;
        }
        let path = path.to_path_buf();
        let files = self.files.clone();
        let pending = self.pending.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let lower = path.to_string_lossy().to_lowercase();
            let r = if lower.ends_with(".desktop") {
                desktop_icon(&path)
            } else {
                exe_icon(&path).map(|p| uri(&p))
            };
            files.lock().unwrap().insert(path.clone(), r);
            pending.lock().unwrap().remove(&path);
            wake();
        });
        None
    }
}

pub fn uri(p: &Path) -> String {
    format!("file://{}", p.display())
}

fn special_folder_icon(p: &Path) -> Option<&'static str> {
    let home = dirs::home_dir()?;
    if p == home {
        return Some("user-home");
    }
    let pairs = [
        (dirs::desktop_dir(), "user-desktop"),
        (dirs::document_dir(), "folder-documents"),
        (dirs::download_dir(), "folder-download"),
        (dirs::audio_dir(), "folder-music"),
        (dirs::picture_dir(), "folder-pictures"),
        (dirs::video_dir(), "folder-videos"),
        (dirs::template_dir(), "folder-templates"),
        (dirs::public_dir(), "folder-publicshare"),
    ];
    pairs.into_iter().find(|(d, _)| d.as_deref() == Some(p)).map(|(_, i)| i)
}

/// GTK keeps these semantic colours in symbolic icons (warning, error,
/// success); everything else is the foreground.
const SEMANTIC: [&str; 8] = ["f57900", "ff7800", "cc0000", "e01b24", "c01c28", "73d216", "33d17a", "4e9a06"];

/// A copy of a monochrome SVG icon in `color`, cached on disk. None if the
/// icon isn't monochrome (or isn't an SVG), so it's used as is.
fn recolored(path: &Path, color: egui::Color32) -> Option<String> {
    use std::hash::{Hash, Hasher};
    if path.extension()? != "svg" {
        return None;
    }
    let svg = std::fs::read_to_string(path).ok()?;
    let hex = format!("{:02x}{:02x}{:02x}", color.r(), color.g(), color.b());
    let symbolic = path.file_stem()?.to_string_lossy().ends_with("-symbolic");
    let out = if symbolic {
        replace_hex_colors(&svg, &hex)
    } else if let Some(i) = svg.find(".ColorScheme-Text") {
        // KDE: only the text colour in the stylesheet is the foreground.
        let rule_end = i + svg[i..].find('}')?;
        let rule = &svg[i..rule_end];
        let fixed = replace_hex_colors(rule, &hex);
        format!("{}{}{}", &svg[..i], fixed, &svg[rule_end..])
    } else {
        return None;
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    hex.hash(&mut h);
    let dir = dirs::cache_dir()?.join("polka/ui-icons");
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join(format!("{:016x}.svg", h.finish()));
    if !file.exists() {
        std::fs::write(&file, out).ok()?;
    }
    Some(uri(&file))
}

/// Replace `#rgb` / `#rrggbb` colours (not ids like url(#a1)) with `hex`.
fn replace_hex_colors(s: &str, hex: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'#' {
            let n = b[i + 1..].iter().take_while(|c| c.is_ascii_alphanumeric()).count();
            let digits = &s[i + 1..i + 1 + n];
            let prev = if i > 0 { b[i - 1] } else { b' ' };
            let is_color = (n == 6 || n == 3) && digits.bytes().all(|c| c.is_ascii_hexdigit()) && prev != b'(';
            if is_color && !SEMANTIC.contains(&digits.to_lowercase().as_str()) {
                out.push('#');
                out.push_str(hex);
                i += 1 + n;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod recolor_tests {
    #[test]
    fn replaces_colors_not_ids() {
        let s = r##"<path fill="#bebebe" style="stroke:#222"/><use href="url(#abc)"/><path fill="#cc0000"/>"##;
        let r = super::replace_hex_colors(s, "ffffff");
        assert_eq!(r, r##"<path fill="#ffffff" style="stroke:#ffffff"/><use href="url(#abc)"/><path fill="#cc0000"/>"##);
    }
}

/// Icons of one colour variant; `color` None = the plain ones.
pub struct IconGroup {
    pub color: Option<String>,
    pub names: Vec<String>,
}

fn icon_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(d) = dirs::data_dir() {
        roots.push(d.join("icons"));
    }
    if let Some(h) = dirs::home_dir() {
        roots.push(h.join(".icons"));
    }
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    roots.extend(sys.split(':').filter(|s| !s.is_empty()).map(|s| Path::new(s).join("icons")));
    roots
}

/// The installed sibling of `theme` that suits a light or dark look.
fn variant(theme: &str, dark: bool) -> String {
    let lower = theme.to_lowercase();
    let cands: Vec<String> = if dark {
        match lower.strip_suffix("-light") {
            Some(_) => vec![theme[..theme.len() - 6].to_string(), format!("{}-Dark", &theme[..theme.len() - 6])],
            None => Vec::new(),
        }
    } else {
        match lower.strip_suffix("-dark") {
            Some(_) => vec![theme[..theme.len() - 5].to_string(), format!("{}-Light", &theme[..theme.len() - 5])],
            None => Vec::new(),
        }
    };
    cands.into_iter().find(|c| !theme_dirs(c).is_empty()).unwrap_or_else(|| theme.to_string())
}

fn theme_dirs(theme: &str) -> Vec<PathBuf> {
    icon_roots().into_iter().map(|r| r.join(theme)).filter(|d| d.is_dir()).collect()
}

/// Icon names under any `places` directory (theme/64x64/places,
/// theme/places/64, theme/scalable/places…).
fn collect_places(dir: &Path, depth: usize, in_places: bool, out: &mut std::collections::BTreeSet<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if depth < 3 {
                collect_places(&p, depth + 1, in_places || name == "places", out);
            }
        } else if in_places && p.exists() {
            // (exists() follows links: a theme's broken ones show nothing.)
            if let Some(stem) = name.strip_suffix(".svg").or_else(|| name.strip_suffix(".png")) {
                if !stem.ends_with("-symbolic") && !stem.contains("-open") && !stem.contains("drag-accept") {
                    out.insert(stem.to_string());
                }
            }
        }
    }
}


/// Folder-ish icons a theme ships in its "places" context, grouped by
/// colour variant.
fn place_icons_in(theme: &str) -> Vec<IconGroup> {
    let mut names = std::collections::BTreeSet::new();
    let mut queue = vec![theme.to_string()];
    let mut seen = HashSet::new();
    while let Some(t) = queue.pop() {
        if t == "hicolor" || !seen.insert(t.clone()) {
            continue;
        }
        for dir in theme_dirs(&t) {
            collect_places(&dir, 0, false, &mut names);
            if let Ok(idx) = std::fs::read_to_string(dir.join("index.theme")) {
                if let Some(inh) = idx.lines().find_map(|l| l.strip_prefix("Inherits=")) {
                    queue.extend(inh.split(',').map(|s| s.trim().to_string()));
                }
            }
        }
    }
    group_by_color(names.into_iter().collect())
}

/// A folder icon name in a colour variant: folder → folder-<c>,
/// folder-documents → folder-<c>-documents, user-home → user-<c>-home
/// (Papirus' naming).
fn tinted(name: &str, c: &str) -> Option<String> {
    match name {
        "folder" | "inode-directory" => Some(format!("folder-{c}")),
        _ => {
            if let Some(rest) = name.strip_prefix("folder-") {
                Some(format!("folder-{c}-{rest}"))
            } else if name.starts_with("user-home") || name.starts_with("user-desktop") {
                Some(format!("user-{c}-{}", &name[5..]))
            } else {
                None
            }
        }
    }
}

/// Trash icon names anywhere in a theme folder, with the files they are
/// (links followed: themes often point all of them at one picture).
fn collect_trash(dir: &Path, depth: usize, out: &mut HashMap<String, HashSet<PathBuf>>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if depth < 3 {
                collect_trash(&p, depth + 1, out);
            }
        } else if TRASH_FAMILIES.iter().any(|f| name.starts_with(f)) {
            let stem = name.strip_suffix(".svg").or_else(|| name.strip_suffix(".png"));
            if let (Some(stem), Ok(real)) = (stem, p.canonicalize()) {
                out.entry(stem.to_string()).or_default().insert(real);
            }
        }
    }
}

/// Old GNOME names that come as empty, half-full (the bare name) and full.
const TRASH_FAMILIES: [&str; 2] = ["gnome-stock-trash", "gnome-trash"];

/// Empty, half-full and full trash icons, when a family has all three and
/// the half-full one is a picture of its own.
fn trash_set(names: &HashMap<String, HashSet<PathBuf>>) -> Option<[String; 3]> {
    TRASH_FAMILIES.iter().find_map(|f| {
        let empty = names.get_key_value(&format!("{f}-empty"))?;
        let full = names.get_key_value(&format!("{f}-full"))?;
        let half = [f.to_string(), format!("{f}-half"), format!("{f}-half-full")].into_iter().find_map(|n| names.get_key_value(&n))?;
        half.1.is_disjoint(empty.1).then_some(())?;
        half.1.is_disjoint(full.1).then_some(())?;
        Some([empty.0.clone(), half.0.clone(), full.0.clone()])
    })
}

/// Split `folder-<colour>-<kind>` variants into groups. A segment counts as a
/// colour when `folder-<c>` exists and at least five `folder-<c>-*` do.
fn group_by_color(names: Vec<String>) -> Vec<IconGroup> {
    let set: HashSet<&str> = names.iter().map(String::as_str).collect();
    let mut colors: Vec<String> = Vec::new();
    for n in &names {
        let Some(c) = n.strip_prefix("folder-") else { continue };
        if c.contains('-') || colors.iter().any(|x| x == c) {
            continue;
        }
        let prefix = format!("folder-{c}-");
        if names.iter().filter(|m| m.starts_with(&prefix)).count() >= 5 && set.contains(n.as_str()) {
            colors.push(c.to_string());
        }
    }
    let color_of = |n: &str| -> Option<usize> {
        colors.iter().position(|c| n == format!("folder-{c}") || n.starts_with(&format!("folder-{c}-")))
    };
    let mut plain = Vec::new();
    let mut groups: Vec<Vec<String>> = vec![Vec::new(); colors.len()];
    for n in &names {
        match color_of(n) {
            Some(i) => groups[i].push(n.clone()),
            None => plain.push(n.clone()),
        }
    }
    // Folders first in each group.
    let order = |v: &mut Vec<String>| v.sort_by_key(|n| (!n.starts_with("folder"), n.clone()));
    order(&mut plain);
    let mut out = vec![IconGroup { color: None, names: plain }];
    for (c, mut g) in colors.into_iter().zip(groups) {
        order(&mut g);
        out.push(IconGroup { color: Some(c), names: g });
    }
    out
}

#[cfg(test)]
mod group_tests {
    #[test]
    fn colour_variants_are_grouped() {
        let mut names: Vec<String> = ["folder", "folder-documents", "user-home", "folder-red"].map(String::from).to_vec();
        for k in ["documents", "music", "pictures", "videos", "download"] {
            names.push(format!("folder-red-{k}"));
        }
        names.push("folder-documents-x".into()); // not a colour: no bare "folder-documents-..." group of 5
        names.sort();
        let g = super::group_by_color(names);
        assert_eq!(g.len(), 2);
        assert_eq!(g[1].color.as_deref(), Some("red"));
        assert_eq!(g[1].names.len(), 6);
        assert!(g[0].names.contains(&"folder-documents".to_string()));
        assert!(g[0].names.contains(&"user-home".to_string()));
        assert_eq!(g[0].names[0], "folder");
    }
}

pub struct ThemeInfo {
    /// Directory name, which is what lookups use.
    pub id: String,
    pub name: String,
}

/// Installed icon themes (not cursor-only ones), user dirs first.
pub fn installed_themes() -> Vec<ThemeInfo> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(d) = dirs::data_dir() {
        roots.push(d.join("icons"));
    }
    if let Some(h) = dirs::home_dir() {
        roots.push(h.join(".icons"));
    }
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    roots.extend(sys.split(':').filter(|s| !s.is_empty()).map(|s| Path::new(s).join("icons")));

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            if id == "hicolor" || id == "default" || seen.contains(&id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path().join("index.theme")) else { continue };
            let mut name = None;
            let mut has_dirs = false;
            let mut hidden = false;
            let mut in_main = false;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_main = line == "[Icon Theme]";
                    continue;
                }
                let Some((k, v)) = line.split_once('=').filter(|_| in_main) else { continue };
                match k.trim() {
                    "Name" => name = Some(v.trim().to_string()),
                    "Directories" => has_dirs = v.split(',').any(|d| !d.trim().is_empty()),
                    "Hidden" => hidden = v.trim() == "true",
                    _ => {}
                }
            }
            // Cursor themes have no icon directories. GNOME marks Adwaita
            // hidden, but it's a complete theme people do pick.
            if !has_dirs || (hidden && id != "Adwaita") {
                continue;
            }
            seen.insert(id.clone());
            out.push(ThemeInfo { name: name.filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone()), id });
        }
    }
    out.sort_by_key(|t| t.name.to_lowercase());
    out
}

/// Icon theme from gsettings, then GTK/KDE config files.
pub fn detect_theme() -> Option<String> {
    // In a Flatpak gsettings reads the sandbox's own (empty) settings; the
    // settings portal has the desktop's.
    if crate::sandbox::flatpak() {
        let out = std::process::Command::new("gdbus")
            .args(["call", "--session", "--dest", "org.freedesktop.portal.Desktop", "--object-path", "/org/freedesktop/portal/desktop"])
            .args(["--method", "org.freedesktop.portal.Settings.ReadOne", "org.gnome.desktop.interface", "icon-theme"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        // (<<'Papirus-Dark'>>,)
        if let Some(name) = out.split('\'').nth(1).filter(|n| !n.is_empty()) {
            return Some(name.to_string());
        }
    } else if let Ok(out) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output()
    {
        let s = String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_string();
        if out.status.success() && !s.is_empty() {
            return Some(s);
        }
    }
    let cfg = dirs::config_dir()?;
    for (file, key) in [
        ("gtk-3.0/settings.ini", "gtk-icon-theme-name"),
        ("gtk-4.0/settings.ini", "gtk-icon-theme-name"),
        ("kdeglobals", "Theme"),
    ] {
        if let Ok(text) = std::fs::read_to_string(cfg.join(file)) {
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    if k.trim() == key && !v.trim().is_empty() {
                        return Some(v.trim().to_string());
                    }
                }
            }
        }
    }
    None
}

/// Icon= of a .desktop file: absolute path -> uri, otherwise the bare name.
pub fn desktop_icon(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_main = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if in_main {
            if let Some(v) = line.strip_prefix("Icon=") {
                let v = v.trim();
                if v.starts_with('/') {
                    return Path::new(v).exists().then(|| uri(Path::new(v)));
                }
                return (!v.is_empty()).then(|| v.to_string());
            }
        }
    }
    None
}

/// Extract the largest icon from a PE file into the cache dir, as PNG.
pub fn exe_icon(path: &Path) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(path).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok().hash(&mut h);
    let cache = dirs::cache_dir()?.join("polka/exe-icons");
    let out = cache.join(format!("{:016x}.png", h.finish()));
    let miss = cache.join(format!("{:016x}.none", h.finish()));
    if out.exists() {
        return Some(out);
    }
    if miss.exists() || meta.len() > 1 << 30 {
        return None;
    }
    std::fs::create_dir_all(&cache).ok()?;
    let r = extract_pe_icon(path).and_then(|img| img.save(&out).ok().map(|_| out.clone()));
    if r.is_none() {
        let _ = std::fs::write(&miss, b"");
    }
    r
}

fn extract_pe_icon(path: &Path) -> Option<image::DynamicImage> {
    let map = pelite::FileMap::open(path).ok()?;
    let pe = pelite::PeFile::from_bytes(&map).ok()?;
    let res = pe.resources().ok()?;
    let (_, group) = res.icons().flatten().next()?;
    // Best first: biggest, then deepest colour. 0 in the header means 256.
    let mut entries: Vec<_> = group.entries().iter().collect();
    let dim = |w: u8| if w == 0 { 256 } else { w as u32 };
    entries.sort_by_key(|e| std::cmp::Reverse((dim(e.bWidth), e.wBitCount)));
    entries.into_iter().find_map(|e| decode_icon_image(group.image(e.nId).ok()?))
}

/// One icon image from a PE resource: either a PNG or a headerless DIB.
fn decode_icon_image(data: &[u8]) -> Option<image::DynamicImage> {
    if data.starts_with(b"\x89PNG") {
        // Not via the ICO decoder: it rejects non-RGBA PNGs.
        return image::load_from_memory_with_format(data, image::ImageFormat::Png).ok();
    }
    // Wrap the DIB in a one-entry .ico so the ICO decoder handles masks/bpp.
    if data.len() < 16 {
        return None;
    }
    let w = u32::from_le_bytes(data[4..8].try_into().ok()?);
    let h = u32::from_le_bytes(data[8..12].try_into().ok()?) / 2;
    let bpp = u16::from_le_bytes(data[14..16].try_into().ok()?);
    let mut ico = Vec::with_capacity(22 + data.len());
    ico.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    ico.push(if w >= 256 { 0 } else { w as u8 });
    ico.push(if h >= 256 { 0 } else { h as u8 });
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&bpp.to_le_bytes());
    ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(data);
    image::load_from_memory_with_format(&ico, image::ImageFormat::Ico).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn trash_families() {
        let set = |a: &[(&str, &str)]| a.iter().map(|(n, f)| (n.to_string(), HashSet::from([PathBuf::from(f)]))).collect::<HashMap<_, _>>();
        use std::collections::{HashMap, HashSet};
        use std::path::PathBuf;
        let own = set(&[("gnome-stock-trash", "half.svg"), ("gnome-stock-trash-empty", "e.svg"), ("gnome-stock-trash-full", "f.svg")]);
        assert_eq!(super::trash_set(&own), Some(["gnome-stock-trash-empty".into(), "gnome-stock-trash".into(), "gnome-stock-trash-full".into()]));
        // Papirus and Win11 here: the bare name is the empty bin again.
        let linked = set(&[("gnome-stock-trash", "user-trash.svg"), ("gnome-stock-trash-empty", "user-trash.svg"), ("gnome-stock-trash-full", "user-trash-full.svg")]);
        assert_eq!(super::trash_set(&linked), None);
    }

    #[test]
    #[ignore]
    fn exe_icon_extracts() {
        // Colon-separated list of .exe paths.
        let list = std::env::var("POLKA_TEST_EXE").expect("POLKA_TEST_EXE");
        for p in list.split(':') {
            let img = super::extract_pe_icon(std::path::Path::new(p));
            let img = img.unwrap_or_else(|| panic!("no icon in {p}"));
            eprintln!("{p}: {}x{}", img.width(), img.height());
        }
    }
}

#[cfg(test)]
mod picker_probe {
    /// `cargo test picker_probe -- --ignored --nocapture`: names the icon
    /// picker would show blank, per installed theme.
    #[test]
    #[ignore]
    fn blanks() {
        for t in super::installed_themes() {
            let mut icons = super::Icons::new(Some(t.id.clone()), || {});
            let groups = icons.place_icons();
            let all: Vec<String> = groups.iter().flat_map(|g| g.names.clone()).collect();
            let missing: Vec<&String> = all.iter().filter(|n| {
                match icons.named(&[n.as_str()], 64) {
                    None => true,
                    Some(u) => !std::path::Path::new(u.trim_start_matches("file://")).exists(),
                }
            }).collect();
            println!("{:<28} {:>5} names, {:>4} blank  {:?}", t.id, all.len(), missing.len(), missing.iter().take(4).collect::<Vec<_>>());
        }
    }
}
