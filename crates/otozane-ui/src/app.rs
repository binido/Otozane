use crate::client::{self, Connection, ServiceClient};
use crate::theme::{self, ThemeChoice};
use crate::widgets::{self, card, icon, icon_text, row, section};
use crate::{effects, icons};
use egui::{Frame, Margin, RichText, Ui};
use otozane_core::config::EffectConfig;
use otozane_core::protocol::{CaptureState, Request, Status};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Effects,
    Profiles,
    Settings,
}

impl Page {
    const ALL: [Page; 4] = [
        Page::Overview,
        Page::Effects,
        Page::Profiles,
        Page::Settings,
    ];
    fn label(self) -> (&'static str, &'static str) {
        match self {
            Page::Overview => (icons::ACTIVITY, "Overview"),
            Page::Effects => (icons::AUDIO_WAVEFORM, "Effects"),
            Page::Profiles => (icons::BOOKMARK, "Profiles"),
            Page::Settings => (icons::SETTINGS, "Settings"),
        }
    }
}

/// UI-only preferences (`ui.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub theme: ThemeChoice,
}

fn ui_settings_path() -> std::path::PathBuf {
    otozane_core::config::config_dir().join("ui.toml")
}

impl UiSettings {
    pub fn load() -> Self {
        std::fs::read_to_string(ui_settings_path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn save(&self) {
        let path = ui_settings_path();
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&path, toml::to_string(self).unwrap_or_default()));
        if let Err(e) = result {
            eprintln!("failed to save {}: {e}", path.display());
        }
    }
}

