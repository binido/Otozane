//! Effects chain editor: ordering, enable/disable, parameters.

use crate::app::App;
use crate::icons;
use crate::theme;
use crate::widgets::{self, icon, icon_text};
use egui::{Margin, RichText, Stroke, Ui};
use otozane_core::config::{EffectConfig, PluginRef, next_id};
use otozane_core::effects::EffectKind;
use otozane_core::param::{ParamKind, ParamSpec};
use otozane_core::protocol::{EffectInfo, Request};

enum Action {
    Move(usize, usize),
    Remove(usize),
    Add(EffectKind),
    AddPlugin(PluginRef),
}

/// Icon shown next to an effect name, here and in the overview chain.
pub fn kind_icon(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::Gain => icons::VOLUME_2,
        EffectKind::Rnnoise => icons::SPARKLES,
        EffectKind::Gate => icons::SHIELD_CHECK,
        EffectKind::Equalizer => icons::SLIDERS_HORIZONTAL,
        EffectKind::Compressor => icons::GAUGE,
        EffectKind::Limiter => icons::BRICK_WALL,
        EffectKind::Plugin => icons::PUZZLE,
    }
}

const LABEL_W: f32 = 150.0;

/// One parameter line: fixed-width label, then the control filling the rest,
/// so sliders line up across all effect cards.
fn param_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(30.0);
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_W, 30.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(LABEL_W);
                ui.add(egui::Label::new(RichText::new(label).weak()).truncate());
            },
        );
        ui.spacing_mut().slider_width = (ui.available_width() - 110.0).max(80.0);
        add(ui);
    });
}

pub fn page(app: &mut App, ui: &mut Ui) {
    let Some((meters, infos)) = app.status().map(|s| (s.meters.clone(), s.effects.clone())) else {
        return;
    };
    let p = theme::current(ui.ctx());
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            widgets::heading(
                ui,
                "Effects",
                Some("Sound passes through the chain from top to bottom."),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            ui.add_space(2.0);
            let add = widgets::primary(ui, icon_text(icons::PLUS, "Add effect"));
            egui::containers::menu::MenuButton::from_button(add).ui(ui, |ui| {
                ui.set_min_width(200.0);
                for &kind in EffectKind::ALL {
                    if ui.button(icon_text(kind_icon(kind), kind.name())).clicked() {
                        app_action(app, Action::Add(kind));
                    }
                }
                ui.separator();
                if ui
                    .button(icon_text(icons::PUZZLE, "VST3 plugin..."))
                    .clicked()
                {
                    app.browser.get_or_insert_with(String::new);
                }
            });
        });
    });
    if let Some(a) = browser(app, ui) {
        app_action(app, a);
        app.browser = None;
    }

    if app.chain.is_empty() {
        widgets::card(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(14.0);
                ui.label(icon(icons::AUDIO_WAVEFORM, 30.0).color(p.text_weak));
                ui.label(widgets::medium("No effects yet"));
                ui.label(
                    RichText::new(
                        "The microphone passes through unchanged. Add an effect to start.",
                    )
                    .weak(),
                );
                ui.add_space(14.0);
            });
        });
    }

    let mut action = None;
    let n = app.chain.len();
    for i in 0..n {
        let id = app.chain[i].id;
        let kind = app.chain[i].kind;
        let on = app.chain[i].enabled;
        let expanded = !app.collapsed.contains(&id);
        let resp = widgets::card_frame(ui)
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.set_min_height(36.0);
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.dnd_drag_source(egui::Id::unique(("effect-drag", id)), i, |ui| {
                        ui.label(icon(icons::GRIP_VERTICAL, 16.0).color(p.text_weak));
                    })
                    .response
                    .on_hover_cursor(egui::CursorIcon::Grab)
                    .on_hover_text("Drag to reorder");
                    // Kind tile, tinted while active.
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
                    let (tile, fg) = if on {
                        (p.accent_soft, p.accent)
                    } else {
                        (p.raised, p.text_weak)
                    };
                    ui.painter().rect_filled(rect, 9.0, tile);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        kind_icon(kind),
                        egui::FontId::proportional(16.0),
                        fg,
                    );
                    ui.add_space(4.0);
                    let chevron = if expanded {
                        icons::CHEVRON_DOWN
                    } else {
                        icons::CHEVRON_RIGHT
                    };
                    let mut title =
                        widgets::medium(format!("{}  {chevron}", app.chain[i].display_name()));
                    if !on {
                        title = title.weak();
                    }
                    if ui
                        .add(egui::Label::new(title).sense(egui::Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        if expanded {
                            app.collapsed.insert(id);
                        } else {
                            app.collapsed.remove(&id);
                        }
                    }
                    if !on {
                        widgets::badge(ui, "Bypassed", p.text_weak, p.raised);
                    } else if let (Some((label, unit)), Some(v)) = (kind.meter(), meters.get(i)) {
                        widgets::badge(
                            ui,
                            &format!("{label} {v:.0} {unit}"),
                            p.text_weak,
                            p.raised,
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut on = on;
                        if widgets::switch(ui, &mut on)
                            .on_hover_text("Enable or bypass")
                            .changed()
                        {
                            app.chain[i].enabled = on;
                            app.edited();
                            app.send(Request::SetEffectEnabled { id, enabled: on });
                        }
                        ui.add_space(6.0);
                        if widgets::icon_button(ui, icons::TRASH_2, "Remove").clicked() {
                            action = Some(Action::Remove(i));
                        }
                        ui.add_enabled_ui(i + 1 < n, |ui| {
                            if widgets::icon_button(ui, icons::ARROW_DOWN, "Move down").clicked() {
                                action = Some(Action::Move(i, i + 1));
                            }
                        });
                        ui.add_enabled_ui(i > 0, |ui| {
                            if widgets::icon_button(ui, icons::ARROW_UP, "Move up").clicked() {
                                action = Some(Action::Move(i, i - 1));
                            }
                        });
                    });
                });
                let info = infos.get(i).cloned().unwrap_or_default();
                if let Some(e) = &info.error {
                    ui.label(RichText::new(icon_text(icons::CIRCLE_ALERT, e)).color(p.error));
                }
                if expanded {
                    ui.add_space(6.0);
                    widgets::divider(ui);
                    ui.add_space(6.0);
                    if !on {
                        ui.multiply_opacity(0.5);
                    }
                    if kind == EffectKind::Plugin {
                        plugin_params(app, ui, i, &info);
                    } else {
                        params(app, ui, i);
                    }
                    ui.add_space(2.0);
                }
            })
            .response;
        if let Some(src) = resp.dnd_release_payload::<usize>() {
            action = Some(Action::Move(*src, i));
        } else if resp.dnd_hover_payload::<usize>().is_some() {
            let y = resp.rect.top() - 6.0;
            ui.painter()
                .hline(resp.rect.x_range(), y, Stroke::new(3.0, p.accent_bg));
        }
        // Connector to the next effect: the signal flows down.
        if i + 1 < n {
            let (rect, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 14.0), egui::Sense::hover());
            let x = resp.rect.left() + 14.0 + 16.0 + 6.0 + 16.0;
            ui.painter()
                .vline(x, rect.y_range().expand(4.0), Stroke::new(2.0, p.border));
        }
    }
    if let Some(a) = action {
        app_action(app, a);
    }
}

