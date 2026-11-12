//! System tray icon (StatusNotifierItem): shows or hides the window, shows
//! the service state and quits.

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;
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
    fn icon_name(&self) -> String {
        "audio-input-microphone".into()
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
            let state = state(&mut client);
            h.update(|t| t.state = state);
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    Some(handle)
}

fn state(client: &mut Option<Client>) -> &'static str {
    if client.is_none() {
        *client = Client::connect().ok();
    }
    let resp = client.as_mut().map(|c| c.request(&Request::Status));
    match resp {
        Some(Ok(Response::Status(st))) => match st.capture {
            CaptureState::Running(_) => "Processing",
            CaptureState::Error(_) => "Audio error",
            _ => "Waiting for input",
        },
        Some(Ok(_)) => "Connecting",
        _ => {
            *client = None;
            "Service stopped"
        }
    }
}
