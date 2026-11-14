use crate::client::{self, Connection, ServiceClient};
use crate::theme::{self, ThemeChoice};
use crate::widgets::{self, card, icon, icon_text, row, section};
use crate::{chrome, effects, icons};
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
    /// Peak hold per meter: highest recent level and when it was reached.
    holds: [(f32, Instant); 2],
    last_frame: Instant,
    toasts: Vec<(String, Instant, bool)>,
    /// Effect cards the user collapsed; all others show their parameters.
    pub collapsed: std::collections::HashSet<u64>,
    pub plugins: otozane_core::protocol::PluginList,
    pub browser: Option<String>,
    new_profile: String,
    renaming: Option<(String, String)>,
    autostart: bool,
    has_tray: bool,
    login_window: bool,
    logo: Option<(bool, egui::TextureHandle)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, has_tray: bool) -> Self {
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
            holds: [(0.0, Instant::now()); 2],
            last_frame: Instant::now(),
            toasts: Vec::new(),
            collapsed: Default::default(),
            plugins: Default::default(),
            // Dev: OTOZANE_PAGE=plugins opens the plugin browser on the effects page.
            browser: (std::env::var("OTOZANE_PAGE").as_deref() == Ok("plugins")).then(String::new),
            new_profile: String::new(),
            renaming: None,
            autostart: client::autostart_enabled(),
            has_tray,
            login_window: client::login_window_enabled(),
            logo: None,
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
            for (hold, level) in self.holds.iter_mut().zip([self.levels.0, self.levels.1]) {
                *hold = peak_hold(*hold, level, now, decay);
            }
            if self.last_edit.elapsed() > Duration::from_millis(500) {
                self.chain = st.chain.clone();
            }
        } else {
            self.levels = (0.0, 0.0);
            self.holds = [(0.0, now); 2];
        }
        self.status = status;
    }

    /// Current state as (color, short title, detail line).
    fn state(&self, p: &theme::Palette) -> (egui::Color32, &'static str, String) {
        match (&self.connection, &self.status) {
            (Connection::Connected, Some(st)) => match &st.capture {
                CaptureState::Muted => (p.text_weak, "Muted", "Apps hear silence".to_string()),
                CaptureState::Running(d) if !st.processing_enabled => {
                    (p.text_weak, "Bypassed", device_label(st, Some(d)))
                }
                CaptureState::Running(d) => (p.accent, "Processing", device_label(st, Some(d))),
                CaptureState::Connecting(d) => {
                    (p.text_weak, "Connecting", device_label(st, Some(d)))
                }
                CaptureState::WaitingForDevice(d) => (
                    p.text_weak,
                    "Waiting for microphone",
                    d.as_deref().map_or("No microphone found".to_string(), |d| {
                        format!("{d} is not connected")
                    }),
                ),
                CaptureState::Starting => (p.text_weak, "Starting", String::new()),
                CaptureState::Error(e) => (p.error, "Audio error", e.clone()),
            },
            (Connection::Connecting, _) | (Connection::Connected, None) => {
                (p.text_weak, "Connecting", String::new())
            }
            (Connection::Disconnected(_), _) => (p.error, "Service stopped", String::new()),
        }
    }

    fn nav(&mut self, ui: &mut Ui) {
        let p = theme::current(ui.ctx());
        // The sidebar's top strip is part of the title bar.
        let top = ui
            .max_rect()
            .with_max_y(ui.max_rect().top() + chrome::HEIGHT);
        crate::chrome::drag_area(ui, top.expand2(egui::vec2(12.0, 0.0)));
        // Centers the 36 px logo row in the title bar strip.
        ui.add_space((chrome::HEIGHT - 36.0) / 2.0);
        if cfg!(target_os = "macos") {
            // Keeps the logo below the traffic lights.
            ui.add_space(chrome::TRAFFIC_LIGHTS);
        }
        ui.horizontal(|ui| {
            let dark = ui.visuals().dark_mode;
            if self.logo.as_ref().is_none_or(|(d, _)| *d != dark) {
                // The icon is a dark tile; on a dark sidebar show it inverted.
                let icon = crate::icon();
                let mut rgba = icon.rgba;
                if dark {
                    for px in rgba.as_chunks_mut::<4>().0 {
                        for c in &mut px[..3] {
                            *c = 255 - *c;
                        }
                    }
                }
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [icon.width as usize, icon.height as usize],
                    &rgba,
                );
                let tex = ui
                    .ctx()
                    .load_texture("logo", img, egui::TextureOptions::LINEAR);
                self.logo = Some((dark, tex));
            }
            let logo = &self.logo.as_ref().unwrap().1;
            ui.add(egui::Image::new((logo.id(), egui::vec2(36.0, 36.0))));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(widgets::medium("Otozane").size(17.0));
                ui.label(RichText::new("Microphone processor").small().weak());
            });
        });
        ui.add_space((chrome::HEIGHT - 36.0) / 2.0 + 4.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        for page in Page::ALL {
            let (glyph, label) = page.label();
            let selected = self.page == page;
            let (text, fill) = if selected {
                (
                    widgets::medium(icon_text(glyph, label)).color(p.text),
                    p.accent_soft,
                )
            } else {
                (
                    RichText::new(icon_text(glyph, label)),
                    egui::Color32::TRANSPARENT,
                )
            };
            let b = egui::Button::new(text)
                .fill(fill)
                .frame_when_inactive(selected)
                .corner_radius(24)
                .min_size(egui::vec2(ui.available_width(), 48.0));
            if ui.add(b).clicked() {
                self.page = page;
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let (color, title, detail) = self.state(&p);
            Frame::new()
                .fill(p.card)
                .corner_radius(16)
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    // Bottom-up layout: the last item added sits on top.
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if !detail.is_empty() {
                        ui.add(egui::Label::new(RichText::new(detail).small().weak()).truncate());
                    }
                    ui.horizontal(|ui| {
                        widgets::dot(ui, color, 4.0);
                        ui.label(widgets::medium(title).size(13.5));
                    });
                });
        });
    }

    fn disconnected(&mut self, ui: &mut Ui, reason: &str) {
        let p = theme::current(ui.ctx());
        ui.add_space(40.0);
        card(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(16.0);
                ui.label(icon(icons::UNPLUG, 34.0).color(p.text_weak));
                ui.add_space(6.0);
                ui.label(widgets::medium("Service not running").size(20.0));
                ui.label(
                    RichText::new(
                        "The Otozane service processes your microphone in the background.\nStart it to use the virtual microphone.",
                    )
                    .weak(),
                );
                ui.add_space(10.0);
                if ui
                    .add(widgets::primary(ui, icon_text(icons::POWER, "Start service")))
                    .clicked()
                    && let Err(e) = client::start_service()
                {
                    self.toasts.push((
                        format!("Could not start otozaned: {e}"),
                        Instant::now(),
                        true,
                    ));
                }
                ui.add_space(6.0);
                ui.label(RichText::new(reason).small().weak());
                ui.add_space(12.0);
            });
        });
    }

    fn overview(&mut self, ui: &mut Ui) {
        let Some(st) = self.status.clone() else {
            return;
        };
        let p = theme::current(ui.ctx());
        let (color, title, detail) = self.state(&p);

        // Hero: what is happening right now, and the master switch.
        card(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(52.0, 52.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(rect.center(), 26.0, color.gamma_multiply(0.16));
                let glyph = if st.processing_enabled && !st.muted {
                    icons::MIC
                } else {
                    icons::MIC_OFF
                };
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    glyph,
                    egui::FontId::proportional(24.0),
                    color,
                );
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(widgets::medium(title).size(20.0));
                    let sub = if detail.is_empty() {
                        "No input".to_string()
                    } else {
                        detail
                    };
                    ui.label(RichText::new(sub).weak());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let from = client::Mode::of(&st);
                    let mut mode = from;
                    if widgets::segmented(ui, &mut mode, &client::Mode::ALL) {
                        for req in mode.requests(from) {
                            self.send(req);
                        }
                    }
                });
            });
            ui.add_space(8.0);
            widgets::divider(ui);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let (glyph, c, text) = if st.virtual_source_ready {
                    (
                        icons::CIRCLE_CHECK,
                        p.accent,
                        format!(
                            "Apps can record from \u{201c}{}\u{201d}",
                            otozane_core::VIRTUAL_DEVICE
                        ),
                    )
                } else if cfg!(target_os = "macos") {
                    (
                        icons::CIRCLE_ALERT,
                        p.text_weak,
                        "BlackHole 2ch is not installed: brew install blackhole-2ch".into(),
                    )
                } else {
                    (
                        icons::CIRCLE_ALERT,
                        p.text_weak,
                        "Virtual microphone is not ready".into(),
                    )
                };
                ui.label(icon(glyph, 14.0).color(c));
                ui.label(RichText::new(text).small());
            });
        });

        section(ui, "Levels");
        card(ui, |ui| {
            ui.add_space(4.0);
            widgets::meters(
                ui,
                &[
                    ("Input", self.levels.0, self.holds[0].0),
                    ("Output", self.levels.1, self.holds[1].0),
                ],
            );
        });

        widgets::section_with(ui, "Signal chain", |ui| {
            if widgets::link(ui, "Edit effects").clicked() {
                self.page = Page::Effects;
            }
        });
        card(ui, |ui| {
            if !st.processing_enabled || st.muted {
                ui.multiply_opacity(0.45);
            }
            self.chain_strip(ui, &st);
        });

        section(ui, "Input");
        card(ui, |ui| {
            self.device_row(ui, &st);
            widgets::divider(ui);
            let profiles = self.profiles.clone();
            row(
                ui,
                "Profile",
                Some("Effect chain and all its settings"),
                |ui| {
                    egui::ComboBox::from_id_salt("profile")
                        .selected_text(&st.active_profile)
                        .width(240.0)
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
                },
            );
            widgets::divider(ui);
            row(
                ui,
                "Listen to myself",
                Some("Plays the processed microphone to your output. Use headphones"),
                |ui| {
                    let mut on = st.monitoring;
                    if widgets::switch(ui, &mut on).changed() {
                        self.send(Request::SetMonitor { on });
                    }
                },
            );
        });
    }

    /// Microphone → effects → virtual microphone, each effect a chip that
    /// toggles bypass on click.
    fn chain_strip(&mut self, ui: &mut Ui, st: &Status) {
        let p = theme::current(ui.ctx());
        let chip = |ui: &mut Ui,
                    glyph: &str,
                    label: &str,
                    fill: egui::Color32,
                    fg: egui::Color32,
                    strike: bool| {
            // M3 filter chip: tonal when selected, outlined otherwise.
            let mut text = RichText::new(icon_text(glyph, label)).color(fg);
            let mut b = egui::Button::new(text.clone());
            if strike {
                text = text.strikethrough();
                b = egui::Button::new(text).stroke(egui::Stroke::new(1.0, p.outline));
            }
            ui.add(
                b.fill(fill)
                    .corner_radius(8)
                    .min_size(egui::vec2(0.0, 32.0)),
            )
        };
        let arrow = |ui: &mut Ui| {
            ui.label(icon(icons::ARROW_RIGHT, 14.0).color(p.text_weak));
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 8.0);
            let input = match &st.input_device {
                None => "Microphone".to_string(),
                Some(n) => device_label(st, Some(n)),
            };
            chip(ui, icons::MIC, &input, p.raised, p.text, false);
            for i in 0..self.chain.len() {
                arrow(ui);
                let e = &self.chain[i];
                let on = e.enabled;
                let (fill, fg) = if on {
                    (p.accent_soft, p.text)
                } else {
                    (egui::Color32::TRANSPARENT, p.text_weak)
                };
                let tip = if on {
                    "Click to bypass"
                } else {
                    "Bypassed. Click to enable"
                };
                if chip(
                    ui,
                    effects::kind_icon(e.kind),
                    e.display_name(),
                    fill,
                    fg,
                    !on,
                )
                .on_hover_text(tip)
                .clicked()
                {
                    self.chain[i].enabled = !on;
                    self.edited();
                    let id = self.chain[i].id;
                    self.send(Request::SetEffectEnabled { id, enabled: !on });
                }
            }
            if self.chain.is_empty() {
                arrow(ui);
                ui.label(RichText::new("no effects").weak().italics());
            }
            arrow(ui);
            chip(
                ui,
                icons::RADIO,
                otozane_core::VIRTUAL_DEVICE,
                p.raised,
                p.text,
                false,
            );
        });
    }

    fn device_row(&mut self, ui: &mut Ui, st: &Status) {
        row(
            ui,
            "Microphone",
            Some("\u{201c}Automatic\u{201d} follows the system default"),
            |ui| {
                let selected = match &st.input_device {
                    None => "Automatic".to_string(),
                    Some(name) => device_label(st, Some(name)),
                };
                egui::ComboBox::from_id_salt("device")
                    .selected_text(selected)
                    .width(240.0)
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
            },
        );
    }

    fn profiles_page(&mut self, ui: &mut Ui) {
        let Some(active) = self.status.as_ref().map(|s| s.active_profile.clone()) else {
            return;
        };
        let p = theme::current(ui.ctx());
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                widgets::heading(
                    ui,
                    "Profiles",
                    Some("Each profile keeps an effect chain with all its settings. The active one saves automatically."),
                );
            });
        });
        widgets::section_with(ui, "Saved profiles", |ui| {
            if widgets::link(ui, icon_text(icons::UPLOAD, "Import"))
                .on_hover_text("Import a profile from a .toml file")
                .clicked()
            {
                self.client.import_profile();
            }
        });
        card(ui, |ui| {
            let names = self.profiles.clone();
            for (i, name) in names.iter().enumerate() {
                if i > 0 {
                    widgets::divider(ui);
                }
                let is_active = *name == active;
                if let Some((from, to)) = &mut self.renaming
                    && from == name
                {
                    let mut commit = None;
                    ui.horizontal(|ui| {
                        ui.set_min_height(40.0);
                        let r = ui.add(egui::TextEdit::singleline(to).desired_width(260.0));
                        r.request_focus();
                        if ui.add(widgets::primary(ui, "Rename")).clicked()
                            || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            commit = Some(true);
                        }
                        if ui.button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            commit = Some(false);
                        }
                    });
                    if let Some(ok) = commit {
                        let (from, to) = self.renaming.take().unwrap();
                        if ok && from != to && !to.trim().is_empty() {
                            self.send(Request::RenameProfile {
                                from,
                                to: to.trim().to_string(),
                            });
                        }
                    }
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.set_min_height(40.0);
                    let glyph = if is_active {
                        icons::CIRCLE_CHECK
                    } else {
                        icons::BOOKMARK
                    };
                    ui.label(icon(glyph, 16.0).color(if is_active {
                        p.accent
                    } else {
                        p.text_weak
                    }));
                    if is_active {
                        ui.label(widgets::medium(name));
                        widgets::badge(ui, "Active", p.accent, p.accent_soft);
                    } else {
                        ui.label(name);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !is_active
                            && widgets::icon_button(ui, icons::TRASH_2, "Delete").clicked()
                        {
                            self.send(Request::DeleteProfile { name: name.clone() });
                        }
                        if widgets::icon_button(ui, icons::DOWNLOAD, "Export to a file").clicked() {
                            self.send(Request::GetProfile { name: name.clone() });
                        }
                        if widgets::icon_button(ui, icons::PENCIL, "Rename").clicked() {
                            self.renaming = Some((name.clone(), name.clone()));
                        }
                        if !is_active && ui.add(widgets::tonal(ui, "Switch to")).clicked() {
                            self.send(Request::LoadProfile { name: name.clone() });
                        }
                    });
                });
            }
        });

        section(ui, "New profile");
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(40.0);
                let name = self.new_profile.trim().to_string();
                let w = ui.available_width() - 190.0;
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.new_profile)
                        .hint_text("Name, e.g. Streaming")
                        .desired_width(w.max(120.0)),
                );
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if (ui
                        .add_enabled(
                            !name.is_empty(),
                            widgets::primary(ui, icon_text(icons::SAVE, "Save current chain")),
                        )
                        .clicked()
                        || enter)
                        && !name.is_empty()
                    {
                        self.send(Request::SaveProfileAs { name });
                        self.new_profile.clear();
                    }
                });
            });
        });

        section(ui, "Presets");
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(48.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 10.0, p.accent_soft);
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icons::SPARKLES, egui::FontId::proportional(17.0), p.accent);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(widgets::medium("Voice"));
                    ui.label(RichText::new("Gain, noise suppression with voice detection, gate, compressor, limiter").small().weak());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(widgets::tonal(ui, icon_text(icons::PLUS, "Create"))).on_hover_text("Creates a new profile from this preset").clicked() {
                        // Keeps the current profile: switch to a new one, then fill it.
                        let name = unique_name(&self.profiles, "Voice");
                        self.send(Request::SaveProfileAs { name });
                        self.send(Request::SetChain {
                            chain: otozane_core::config::Profile::voice().chain,
                        });
                    }
                });
            });
        });
    }

    fn settings_page(&mut self, ui: &mut Ui) {
        widgets::heading(ui, "Settings", None);
        section(ui, "Appearance");
        card(ui, |ui| {
            row(ui, "Theme", None, |ui| {
                let mut choice = self.settings.theme;
                if widgets::segmented(
                    ui,
                    &mut choice,
                    &[
                        (ThemeChoice::System, "System"),
                        (ThemeChoice::Light, "Light"),
                        (ThemeChoice::Dark, "Dark"),
                    ],
                ) {
                    self.settings.theme = choice;
                    theme::apply(ui.ctx(), choice);
                    self.settings.save();
                }
            });
        });
        section(ui, "Background");
        card(ui, |ui| {
            row(
                ui,
                "Start with session",
                Some("Runs the audio service when you log in"),
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
            // macOS has no tray, so the window cannot start hidden.
            if !cfg!(target_os = "macos") {
                widgets::divider(ui);
                let hint = if self.has_tray {
                    "Starts this window hidden in the tray when you log in"
                } else {
                    "Needs a system tray, none is running"
                };
                row(ui, "Open in tray at login", Some(hint), |ui| {
                    let mut on = self.login_window;
                    ui.add_enabled_ui(self.has_tray || on, |ui| {
                        if widgets::switch(ui, &mut on).changed() {
                            match client::set_login_window(on) {
                                Ok(()) => self.login_window = on,
                                Err(e) => self.toasts.push((
                                    format!("Could not change login autostart: {e}"),
                                    Instant::now(),
                                    true,
                                )),
                            }
                        }
                    });
                });
            }
            if self.status.is_some() {
                widgets::divider(ui);
                let sub = if self.has_tray && cfg!(target_os = "macos") {
                    "Keeps running when this window is closed; Otozane stays in the menu bar"
                } else if self.has_tray {
                    "Keeps running when this window is closed; the window hides to the tray"
                } else {
                    "Keeps running when this window is closed"
                };
                row(ui, "Audio service", Some(sub), |ui| {
                    if ui
                        .add(widgets::danger(ui, icon_text(icons::POWER, "Stop")))
                        .clicked()
                    {
                        self.send(Request::Shutdown);
                    }
                });
            }
        });
        let Some(st) = self.status.clone() else {
            return;
        };
        section(ui, "Audio");
        card(ui, |ui| {
            self.device_row(ui, &st);
            widgets::divider(ui);
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
                    ui.label(format!(
                        "{} kHz · {ch}",
                        st.diagnostics.sample_rate as f32 / 1000.0
                    ));
                },
            );
        });
        section(ui, "Diagnostics");
        card(ui, |ui| {
            let d = &st.diagnostics;
            let ms = |f: u32| f as f32 * 1000.0 / d.sample_rate.max(1) as f32;
            let items = [
                (
                    "Estimated latency",
                    format!("{:.1} ms", d.estimated_latency_ms()),
                ),
                (
                    "Effect latency",
                    format!(
                        "{:.1} ms · {} fr",
                        ms(d.chain_latency_frames),
                        d.chain_latency_frames
                    ),
                ),
                (
                    "Capture quantum",
                    format!("{:.1} ms · {} fr", ms(d.capture_quantum), d.capture_quantum),
                ),
                ("Output quantum", format!("{} fr", d.playback_quantum)),
                ("Queued output", format!("{} fr", d.ring_fill_frames)),
                ("Underruns", format!("{} fr", d.underrun_frames)),
                ("Overruns", format!("{} fr", d.overrun_frames)),
                (
                    "Latency corrections",
                    format!("{} fr skipped", d.skipped_frames),
                ),
                ("Processed blocks", d.processed_blocks.to_string()),
                ("Service version", st.version.clone()),
            ];
            let col = (ui.available_width() - 24.0) / 2.0;
            egui::Grid::new("diag")
                .num_columns(2)
                .spacing([24.0, 10.0])
                .min_col_width(col)
                .show(ui, |ui| {
                    for pair in items.chunks(2) {
                        for (k, v) in pair {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(RichText::new(*k).small().weak());
                                ui.label(widgets::medium(v.as_str()));
                            });
                        }
                        ui.end_row();
                    }
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
        #[cfg(target_os = "macos")]
        if self.has_tray {
            crate::menubar::on_frame(ui.ctx(), self.status.as_ref());
        }
        dev_screenshot(ui);
        let p = theme::current(ui.ctx());
        egui::Panel::left("nav")
            .exact_size(216.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.sidebar).inner_margin(Margin {
                left: 12,
                right: 12,
                top: 0,
                bottom: 14,
            }))
            .show(ui, |ui| self.nav(ui));
        egui::Panel::top("titlebar")
            .exact_size(chrome::HEIGHT)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(p.window)
                    .inner_margin(Margin::symmetric(14, 0)),
            )
            .show(ui, |ui| {
                chrome::drag_area(ui, ui.max_rect().expand2(egui::vec2(14.0, 0.0)));
                if !cfg!(target_os = "macos") {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        chrome::buttons,
                    );
                }
            });
        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.window).inner_margin(Margin {
                left: 32,
                right: 32,
                top: 0,
                bottom: 24,
            }))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let width = ui.available_width().min(760.0);
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
        if !cfg!(target_os = "macos") {
            chrome::resize_edges(ui.ctx());
        }
        if !self.toasts.is_empty() {
            egui::Area::new(egui::Id::unique("toasts"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -16.0))
                .show(ui.ctx(), |ui| {
                    for (msg, _, is_error) in &self.toasts {
                        let glyph = if *is_error {
                            icons::CIRCLE_ALERT
                        } else {
                            icons::CIRCLE_CHECK
                        };
                        // M3 snackbar: inverse surface.
                        Frame::popup(ui.style())
                            .fill(p.inverse)
                            .corner_radius(8)
                            .inner_margin(Margin::symmetric(16, 12))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(icon(glyph, 16.0).color(p.on_inverse));
                                    ui.label(RichText::new(msg).color(p.on_inverse));
                                });
                            });
                    }
                });
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    }
}