pub struct App {
    client: ServiceClient,
    page: Page,
    settings: UiSettings,
    status: Option<Status>,
    connection: Connection,
    profiles: Vec<String>,
    /// Local working copy of the chain; edits apply here immediately and are
    /// sent to the service, so sliders never wait for a round trip.
    pub chain: Vec<EffectConfig>,
    last_edit: Instant,
    levels: (f32, f32),
    last_frame: Instant,
    toasts: Vec<(String, Instant, bool)>,
    /// Effect cards the user collapsed; all others show their parameters.
    pub collapsed: std::collections::HashSet<u64>,
    pub plugins: otozane_core::protocol::PluginList,
    pub browser: Option<String>,
    new_profile: String,
    renaming: Option<(String, String)>,
    autostart: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut settings = UiSettings::load();
        match std::env::var("OTOZANE_THEME").as_deref() {
            Ok("dark") => settings.theme = ThemeChoice::Dark,
            Ok("light") => settings.theme = ThemeChoice::Light,
            _ => {}
        }
        theme::install(&cc.egui_ctx, settings.theme);
        Self {
            client: ServiceClient::spawn(cc.egui_ctx.clone()),
            page: match std::env::var("OTOZANE_PAGE").as_deref() {
                Ok("effects" | "plugins") => Page::Effects,
                Ok("profiles") => Page::Profiles,
                Ok("settings") => Page::Settings,
                _ => Page::Overview,
            },
            settings,
            status: None,
            connection: Connection::Connecting,
            profiles: Vec::new(),
            chain: Vec::new(),
            last_edit: Instant::now() - Duration::from_secs(10),
            levels: (0.0, 0.0),
            last_frame: Instant::now(),
            toasts: Vec::new(),
            collapsed: Default::default(),
            plugins: Default::default(),
            // Dev: OTOZANE_PAGE=plugins opens the plugin browser on the effects page.
            browser: (std::env::var("OTOZANE_PAGE").as_deref() == Ok("plugins")).then(String::new),
            new_profile: String::new(),
            renaming: None,
            autostart: client::autostart_enabled(),
        }
    }

    pub fn send(&mut self, req: Request) {
        self.client.send(req);
    }

    /// Marks a local edit so incoming status does not overwrite it mid-drag.
    pub fn edited(&mut self) {
        self.last_edit = Instant::now();
    }

    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    fn sync(&mut self) {
        let (status, errors) = {
            let mut s = self.client.state.lock().unwrap();
            self.connection = s.connection.clone();
            self.profiles = s.profiles.clone();
            self.plugins = s.plugins.clone();
            (s.status.clone(), std::mem::take(&mut s.messages))
        };
        let now = Instant::now();
        for (msg, is_error) in errors {
            self.toasts.push((msg, now, is_error));
        }
        self.toasts
            .retain(|(_, t, _)| t.elapsed() < Duration::from_secs(5));
        let dt = (now - self.last_frame).as_secs_f32().min(0.2);
        self.last_frame = now;
        // Peak meters with a 20 dB/s style decay.
        let decay = 10f32.powf(-dt);
        if let Some(st) = &status {
            self.levels.0 = st.input_peak.max(self.levels.0 * decay);
            self.levels.1 = st.output_peak.max(self.levels.1 * decay);
            if self.last_edit.elapsed() > Duration::from_millis(500) {
                self.chain = st.chain.clone();
            }
        } else {
            self.levels = (0.0, 0.0);
        }
        self.status = status;
    }

    fn nav(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(icon(icons::AUDIO_LINES, 20.0).color(theme::current(ui.ctx()).accent));
            ui.label(RichText::new("Otozane").heading().size(20.0));
        });
        ui.add_space(14.0);
        for page in Page::ALL {
            let (glyph, label) = page.label();
            let selected = self.page == page;
            let text = RichText::new(icon_text(glyph, label));
            let b = egui::Button::selectable(selected, text)
                .min_size(egui::vec2(ui.available_width(), 34.0));
            if ui.add(b).clicked() {
                self.page = page;
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(4.0);
            let p = theme::current(ui.ctx());
            let (color, text) = match (&self.connection, &self.status) {
                (Connection::Connected, Some(st)) => match &st.capture {
                    CaptureState::Running(_) => (p.success, "Processing"),
                    CaptureState::Error(_) => (p.error, "Audio error"),
                    _ => (p.warning, "Waiting for input"),
                },
                (Connection::Connecting, _) | (Connection::Connected, None) => {
                    (p.text_weak, "Connecting")
                }
                (Connection::Disconnected(_), _) => (p.error, "Service stopped"),
            };
            widgets::status(ui, color, text);
        });
    }

    fn disconnected(&mut self, ui: &mut Ui, reason: &str) {
        widgets::heading(ui, "Service not running");
        card(ui, |ui| {
            ui.label("The Otozane audio service processes your microphone in the background. Start it to use the virtual microphone.");
            ui.label(RichText::new(reason).small().weak());
            ui.add_space(6.0);
            if ui
                .button(icon_text(icons::POWER, "Start service"))
                .clicked()
                && let Err(e) = client::start_service()
            {
                self.toasts.push((
                    format!("Could not start otozaned: {e}"),
                    Instant::now(),
                    true,
                ));
            }
        });
    }

    fn overview(&mut self, ui: &mut Ui) {
        let Some(st) = self.status.clone() else {
            return;
        };
        let p = theme::current(ui.ctx());
        widgets::heading(ui, "Overview");
        card(ui, |ui| {
            let (color, title, detail) = match &st.capture {
                CaptureState::Running(d) => (
                    p.success,
                    "Processing".to_string(),
                    device_label(&st, Some(d)),
                ),
                CaptureState::Connecting(d) => (
                    p.warning,
                    "Connecting".to_string(),
                    device_label(&st, Some(d)),
                ),
                CaptureState::WaitingForDevice(d) => (
                    p.warning,
                    "Waiting for microphone".to_string(),
                    d.as_deref().map_or("No microphone found".to_string(), |d| {
                        format!("{d} is not connected")
                    }),
                ),
                CaptureState::Starting => (p.text_weak, "Starting".to_string(), String::new()),
                CaptureState::Error(e) => (p.error, "Audio error".to_string(), e.clone()),
            };
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 6.0, color);
                ui.label(
                    RichText::new(title)
                        .family(egui::FontFamily::Name(theme::MEDIUM.into()))
                        .size(17.0),
                );
            });
            if !detail.is_empty() {
                ui.label(RichText::new(detail).weak());
            }
            ui.add_space(4.0);
            let vmic = if st.virtual_source_ready {
                "\u{201c}Otozane Microphone\u{201d} is available to applications"
            } else {
                "Virtual microphone is not ready"
            };
            ui.label(RichText::new(icon_text(icons::MIC, vmic)).small().weak());
        });
        section(ui, "Levels");
        card(ui, |ui| {
            widgets::meter(ui, "Input", self.levels.0);
            widgets::meter(ui, "Output", self.levels.1);
        });
        section(ui, "Processing");
        card(ui, |ui| {
            row(
                ui,
                "Enable processing",
                Some("When off, the microphone passes through unchanged"),
                |ui| {
                    let mut on = st.processing_enabled;
                    if widgets::switch(ui, &mut on).changed() {
                        self.send(Request::SetProcessing { enabled: on });
                    }
                },
            );
            ui.separator();
            self.device_row(ui, &st);
            ui.separator();
            let profiles = self.profiles.clone();
            row(ui, "Profile", None, |ui| {
                egui::ComboBox::from_id_salt("profile")
                    .selected_text(&st.active_profile)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for name in &profiles {
                            if ui
                                .selectable_label(*name == st.active_profile, name)
                                .clicked()
                                && *name != st.active_profile
                            {
                                self.send(Request::LoadProfile { name: name.clone() });
                            }
                        }
                    });
            });
            if let Some(i) = self
                .chain
                .iter()
                .position(|e| e.kind == otozane_core::effects::EffectKind::Rnnoise)
            {
                ui.separator();
                row(ui, "Noise suppression", Some("RNNoise"), |ui| {
                    let mut on = self.chain[i].enabled;
                    if widgets::switch(ui, &mut on).changed() {
                        self.chain[i].enabled = on;
                        self.edited();
                        let id = self.chain[i].id;
                        self.send(Request::SetEffectEnabled { id, enabled: on });
                    }
                });
            }
        });
    }

    fn device_row(&mut self, ui: &mut Ui, st: &Status) {
        row(ui, "Microphone", None, |ui| {
            let selected = match &st.input_device {
                None => "Automatic".to_string(),
                Some(name) => device_label(st, Some(name)),
            };
            egui::ComboBox::from_id_salt("device")
                .selected_text(selected)
                .width(260.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(st.input_device.is_none(), "Automatic")
                        .clicked()
                    {
                        self.send(Request::SetInput { device: None });
                    }
                    for d in &st.devices {
                        let current = st.input_device.as_deref() == Some(&d.name);
                        let mut label = d.description.clone();
                        if d.is_virtual {
                            label.push_str("  (virtual)");
                        }
                        if ui
                            .selectable_label(current, label)
                            .on_hover_text(&d.name)
                            .clicked()
                        {
                            self.send(Request::SetInput {
                                device: Some(d.name.clone()),
                            });
                        }
                    }
                });
        });
    }

    fn profiles_page(&mut self, ui: &mut Ui) {
        let Some(active) = self.status.as_ref().map(|s| s.active_profile.clone()) else {
            return;
        };
        widgets::heading(ui, "Profiles");
        ui.label(RichText::new("A profile stores the effect chain with all parameters. Changes are saved to the active profile automatically.").weak());
        section(ui, "Saved profiles");
        card(ui, |ui| {
            let names = self.profiles.clone();
            for (i, name) in names.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                let is_active = *name == active;
                if let Some((from, to)) = &mut self.renaming
                    && from == name
                {
                    let mut commit = None;
                    ui.horizontal(|ui| {
                        let r = ui.text_edit_singleline(to);
                        if ui
                            .button(icon(icons::CHECK, 15.0))
                            .on_hover_text("Rename")
                            .clicked()
                            || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            commit = Some(true);
                        }
                        if ui
                            .button(icon(icons::X, 15.0))
                            .on_hover_text("Cancel")
                            .clicked()
                        {
                            commit = Some(false);
                        }
                    });
                    if let Some(ok) = commit {
                        let (from, to) = self.renaming.take().unwrap();
                        if ok && from != to {
                            self.send(Request::RenameProfile { from, to });
                        }
                    }
                    continue;
                }
                let subtitle = is_active.then_some("Active");
                row(ui, name, subtitle, |ui| {
                    if !is_active
                        && ui
                            .button(icon(icons::TRASH_2, 15.0))
                            .on_hover_text("Delete")
                            .clicked()
                    {
                        self.send(Request::DeleteProfile { name: name.clone() });
                    }
                    if ui
                        .button(icon(icons::PENCIL, 15.0))
                        .on_hover_text("Rename")
                        .clicked()
                    {
                        self.renaming = Some((name.clone(), name.clone()));
                    }
                    if ui.button("Export").clicked() {
                        self.send(Request::GetProfile { name: name.clone() });
                    }
                    if !is_active && ui.button("Load").clicked() {
                        self.send(Request::LoadProfile { name: name.clone() });
                    }
                });
            }
        });
        if ui
            .button(icon_text(icons::FOLDER_OPEN, "Import profile..."))
            .clicked()
        {
            self.client.import_profile();
        }
        section(ui, "Save current chain as a new profile");
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_profile)
                        .hint_text("Profile name")
                        .desired_width(260.0),
                );
                let name = self.new_profile.trim().to_string();
                if ui
                    .add_enabled(
                        !name.is_empty(),
                        egui::Button::new(icon_text(icons::SAVE, "Save")),
                    )
                    .clicked()
                {
                    self.send(Request::SaveProfileAs { name });
                    self.new_profile.clear();
                }
            });
        });
        section(ui, "Presets");
        card(ui, |ui| {
            row(
                ui,
                "Voice",
                Some("Gain, noise suppression with voice detection, gate, compressor, limiter"),
                |ui| {
                    if ui
                        .button(icon_text(icons::PLUS, "Create profile"))
                        .clicked()
                    {
                        // Keeps the current profile: switch to a new one, then fill it.
                        let name = unique_name(&self.profiles, "Voice");
                        self.send(Request::SaveProfileAs { name });
                        self.send(Request::SetChain {
                            chain: otozane_core::config::Profile::voice().chain,
                        });
                    }
                },
            );
        });
    }

    fn settings_page(&mut self, ui: &mut Ui) {
        widgets::heading(ui, "Settings");
        section(ui, "Appearance");
        card(ui, |ui| {
            row(ui, "Theme", None, |ui| {
                let mut choice = self.settings.theme;
                for (c, label) in [
                    (ThemeChoice::Dark, "Dark"),
                    (ThemeChoice::Light, "Light"),
                    (ThemeChoice::System, "System"),
                ] {
                    ui.selectable_value(&mut choice, c, label);
                }
                if choice != self.settings.theme {
                    self.settings.theme = choice;
                    theme::apply(ui.ctx(), choice);
                    self.settings.save();
                }
            });
        });
        section(ui, "Background operation");
        card(ui, |ui| {
            ui.label("Audio is processed by the otozaned service and keeps running when this window is closed.");
            ui.separator();
            row(
                ui,
                "Start with session",
                Some("Runs otozaned as a systemd user service when you log in"),
                |ui| {
                    let mut on = self.autostart;
                    if widgets::switch(ui, &mut on).changed() {
                        let running = self.connection == Connection::Connected;
                        match client::set_autostart(on, !running) {
                            Ok(()) => self.autostart = on,
                            Err(e) => self.toasts.push((
                                format!("Could not change autostart: {e}"),
                                Instant::now(),
                                true,
                            )),
                        }
                    }
                },
            );
            ui.separator();
            if self.status.is_some() {
                ui.separator();
                if ui.button(icon_text(icons::POWER, "Stop service")).clicked() {
                    self.send(Request::Shutdown);
                }
            }
        });
        let Some(st) = self.status.clone() else {
            return;
        };
        section(ui, "Audio");
        card(ui, |ui| {
            self.device_row(ui, &st);
            ui.separator();
            row(
                ui,
                "Format",
                Some("Set `channels` in config.toml and restart the service to change"),
                |ui| {
                    let ch = if st.diagnostics.channels == 2 {
                        "Stereo"
                    } else {
                        "Mono"
                    };
                    ui.label(format!("{} Hz, {ch}", st.diagnostics.sample_rate));
                },
            );
        });
        section(ui, "Diagnostics");
        card(ui, |ui| {
            let d = &st.diagnostics;
            let ms = |f: u32| f as f32 * 1000.0 / d.sample_rate.max(1) as f32;
            egui::Grid::new("diag")
                .num_columns(2)
                .spacing([24.0, 6.0])
                .show(ui, |ui| {
                    let mut kv = |k: &str, v: String| {
                        ui.label(RichText::new(k).weak());
                        ui.label(v);
                        ui.end_row();
                    };
                    kv("Service version", st.version.clone());
                    kv(
                        "Estimated latency",
                        format!("{:.1} ms", d.estimated_latency_ms()),
                    );
                    kv(
                        "Effect latency",
                        format!(
                            "{} frames ({:.1} ms)",
                            d.chain_latency_frames,
                            ms(d.chain_latency_frames)
                        ),
                    );
                    kv(
                        "Capture quantum",
                        format!(
                            "{} frames ({:.1} ms)",
                            d.capture_quantum,
                            ms(d.capture_quantum)
                        ),
                    );
                    kv("Output quantum", format!("{} frames", d.playback_quantum));
                    kv("Queued output", format!("{} frames", d.ring_fill_frames));
                    kv("Underruns", format!("{} frames", d.underrun_frames));
                    kv("Overruns", format!("{} frames", d.overrun_frames));
                    kv(
                        "Latency corrections",
                        format!("{} frames skipped", d.skipped_frames),
                    );
                    kv("Processed blocks", d.processed_blocks.to_string());
                });
        });
    }
}

