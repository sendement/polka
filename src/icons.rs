//! Icon resolution: freedesktop icon theme lookups, `.desktop` Icon= keys and
//! icons extracted from Windows PE resources (.exe/.dll), all cached.
//! Results are `file://` URIs for egui's image loaders.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::vfs::Entry;

pub struct Icons {
    theme: String,
    /// icon name -> resolved uri (None = not found in any theme)
    names: HashMap<String, Option<String>>,
    /// file path -> uri for per-file icons (.desktop, .exe)
    files: Arc<Mutex<HashMap<PathBuf, Option<String>>>>,
    pending: Arc<Mutex<HashSet<PathBuf>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    thumbs: crate::thumbs::Thumbs,
    /// Thumbnail textures handed to egui, to free them in bulk.
    thumb_uris: HashSet<String>,
}

impl Icons {
    pub fn new(theme: Option<String>, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let w = wake.clone();
        Self {
            thumbs: crate::thumbs::Thumbs::new(move || w()),
            thumb_uris: HashSet::new(),
            theme: theme.or_else(detect_theme).unwrap_or_else(|| "hicolor".into()),
            names: HashMap::new(),
            files: Default::default(),
            pending: Default::default(),
            wake,
        }
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
        self.named_in(&theme, candidates, size)
    }

    /// Like `named`, but from any installed theme (theme picker preview).
    pub fn named_in(&mut self, theme: &str, candidates: &[&str], size: u16) -> Option<String> {
        candidates.iter().find_map(|c| self.lookup_in(theme, c, size))
    }

    /// Switch themes; None follows the desktop's setting.
    pub fn set_theme(&mut self, theme: Option<String>) {
        self.theme = theme.or_else(detect_theme).unwrap_or_else(|| "hicolor".into());
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
            .map(|p| uri(&p));
        self.names.insert(key, found.clone());
        found
    }

    /// Icon for a directory entry. `path` is the real file when local, which
    /// enables per-file icons.
    pub fn for_entry(&mut self, e: &Entry, path: Option<&Path>, size: u16) -> Option<String> {
        if e.is_dir {
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
    if let Ok(out) = std::process::Command::new("gsettings")
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
fn desktop_icon(path: &Path) -> Option<String> {
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
fn exe_icon(path: &Path) -> Option<PathBuf> {
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