fn app_action(app: &mut App, action: Action) {
    let mut chain = app.chain.clone();
    match action {
        Action::Move(from, to) if from != to && from < chain.len() && to < chain.len() => {
            let e = chain.remove(from);
            chain.insert(to, e);
        }
        Action::Remove(i) if i < chain.len() => {
            chain.remove(i);
        }
        Action::Add(kind) => {
            let e = EffectConfig::new(next_id(&chain), kind);
            chain.push(e);
        }
        Action::AddPlugin(plugin) => {
            let e = EffectConfig::new_plugin(next_id(&chain), plugin);
            chain.push(e);
        }
        _ => return,
    }
    app.chain = chain.clone();
    app.edited();
    app.send(Request::SetChain { chain });
}

fn params(app: &mut App, ui: &mut Ui, i: usize) {
    let kind = app.chain[i].kind;
    let id = app.chain[i].id;
    let values = app.chain[i].values();
    let mut changes = Vec::new();
    if kind == EffectKind::Equalizer {
        eq_editor(ui, id, &values, &mut changes);
    } else {
        for (spec, &v) in kind.params().iter().zip(&values) {
            let mut v = v;
            param_row(ui, spec.name, |ui| {
                if param(ui, spec, &mut v, id) {
                    changes.push((spec.key, v));
                }
            });
        }
    }
    for (key, value) in changes {
        app.chain[i].params.insert(key.to_string(), value);
        app.edited();
        app.send(Request::SetParam {
            id,
            key: key.to_string(),
            value,
        });
    }
}