/// `base`, or `base 2`, `base 3`... if taken.
fn unique_name(taken: &[String], base: &str) -> String {
    (1..)
        .map(|i| {
            if i == 1 {
                base.to_string()
            } else {
                format!("{base} {i}")
            }
        })
        .find(|n| !taken.contains(n))
        .unwrap()
}

fn device_label(st: &Status, name: Option<&str>) -> String {
    let Some(name) = name else {
        return String::new();
    };
    st.devices
        .iter()
        .find(|d| d.name == name)
        .map_or_else(|| name.to_string(), |d| d.description.clone())
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.sync();
        dev_screenshot(ui);
        let p = theme::current(ui.ctx());
        egui::Panel::left("nav")
            .exact_size(200.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.sidebar).inner_margin(Margin::same(12)))
            .show(ui, |ui| self.nav(ui));
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.window)
                    .inner_margin(Margin::symmetric(28, 20)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let width = ui.available_width().min(780.0);
                        ui.horizontal_top(|ui| {
                            ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                            ui.vertical(|ui| {
                                ui.set_width(width);
                                match self.connection.clone() {
                                    Connection::Disconnected(reason)
                                        if self.page != Page::Settings =>
                                    {
                                        self.disconnected(ui, &reason)
                                    }
                                    _ => match self.page {
                                        Page::Overview => self.overview(ui),
                                        Page::Effects => effects::page(self, ui),
                                        Page::Profiles => self.profiles_page(ui),
                                        Page::Settings => self.settings_page(ui),
                                    },
                                }
                            });
                        });
                    });
            });
        if !self.toasts.is_empty() {
            egui::Area::new(egui::Id::unique("toasts"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -16.0))
                .show(ui.ctx(), |ui| {
                    for (msg, _, is_error) in &self.toasts {
                        Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(if *is_error {
                                RichText::new(icon_text(icons::CIRCLE_ALERT, msg)).color(p.error)
                            } else {
                                RichText::new(icon_text(icons::CIRCLE_CHECK, msg))
                            });
                        });
                    }
                });
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    }
}

/// Development aid: with `OTOZANE_SCREENSHOT=<file>`, captures the window as
/// raw RGBA (`<file>`, size printed to stdout) after one second and exits.
fn dev_screenshot(ui: &mut Ui) {
    static REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let Some(path) = std::env::var_os("OTOZANE_SCREENSHOT") else {
        return;
    };
    let ctx = ui.ctx().clone();
    ctx.request_repaint();
    if ctx.input(|i| i.time) < 1.0 || REQUESTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let c = ctx.clone();
    ctx.request_screenshot(move |img| {
        let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
        if std::fs::write(&path, bytes).is_ok() {
            println!("{}x{}", img.size[0], img.size[1]);
        }
        c.send_viewport_cmd(egui::ViewportCommand::Close);
    });
}
