//! `otozane`: desktop interface for the Otozane audio service.
//!
//! With a system tray, closing the window hides it to the tray. `--hidden`
//! starts in the tray (used by the login autostart entry).

mod app;
mod chrome;
mod client;
#[cfg(target_os = "macos")]
mod driver;
mod effects;
#[allow(dead_code)] // generated table, not every icon is used yet
mod icons;
#[cfg(target_os = "macos")]
mod launchd;
#[cfg(target_os = "macos")]
mod menubar;
mod theme;
#[cfg(target_os = "linux")]
mod tray;
mod widgets;

/// The app icon (`packaging/otozane.svg` rendered at 256 px).
pub fn icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/otozane.png"))
        .expect("bundled icon is a valid PNG")
}

/// A tray icon variant: black on transparent, recoloured by the host.
pub struct TrayIcon {
    /// Symbolic icon name on Linux.
    pub name: &'static str,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub svg: &'static [u8],
    pub png: &'static [u8],
}

// PNGs: `rsvg-convert packaging/<name>.svg -o crates/otozane-ui/assets/<name>.png`
pub const TRAY_ICONS: [TrayIcon; 3] = [
    TrayIcon {
        name: "otozane-symbolic",
        svg: include_bytes!("../../../packaging/tray.svg"),
        png: include_bytes!("../assets/tray.png"),
    },
    TrayIcon {
        name: "otozane-bypass-symbolic",
        svg: include_bytes!("../../../packaging/tray-bypass.svg"),
        png: include_bytes!("../assets/tray-bypass.png"),
    },
    TrayIcon {
        name: "otozane-off-symbolic",
        svg: include_bytes!("../../../packaging/tray-off.svg"),
        png: include_bytes!("../assets/tray-off.png"),
    },
];

/// Tray icon for a state label: crossed out when no processed audio comes
/// out, dimmed while processing is bypassed.
pub fn tray_icon(state: &str) -> &'static TrayIcon {
    match state {
        "Muted" | "Audio error" | "Service stopped" => &TRAY_ICONS[2],
        "Bypassed" => &TRAY_ICONS[1],
        _ => &TRAY_ICONS[0],
    }
}

fn options() -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Otozane")
            .with_app_id("otozane")
            .with_icon(icon())
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };
    options.viewport = if cfg!(target_os = "macos") {
        // System frame for rounded corners and the traffic lights, content under the hidden title bar.
        options
            .viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
    } else {
        // Title bar and window buttons are drawn by the app (see chrome.rs).
        options.viewport.with_decorations(false)
    };
    // Screenshots get a fixed size; tiling compositors (niri) float such windows.
    // OTOZANE_SCREENSHOT_SIZE=WxH overrides it, e.g. to check the minimum width.
    if std::env::var_os("OTOZANE_SCREENSHOT").is_some() {
        let size = std::env::var("OTOZANE_SCREENSHOT_SIZE")
            .ok()
            .and_then(|s| parse_size(&s))
            .unwrap_or([1100.0, 720.0]);
        options.viewport = options
            .viewport
            .with_inner_size(size)
            .with_min_inner_size(size)
            .with_max_inner_size(size)
            .with_resizable(false);
    }
    options
}

/// Parses `WxH` into a window size. Returns None unless both sides are finite and positive.
fn parse_size(s: &str) -> Option<[f32; 2]> {
    let (w, h) = s.split_once('x')?;
    let side = |v: &str| {
        v.trim()
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && *v > 0.0)
    };
    Some([side(w)?, side(h)?])
}

#[cfg(target_os = "macos")]
fn main() -> eframe::Result {
    eframe::run_native(
        "Otozane",
        options(),
        Box::new(|cc| {
            let has_menubar = menubar::spawn()
                .map_err(|e| eprintln!("no menu bar icon: {e}"))
                .is_ok();
            Ok(Box::new(app::App::new(cc, has_menubar)))
        }),
    )
}

#[cfg(target_os = "linux")]
fn window(tray: Option<&ksni::blocking::Handle<tray::Tray>>) -> eframe::Result {
    let has_tray = tray.is_some();
    let tray_cc = tray.cloned();
    let result = eframe::run_native(
        "Otozane",
        options(),
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

#[cfg(target_os = "linux")]
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

#[cfg(test)]
mod tests {
    use super::parse_size;

    #[test]
    fn screenshot_size_needs_positive_finite_sides() {
        assert_eq!(parse_size("720x480"), Some([720.0, 480.0]));
        for bad in [
            "0x480", "-5x480", "720x0", "720x-1", "infx480", "NaNx480", "720", "x",
        ] {
            assert_eq!(parse_size(bad), None, "{bad}");
        }
    }
}