/// Peak hold: jumps to a higher level, stays put for 1.5 s, then falls at
/// the meter's decay rate.
fn peak_hold(hold: (f32, Instant), level: f32, now: Instant, decay: f32) -> (f32, Instant) {
    if level >= hold.0 {
        (level, now)
    } else if now - hold.1 > Duration::from_millis(1500) {
        ((hold.0 * decay).max(level), hold.1)
    } else {
        hold
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
    ctx.request_screenshot(move |img| {
        let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
        if std::fs::write(&path, bytes).is_ok() {
            println!("{}x{}", img.size[0], img.size[1]);
        }
        // Exits rather than closing, which would only hide to the tray.
        std::process::exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_hold_holds_then_falls() {
        let t0 = Instant::now();
        let h = peak_hold((0.0, t0), 0.5, t0, 0.9);
        assert_eq!(h.0, 0.5);
        let h = peak_hold(h, 0.1, t0 + Duration::from_secs(1), 0.9);
        assert_eq!(h.0, 0.5, "held for 1.5 s");
        let h = peak_hold(h, 0.1, t0 + Duration::from_secs(2), 0.9);
        assert!((h.0 - 0.45).abs() < 1e-6, "falls after the hold");
        assert_eq!(peak_hold(h, 0.8, t0, 0.9).0, 0.8, "jumps to a new peak");
    }
}
