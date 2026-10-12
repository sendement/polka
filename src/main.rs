mod app;
mod archive_rw;
mod config;
mod fonts;
mod i18n;
mod icons;
mod input;
mod launch;
mod ops;
mod thumbs;
mod vfs;
mod view;
#[cfg(test)]
mod view_tests;

fn main() -> eframe::Result {
    let start = std::env::args_os().nth(1).map(|a| app::arg_path(&a));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Polka")
            .with_app_id("polka")
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
            Ok(Box::new(app::App::new(cc.egui_ctx.clone(), start)))
        }),
    )
}
