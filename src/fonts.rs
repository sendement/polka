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

pub fn install(ctx: &egui::Context) {
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
    ctx.set_fonts(fonts);
}
