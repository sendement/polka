mod app;
mod appearance;
mod archive_rw;
mod config;
mod diagnose;
mod dnd;
mod drives;
mod fonts;
mod i18n;
mod icons;
mod input;
mod keyboard;
mod log;
mod launch;
mod locks;
mod mouse;
mod ops;
mod sandbox;
mod sidebar;
mod steam;
mod terminal;
mod thumbs;
mod trashbin;
mod usage;
mod vfs;
mod volumes;
mod xcursor;
mod view;
mod viewer;
#[cfg(test)]
mod view_tests;

fn main() -> eframe::Result {
    sandbox::fix_env();
    archive_rw::utf8_locale();
    // Lists an archive the way the panels see it (for bug reports).
    if std::env::args().nth(1).as_deref() == Some("--list") {
        for a in std::env::args_os().skip(2) {
            match vfs::archive_index(std::path::Path::new(&a)) {
                Ok(idx) => {
                    let mut all: Vec<_> = idx.dirs.iter().flat_map(|(d, es)| es.iter().map(move |e| if d.is_empty() { e.name.clone() } else { format!("{d}/{}", e.name) })).collect();
                    all.sort();
                    println!("{}: {} entries", std::path::Path::new(&a).display(), all.len());
                    for e in all {
                        println!("  {e}");
                    }
                }
                Err(e) => println!("{}: error: {e}", std::path::Path::new(&a).display()),
            }
        }
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--diagnose") {
        i18n::set(None);
        diagnose::run();
        return Ok(());
    }
    log::catch_panics();
    let _ = input::LOG.set(|s| log::line(s));
    log::startup();
    let start = std::env::args_os().nth(1).map(|a| app::arg_path(&a));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Polka")
            // The window matches its .desktop file (and icon) by this.
            .with_app_id(std::env::var("FLATPAK_ID").unwrap_or_else(|_| "polka".into()))
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([520.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Polka",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            if let Some(gl) = cc.gl.as_ref() {
                use eframe::glow::HasContext;
                let (r, v) = unsafe { (gl.get_parameter_string(eframe::glow::RENDERER), gl.get_parameter_string(eframe::glow::VERSION)) };
                log::line(format!("renderer {r} / {v}"));
            }
            fonts::install(&cc.egui_ctx);
            appearance::watch(cc.egui_ctx.clone());
            let mut app = app::App::new(cc.egui_ctx.clone(), start);
            use raw_window_handle::HasDisplayHandle;
            app.sys_clip = Some(egui_winit::clipboard::Clipboard::new(cc.display_handle().ok().map(|h| h.as_raw())));
            // Drag and drop with other programs: Wayland only (X11 drops come
            // through winit).
            use raw_window_handle::{HasWindowHandle, RawDisplayHandle, RawWindowHandle};
            if let (Ok(d), Ok(w)) = (cc.display_handle(), cc.window_handle()) {
                if let (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(w)) = (d.as_raw(), w.as_raw()) {
                    app.dnd = dnd::Dnd::new(d.display.as_ptr(), w.surface.as_ptr(), cc.egui_ctx.clone());
                    log::line(format!("drag and drop with other programs: {}", if app.dnd.is_some() { "on" } else { "unavailable" }));
                }
            }
            Ok(Box::new(app))
        }),
    )
}
