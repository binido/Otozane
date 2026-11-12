//! `otozane`: desktop interface for the Otozane audio service.
//!
//! With a system tray, closing the window hides it to the tray. `--hidden`
//! starts in the tray (used by the login autostart entry).

mod app;
mod client;
mod effects;
#[allow(dead_code)] // generated table, not every icon is used yet
mod icons;
mod theme;
mod tray;
mod widgets;

/// The app icon (`packaging/otozane.svg` rendered at 256 px).
fn icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/otozane.png"))
        .expect("bundled icon is a valid PNG")
}

fn window(tray: Option<&ksni::blocking::Handle<tray::Tray>>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Otozane")
            .with_app_id("otozane")
            .with_icon(icon())
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };
    let has_tray = tray.is_some();
    let tray_cc = tray.cloned();
    let result = eframe::run_native(
        "Otozane",
        options,
        Box::new(move |cc| {
            if let Some(t) = tray_cc {
                let ctx = cc.egui_ctx.clone();
                t.update(|t| t.window = Some(ctx));
            }
            Ok(Box::new(app::App::new(cc, has_tray)))
        }),
    );
    if let Some(t) = tray {
        t.update(|t| t.window = None);
    }
    result
}

fn main() -> eframe::Result {
    let hidden = std::env::args().any(|a| a == "--hidden");
    let (tx, rx) = std::sync::mpsc::channel();
    let tray = tray::spawn(tx, hidden);
    let Some(tray) = tray else {
        return window(None);
    };
    if !hidden {
        window(Some(&tray))?;
    }
    // The window event loop has ended (or never started): wait in the tray.
    while let Ok(tray::Cmd::Show) = rx.recv() {
        window(Some(&tray))?;
    }
    Ok(())
}
