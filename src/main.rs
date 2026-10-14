mod app;
mod appearance;
mod archive_rw;
mod config;
mod diagnose;
mod drives;
mod fonts;
mod i18n;
mod icons;
mod input;
mod keyboard;
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
    if std::env::args().nth(1).as_deref() == Some("--diagnose") {
        i18n::set(None);
        diagnose::run();
        return Ok(());
    }
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
            fonts::install(&cc.egui_ctx);
            appearance::watch(cc.egui_ctx.clone());
            let mut app = app::App::new(cc.egui_ctx.clone(), start);
            use raw_window_handle::HasDisplayHandle;
            app.sys_clip = Some(egui_winit::clipboard::Clipboard::new(cc.display_handle().ok().map(|h| h.as_raw())));
            Ok(Box::new(app))
        }),
    )
}