/// Plugin browser: search the last scan result and add an effect to the chain.
fn browser(app: &mut App, ui: &mut Ui) -> Option<Action> {
    let mut query = app.browser.clone()?;
    let p = theme::current(ui.ctx());
    let mut action = None;
    let mut close = false;
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(icon(icons::SEARCH, 16.0).color(p.text_weak));
            ui.add(
                egui::TextEdit::singleline(&mut query)
                    .hint_text("Search plugins by name, vendor or category")
                    .desired_width(ui.available_width() - 150.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(icon(icons::X, 15.0))
                    .on_hover_text("Close")
                    .clicked()
                {
                    close = true;
                }
                let scanning = app.plugins.scanning;
                let label = if scanning { "Scanning..." } else { "Rescan" };
                if ui
                    .add_enabled(
                        !scanning,
                        egui::Button::new(icon_text(icons::REFRESH_CW, label)),
                    )
                    .on_hover_text(app.plugins.dirs.join("\n"))
                    .clicked()
                {
                    app.send(Request::ScanPlugins);
                }
            });
        });
        ui.add_space(6.0);
        let q = query.to_lowercase();
        let matches: Vec<_> = app
            .plugins
            .plugins
            .iter()
            .filter(|i| {
                q.split_whitespace().all(|w| {
                    i.plugin.name.to_lowercase().contains(w)
                        || i.plugin.vendor.to_lowercase().contains(w)
                        || i.category.to_lowercase().contains(w)
                })
            })
            .collect();
        if app.plugins.plugins.is_empty() {
            ui.label(
                RichText::new(if app.plugins.scanning {
                    "Looking for VST3 plugins..."
                } else {
                    "No VST3 plugins found. Install some (e.g. lsp-plugins-vst3) and press Rescan."
                })
                .weak(),
            );
        }
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for info in matches {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(&info.plugin.name);
                            ui.label(
                                RichText::new(format!(
                                    "{} \u{b7} {} \u{b7} {}",
                                    info.plugin.vendor,
                                    info.plugin.format.name(),
                                    info.category
                                ))
                                .small()
                                .weak(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(icon_text(icons::PLUS, "Add")).clicked() {
                                action = Some(Action::AddPlugin(info.plugin.clone()));
                            }
                        });
                    });
                    ui.separator();
                }
            });
        if !app.plugins.errors.is_empty() {
            egui::CollapsingHeader::new(
                RichText::new(format!(
                    "{} files could not be loaded",
                    app.plugins.errors.len()
                ))
                .color(p.warning),
            )
            .show(ui, |ui| {
                for (path, why) in &app.plugins.errors {
                    ui.label(RichText::new(format!("{path}: {why}")).small());
                }
            });
        }
    });
    app.browser = (!close).then_some(query);
    action
}

/// Plugin parameters as reported by the service. Values are normalized (0..1).
fn plugin_params(app: &mut App, ui: &mut Ui, i: usize, info: &EffectInfo) {
    let id = app.chain[i].id;
    if info.params.is_empty() {
        if info.error.is_none() {
            ui.label(RichText::new("This plugin has no adjustable parameters.").weak());
        }
        return;
    }
    let mut changes = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt(("plugin-params", id))
        .max_height(360.0)
        .show(ui, |ui| {
            for (j, d) in info.params.iter().enumerate() {
                let remote = info.values.get(j).copied().unwrap_or(d.default);
                let mut v = app.chain[i].params.get(&d.key).copied().unwrap_or(remote);
                let mut changed = false;
                param_row(ui, &d.name, |ui| {
                    changed = if d.steps == 1 {
                        let mut on = v >= 0.5;
                        let c = widgets::switch(ui, &mut on).changed();
                        v = if on { 1.0 } else { 0.0 };
                        c
                    } else if !d.choices.is_empty() {
                        let n = d.steps as f32;
                        let mut idx = (v * n).round() as usize;
                        let before = idx;
                        egui::ComboBox::from_id_salt(("pchoice", id, j))
                            .selected_text(d.choices.get(idx).map_or("?", |s| s.as_str()))
                            .show_ui(ui, |ui| {
                                for (k, l) in d.choices.iter().enumerate() {
                                    ui.selectable_value(&mut idx, k, l);
                                }
                            });
                        v = idx as f32 / n;
                        idx != before
                    } else {
                        ui.add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false))
                            .changed()
                    };
                    // The plugin formats the value; it lags one status poll behind edits.
                    let text = info.texts.get(j).map_or("", |s| s.as_str());
                    ui.label(
                        RichText::new(format!("{text} {}", d.unit))
                            .font(egui::FontId::monospace(13.0)),
                    );
                });
                if changed {
                    changes.push((d.key.clone(), v));
                }
            }
        });
    for (key, value) in changes {
        app.chain[i].params.insert(key.clone(), value);
        app.edited();
        app.send(Request::SetParam { id, key, value });
    }
}

