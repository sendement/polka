//! Use the system UI font (via fontconfig) plus a symbol fallback, so the UI
//! matches the desktop and arrows/⇧/⌫ render. egui's bundled fonts stay as
//! the last fallback.

use std::sync::Arc;

fn fc_match(pattern: &str) -> Option<Vec<u8>> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let lower = path.to_lowercase();
    // egui can't index into font collections.
    if !(lower.ends_with(".ttf") || lower.ends_with(".otf")) {
        return None;
    }
    std::fs::read(path.trim()).ok()
}

/// A font for `pattern` that may live in a collection (.ttc): bytes and
/// the face's index in it.
fn fc_match_face(pattern: &str) -> Option<(Vec<u8>, u32)> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}\t%{index}", pattern]).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let (path, index) = text.split_once('\t')?;
    let lower = path.to_lowercase();
    if !(lower.ends_with(".ttf") || lower.ends_with(".otf") || lower.ends_with(".ttc") || lower.ends_with(".otc")) {
        return None;
    }
    Some((std::fs::read(path.trim()).ok()?, index.trim().parse().unwrap_or(0)))
}

/// Installed monospace families with a usable file (TTF/OTF), sorted.
pub fn mono_families() -> &'static [(String, std::path::PathBuf)] {
    static F: std::sync::OnceLock<Vec<(String, std::path::PathBuf)>> = std::sync::OnceLock::new();
    F.get_or_init(|| {
        let out = std::process::Command::new("fc-list")
            .args([":spacing=mono", "-f", "%{family[0]}\t%{style[0]}\t%{file}\n"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let mut best: std::collections::BTreeMap<String, (u8, std::path::PathBuf)> = Default::default();
        for line in out.lines() {
            let mut it = line.splitn(3, '\t');
            let (Some(fam), Some(style), Some(file)) = (it.next(), it.next(), it.next()) else { continue };
            let lower = file.to_lowercase();
            if !(lower.ends_with(".ttf") || lower.ends_with(".otf")) {
                continue;
            }
            // Prefer the upright regular face.
            let rank = match style.to_lowercase().as_str() {
                "regular" | "book" | "normal" | "roman" => 0,
                s if s.contains("italic") || s.contains("oblique") || s.contains("bold") => 3,
                s if s.contains("medium") => 1,
                _ => 2,
            };
            let e = best.entry(fam.to_string()).or_insert((u8::MAX, file.into()));
            if rank < e.0 {
                *e = (rank, file.into());
            }
        }
        best.into_iter().map(|(f, (_, p))| (f, p)).collect()
    })
}

pub fn install(ctx: &egui::Context) {
    install_with(ctx, None);
}

/// Fonts, with `viewer` (a monospace family) for the text viewer.
pub fn install_with(ctx: &egui::Context, viewer: Option<&str>) {
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |name: &str, pattern: &str, family: egui::FontFamily, at: usize| {
        if let Some(bytes) = fc_match(pattern) {
            fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
            let list = fonts.families.entry(family).or_default();
            list.insert(at.min(list.len()), name.into());
        }
    };
    add("system-sans", "sans-serif", egui::FontFamily::Proportional, 0);
    add("system-symbols", "sans-serif:charset=21e7 232b 25c9 2192 2195", egui::FontFamily::Proportional, 1);
    add("system-mono", "monospace", egui::FontFamily::Monospace, 0);
    // Bold for Markdown: the bold face of the UI font, then the usual chain.
    add("system-sans-bold", "sans-serif:bold", egui::FontFamily::Name("bold".into()), 0);
    // Chinese, Japanese and Korean (the interface, and file names in any
    // language) from the system's CJK font, as the last fallback.
    let cjk = match crate::i18n::current() {
        "ja" => "sans-serif:lang=ja",
        "ko" => "sans-serif:lang=ko",
        _ => "sans-serif:lang=zh-cn",
    };
    if let Some((bytes, index)) = fc_match_face(cjk) {
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        fonts.font_data.insert("system-cjk".into(), Arc::new(data));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().push("system-cjk".into());
        fonts.families.entry(egui::FontFamily::Monospace).or_default().push("system-cjk".into());
    }
    // Devanagari (Hindi, and file names in it), after the rest.
    if let Some((bytes, index)) = fc_match_face("sans-serif:lang=hi") {
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        fonts.font_data.insert("system-deva".into(), Arc::new(data));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().push("system-deva".into());
        fonts.families.entry(egui::FontFamily::Monospace).or_default().push("system-deva".into());
    }
    let prop = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.entry(egui::FontFamily::Name("bold".into())).or_default().extend(prop);
    // The viewer's family: the chosen font, then the usual monospace chain.
    let mut chain: Vec<String> = Vec::new();
    if let Some(path) = viewer.and_then(|v| mono_families().iter().find(|(f, _)| f == v)).map(|(_, p)| p) {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert("viewer-font".into(), Arc::new(egui::FontData::from_owned(bytes)));
            chain.push("viewer-font".into());
        }
    }
    chain.extend(fonts.families.get(&egui::FontFamily::Monospace).cloned().unwrap_or_default());
    // Symbols and Cyrillic from the UI font, if the monospace one lacks them.
    chain.extend(fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(egui::FontFamily::Name("viewer".into()), chain);
    ctx.set_fonts(fonts);
}
