//! `otozane`: desktop interface for the Otozane audio service.

mod app;
mod client;
mod effects;
#[allow(dead_code)] // generated table, not every icon is used yet
mod icons;
mod theme;
mod widgets;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Otozane")
            .with_app_id("otozane")
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Otozane",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