/// Equalizer: frequency response curve and one row per band.
fn eq_editor(ui: &mut Ui, id: u64, values: &[f32], changes: &mut Vec<(&'static str, f32)>) {
    let p = theme::current(ui.ctx());
    let sr = otozane_core::SAMPLE_RATE as f32;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 160.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 10.0, ui.visuals().extreme_bg_color);
    let x_of = |f: f32| rect.left() + rect.width() * (f / 20.0).log10() / 3.0;
    let y_of =
        |db: f32| rect.center().y - db.clamp(-24.0, 24.0) / 24.0 * (rect.height() / 2.0 - 6.0);
    let grid = Stroke::new(1.0, p.border);
    for f in [100.0f32, 1000.0, 10000.0] {
        painter.vline(x_of(f), rect.y_range(), grid);
        let label = if f >= 1000.0 {
            format!("{}k", f / 1000.0)
        } else {
            format!("{f}")
        };
        painter.text(
            egui::pos2(x_of(f) + 4.0, rect.bottom() - 4.0),
            egui::Align2::LEFT_BOTTOM,
            label,
            egui::FontId::proportional(11.0),
            p.text_weak,
        );
    }
    for db in [-12.0, 12.0] {
        painter.hline(rect.x_range(), y_of(db), grid);
    }
    painter.hline(rect.x_range(), y_of(0.0), Stroke::new(1.0, p.text_weak));
    let points: Vec<egui::Pos2> = (0..=200)
        .map(|k| {
            let f = 20.0 * 1000f32.powf(k as f32 / 200.0);
            egui::pos2(
                x_of(f),
                y_of(otozane_core::effects::eq::response_db(values, f, sr)),
            )
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(2.0, p.accent_bg)));
    ui.add_space(6.0);

    let specs = EffectKind::Equalizer.params();
    egui::Grid::new(("eq", id))
        .num_columns(5)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            for h in ["", "Type", "Frequency", "Gain", "Q"] {
                ui.label(RichText::new(h).small().weak());
            }
            ui.end_row();
            for (b, (band, vals)) in specs
                .as_chunks::<4>()
                .0
                .iter()
                .zip(values.as_chunks::<4>().0)
                .enumerate()
            {
                ui.label(format!("{}", b + 1));
                let on = vals[0].round() as i32 != 0;
                let has_gain = matches!(vals[0].round() as i32, 1..=3);
                for (k, (spec, &v)) in band.iter().zip(vals).enumerate() {
                    let mut v = v;
                    let enabled = k == 0 || (on && (k != 2 || has_gain));
                    ui.add_enabled_ui(enabled, |ui| {
                        ui.spacing_mut().slider_width = 110.0;
                        if param(ui, spec, &mut v, id) {
                            changes.push((spec.key, v));
                        }
                    });
                }
                ui.end_row();
            }
        });
}

fn param(ui: &mut Ui, spec: &ParamSpec, v: &mut f32, id: u64) -> bool {
    match spec.kind {
        ParamKind::Continuous | ParamKind::Logarithmic => {
            let range = spec.max - spec.min;
            let decimals = if range > 500.0 {
                0
            } else if range > 20.0 {
                1
            } else {
                2
            };
            let suffix = if spec.unit.is_empty() {
                String::new()
            } else {
                format!(" {}", spec.unit)
            };
            ui.add(
                egui::Slider::new(v, spec.min..=spec.max)
                    .logarithmic(spec.kind == ParamKind::Logarithmic)
                    .fixed_decimals(decimals)
                    .suffix(suffix),
            )
            .changed()
        }
        ParamKind::Toggle => {
            let mut on = *v >= 0.5;
            let changed = widgets::switch(ui, &mut on).changed();
            *v = if on { 1.0 } else { 0.0 };
            changed
        }
        ParamKind::Choice(labels) => {
            let mut idx = (v.round() as usize).min(labels.len().saturating_sub(1));
            let before = idx;
            egui::ComboBox::from_id_salt((spec.key, id))
                .selected_text(labels.get(idx).copied().unwrap_or("?"))
                .show_ui(ui, |ui| {
                    for (j, l) in labels.iter().enumerate() {
                        ui.selectable_value(&mut idx, j, *l);
                    }
                });
            *v = idx as f32;
            idx != before
        }
    }
}
