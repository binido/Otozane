//! Effects chain editor: ordering, enable/disable, parameters.

use crate::app::App;
use crate::icons;
use crate::theme;
use crate::widgets::{self, icon, icon_text};
use egui::{CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use otozane_core::config::{EffectConfig, next_id};
use otozane_core::effects::EffectKind;
use otozane_core::param::{ParamKind, ParamSpec};
use otozane_core::protocol::Request;

enum Action {
    Move(usize, usize),
    Remove(usize),
    Add(EffectKind),
}

pub fn page(app: &mut App, ui: &mut Ui) {
    let Some(meters) = app.status().map(|s| s.meters.clone()) else {
        return;
    };
    let p = theme::current(ui.ctx());
    ui.horizontal(|ui| {
        widgets::heading(ui, "Effects");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(icon_text(icons::PLUS, "Add effect"), |ui| {
                for &kind in EffectKind::ALL {
                    if ui.button(kind.name()).clicked() {
                        app_action(app, Action::Add(kind));
                    }
                }
            });
        });
    });
    ui.label(RichText::new("Audio flows from top to bottom. Drag the handle to reorder.").weak());
    ui.add_space(8.0);

    if app.chain.is_empty() {
        widgets::card(ui, |ui| {
            ui.label(
                RichText::new("The chain is empty, so the microphone passes through unchanged.")
                    .weak(),
            );
        });
    }

    let mut action = None;
    for i in 0..app.chain.len() {
        let id = app.chain[i].id;
        let kind = app.chain[i].kind;
        let expanded = !app.collapsed.contains(&id);
        let resp = Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.dnd_drag_source(egui::Id::unique(("effect-drag", id)), i, |ui| {
                        ui.label(icon(icons::GRIP_VERTICAL, 18.0).color(p.text_weak));
                    })
                    .response
                    .on_hover_text("Drag to reorder");
                    let chevron = if expanded {
                        icons::CHEVRON_DOWN
                    } else {
                        icons::CHEVRON_RIGHT
                    };
                    let title = RichText::new(format!("{chevron}  {}", kind.name()))
                        .family(egui::FontFamily::Name(theme::MEDIUM.into()));
                    if ui
                        .add(egui::Label::new(title).sense(egui::Sense::click()))
                        .clicked()
                    {
                        if expanded {
                            app.collapsed.insert(id);
                        } else {
                            app.collapsed.remove(&id);
                        }
                    }
                    if let (Some((label, unit)), Some(v)) = (kind.meter(), meters.get(i)) {
                        ui.label(
                            RichText::new(format!("{label} {v:.0} {unit}"))
                                .small()
                                .weak(),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut on = app.chain[i].enabled;
                        if widgets::switch(ui, &mut on)
                            .on_hover_text("Enable or bypass")
                            .changed()
                        {
                            app.chain[i].enabled = on;
                            app.edited();
                            app.send(Request::SetEffectEnabled { id, enabled: on });
                        }
                        if ui
                            .button(icon(icons::TRASH_2, 15.0))
                            .on_hover_text("Remove")
                            .clicked()
                        {
                            action = Some(Action::Remove(i));
                        }
                        if i + 1 < app.chain.len()
                            && ui
                                .button(icon(icons::ARROW_DOWN, 15.0))
                                .on_hover_text("Move down")
                                .clicked()
                        {
                            action = Some(Action::Move(i, i + 1));
                        }
                        if i > 0
                            && ui
                                .button(icon(icons::ARROW_UP, 15.0))
                                .on_hover_text("Move up")
                                .clicked()
                        {
                            action = Some(Action::Move(i, i - 1));
                        }
                    });
                });
                if expanded {
                    ui.add_space(4.0);
                    params(app, ui, i);
                }
            })
            .response;
        if let Some(src) = resp.dnd_release_payload::<usize>() {
            action = Some(Action::Move(*src, i));
        } else if resp.dnd_hover_payload::<usize>().is_some() {
            let y = resp.rect.top() - 4.0;
            ui.painter()
                .hline(resp.rect.x_range(), y, Stroke::new(2.0, p.accent_bg));
        }
        ui.add_space(6.0);
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
        egui::Grid::new(("params", id))
            .num_columns(2)
            .spacing([18.0, 8.0])
            .show(ui, |ui| {
                for (spec, &v) in kind.params().iter().zip(&values) {
                    let mut v = v;
                    ui.label(spec.name);
                    if param(ui, spec, &mut v, id) {
                        changes.push((spec.key, v));
                    }
                    ui.end_row();
                }
            });
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

/// Equalizer: frequency response curve and one row per band.
fn eq_editor(ui: &mut Ui, id: u64, values: &[f32], changes: &mut Vec<(&'static str, f32)>) {
    let p = theme::current(ui.ctx());
    let sr = otozane_core::SAMPLE_RATE as f32;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 140.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, ui.visuals().extreme_bg_color);
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
                .chunks_exact(4)
                .zip(values.chunks_exact(4))
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
