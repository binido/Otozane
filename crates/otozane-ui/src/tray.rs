//! System tray icon (StatusNotifierItem): shows or hides the window, shows
//! the service state, switches the processing mode and quits.

use crate::client::Mode;
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{RadioGroup, RadioItem, StandardItem};
use otozane_core::protocol::{CaptureState, Client, Request, Response};
use std::sync::mpsc::Sender;
use std::time::Duration;

pub enum Cmd {
    Show,
    Quit,
}

pub struct Tray {
    tx: Sender<Cmd>,
    /// Context of the open window, `None` while it is hidden.
    pub window: Option<egui::Context>,
    state: &'static str,
    /// `None` while the service is not reachable.
    mode: Option<Mode>,
    theme_path: String,
}

impl Tray {
    fn toggle(&mut self) {
        match &self.window {
            Some(ctx) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            None => {
                let _ = self.tx.send(Cmd::Show);
            }
        }
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "otozane".into()
    }
    fn title(&self) -> String {
        "Otozane".into()
    }
    fn icon_theme_path(&self) -> String {
        self.theme_path.clone()
    }
    /// Symbolic, so the host recolours it for its panel.
    fn icon_name(&self) -> String {
        "otozane-symbolic".into()
    }
    /// Fallback for hosts that do not find the symbolic icon: white with a
    /// dark outline, so it stays visible on light and dark panels.
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/tray.png"))
            .expect("bundled tray icon is a valid PNG");
        let (w, h) = (icon.width as usize, icon.height as usize);
        vec![ksni::Icon {
            width: w as i32,
            height: h as i32,
            data: outlined(&icon.rgba, w, h),
        }]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Otozane".into(),
            description: self.state.into(),
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.toggle();
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            StandardItem {
                label: self.state.into(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            RadioGroup {
                selected: Mode::ALL
                    .iter()
                    .position(|(m, _)| Some(*m) == self.mode)
                    .unwrap_or(usize::MAX),
                select: Box::new(|this: &mut Self, i| {
                    let mode = Mode::ALL[i].0;
                    let from = this.mode.unwrap_or(mode);
                    this.mode = Some(mode);
                    // Off the menu thread: the socket round trip can block.
                    std::thread::spawn(move || {
                        if let Ok(mut c) = Client::connect() {
                            for req in mode.requests(from) {
                                let _ = c.request(&req);
                            }
                        }
                    });
                }),
                options: Mode::ALL
                    .iter()
                    .map(|(_, label)| RadioItem {
                        label: (*label).into(),
                        enabled: self.mode.is_some(),
                        ..Default::default()
                    })
                    .collect(),
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: if self.window.is_some() {
                    "Hide window"
                } else {
                    "Show window"
                }
                .into(),
                activate: Box::new(Self::toggle),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    if let Some(ctx) = &this.window {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    let _ = this.tx.send(Cmd::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Registers the tray icon and keeps its service state current. `wait`
/// registers even before a tray host is running (login races the panel).
/// `None` when there is no tray host.
pub fn spawn(tx: Sender<Cmd>, wait: bool) -> Option<Handle<Tray>> {
    let tray = Tray {
        tx,
        window: None,
        state: "Connecting",
        mode: None,
        theme_path: install_icon(),
    };
    let handle = match tray.assume_sni_available(wait).spawn() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("no system tray: {e}");
            return None;
        }
    };
    let h = handle.clone();
    std::thread::spawn(move || {
        let mut client = None;
        while !h.is_closed() {
            let (state, mode) = state(&mut client);
            h.update(|t| {
                t.state = state;
                t.mode = mode;
            });
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    Some(handle)
}

/// Turns a mask (alpha of RGBA) into white ARGB with a black outline
/// `R` pixels wide.
fn outlined(rgba: &[u8], w: usize, h: usize) -> Vec<u8> {
    const R: usize = 2;
    let alpha = |x: usize, y: usize| rgba[(y * w + x) * 4 + 3] as u32;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let a = alpha(x, y);
            let mut halo = 0;
            for ny in y.saturating_sub(R)..(y + R + 1).min(h) {
                for nx in x.saturating_sub(R)..(x + R + 1).min(w) {
                    halo = halo.max(alpha(nx, ny));
                }
            }
            // White over the black outline: a + halo * (1 - a).
            let out_a = a + halo * (255 - a) / 255;
            let white = (255 * a).checked_div(out_a).unwrap_or(0);
            out.extend([out_a as u8, white as u8, white as u8, white as u8]);
        }
    }
    out
}

/// Writes the symbolic icon into an icon theme under the cache dir and
/// returns that theme path, empty when it cannot be written.
fn install_icon() -> String {
    use std::path::PathBuf;
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")));
    let Some(root) = cache.map(|c| c.join("otozane/icons")) else {
        return String::new();
    };
    let dir = root.join("hicolor/scalable/apps");
    let svg = include_bytes!("../../../packaging/tray.svg");
    match std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join("otozane-symbolic.svg"), svg))
    {
        Ok(()) => root.to_string_lossy().into_owned(),
        Err(e) => {
            eprintln!("tray icon: {e}");
            String::new()
        }
    }
}

fn state(client: &mut Option<Client>) -> (&'static str, Option<Mode>) {
    if client.is_none() {
        *client = Client::connect().ok();
    }
    let resp = client.as_mut().map(|c| c.request(&Request::Status));
    match resp {
        Some(Ok(Response::Status(st))) => {
            let state = match st.capture {
                CaptureState::Running(_) if !st.processing_enabled => "Bypassed",
                CaptureState::Running(_) => "Processing",
                CaptureState::Muted => "Muted",
                CaptureState::Error(_) => "Audio error",
                _ => "Waiting for input",
            };
            (state, Some(Mode::of(&st)))
        }
        Some(Ok(_)) => ("Connecting", None),
        _ => {
            *client = None;
            ("Service stopped", None)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn outline_surrounds_white_pixel() {
        // 5x5 mask with one opaque pixel in the centre.
        let mut rgba = vec![0u8; 5 * 5 * 4];
        rgba[(2 * 5 + 2) * 4 + 3] = 255;
        let argb = super::outlined(&rgba, 5, 5);
        let px = |x: usize, y: usize| &argb[(y * 5 + x) * 4..][..4];
        assert_eq!(px(2, 2), [255, 255, 255, 255]); // the shape stays white
        assert_eq!(px(0, 0), [255, 0, 0, 0]); // two pixels out is black outline
        let mut rgba = vec![0u8; 7 * 7 * 4];
        rgba[(3 * 7 + 3) * 4 + 3] = 255;
        let argb = super::outlined(&rgba, 7, 7);
        assert_eq!(argb[0], 0); // three pixels out is transparent
    }
}
