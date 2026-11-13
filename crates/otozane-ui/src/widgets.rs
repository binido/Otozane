//! Small custom widgets: switch, segmented control, LED meters, cards, buttons.

use crate::theme::{self, MEDIUM};
use egui::{
    Align2, Button, Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Response, RichText,
    Sense, Stroke, Ui, Vec2, WidgetInfo,
};

/// Icon glyph at the given size, in the current text color unless styled further.
pub fn icon(glyph: &str, size: f32) -> RichText {
    RichText::new(glyph).size(size)
}

/// Icon followed by a label, for buttons and menu entries.
pub fn icon_text(glyph: &str, label: &str) -> String {
    format!("{glyph}  {label}")
}

pub fn medium(text: impl Into<String>) -> RichText {
    RichText::new(text).family(FontFamily::Name(MEDIUM.into()))
}

/// Page title with an optional one-line description under it.
pub fn heading(ui: &mut Ui, text: &str, subtitle: Option<&str>) {
    ui.label(RichText::new(text).heading());
    if let Some(s) = subtitle {
        ui.label(RichText::new(s).weak());
    }
    ui.add_space(10.0);
}

/// Small uppercase label above a group of controls.
pub fn section(ui: &mut Ui, text: &str) {
    section_with(ui, text, |_| {});
}

/// Section label with a control (usually a link-style button) on the right.
pub fn section_with(ui: &mut Ui, text: &str, right: impl FnOnce(&mut Ui)) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        ui.label(medium(text).size(14.0).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    ui.add_space(2.0);
}

/// Accent-colored text button without a frame, for secondary actions.
pub fn link(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = theme::current(ui.ctx());
    ui.add(
        Button::new(medium(text).color(p.accent))
            .frame_when_inactive(false)
            .min_size(Vec2::new(0.0, 24.0)),
    )
}

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_frame(ui)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn card_frame(ui: &Ui) -> Frame {
    let p = theme::current(ui.ctx());
    // M3 filled card: tonal surface, no outline.
    Frame::new()
        .fill(p.card)
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::symmetric(16, 12))
}

/// Row with a title (and optional subtitle) on the left and controls on the right.
pub fn row(ui: &mut Ui, title: &str, subtitle: Option<&str>, controls: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(if subtitle.is_some() { 44.0 } else { 36.0 });
        ui.vertical(|ui| {
            if subtitle.is_some() {
                ui.add_space(2.0);
            } else {
                ui.add_space(8.0);
            }
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(title);
            if let Some(s) = subtitle {
                ui.label(RichText::new(s).small().weak());
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), controls);
    });
}

/// Thin divider between rows inside a card.
pub fn divider(ui: &mut Ui) {
    let p = theme::current(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, p.border));
}

/// M3 filled button for the main action of a view.
pub fn primary(ui: &Ui, text: impl Into<String>) -> Button<'static> {
    let p = theme::current(ui.ctx());
    Button::new(medium(text).color(p.on_accent))
        .fill(p.accent_bg)
        .min_size(Vec2::new(0.0, 40.0))
}

/// M3 tonal button: secondary actions that still deserve weight.
pub fn tonal(ui: &Ui, text: impl Into<String>) -> Button<'static> {
    let p = theme::current(ui.ctx());
    Button::new(medium(text))
        .fill(p.accent_soft)
        .min_size(Vec2::new(0.0, 40.0))
}

/// M3 outlined button in the error color, for destructive actions.
pub fn danger(ui: &Ui, text: impl Into<String>) -> Button<'static> {
    let p = theme::current(ui.ctx());
    Button::new(medium(text).color(p.error))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, p.outline))
        .min_size(Vec2::new(0.0, 40.0))
}

/// Icon-only button without a background until hovered.
pub fn icon_button(ui: &mut Ui, glyph: &str, tip: &str) -> Response {
    let p = theme::current(ui.ctx());
    ui.add(
        Button::new(icon(glyph, 15.0).color(p.text_weak))
            .frame_when_inactive(false)
            .min_size(Vec2::splat(30.0)),
    )
    .on_hover_text(tip)
}

/// Pill-shaped label, e.g. "Active" or a live reading.
pub fn badge(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        FontId::new(12.0, FontFamily::Name(MEDIUM.into())),
        fg,
    );
    let size = galley.size() + Vec2::new(16.0, 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, rect.height() / 2.0, bg);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// M3 segmented button: one option out of a few, the selected one checked.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let p = theme::current(ui.ctx());
    let mut changed = false;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        {
            let n = options.len();
            let mut rects = Vec::new();
            // In a right-to-left row add the last option first, so the
            // segments still read left to right.
            let mut order: Vec<usize> = (0..n).collect();
            if ui.layout().prefer_right_to_left() {
                order.reverse();
            }
            for k in order {
                let (v, label) = options[k];
                let on = *value == v;
                let text = if on {
                    medium(format!("{}  {label}", crate::icons::CHECK))
                } else {
                    RichText::new(label)
                };
                let r = 20;
                let radius = CornerRadius {
                    nw: if k == 0 { r } else { 0 },
                    sw: if k == 0 { r } else { 0 },
                    ne: if k + 1 == n { r } else { 0 },
                    se: if k + 1 == n { r } else { 0 },
                };
                let b = Button::new(text)
                    .fill(if on {
                        p.accent_soft
                    } else {
                        Color32::TRANSPARENT
                    })
                    .frame_when_inactive(on)
                    .corner_radius(radius)
                    .min_size(Vec2::new(96.0, 40.0));
                let resp = ui.add(b);
                if resp.clicked() && !on {
                    *value = v;
                    changed = true;
                }
                rects.push(resp.rect);
            }
            // Outline around the group and between segments.
            rects.sort_by(|a, b| a.left().total_cmp(&b.left()));
            let all = rects.iter().fold(rects[0], |a, r| a.union(*r));
            let s = Stroke::new(1.0, p.outline);
            ui.painter()
                .rect_stroke(all, 20.0, s, egui::StrokeKind::Inside);
            for r in &rects[1..] {
                ui.painter().vline(r.left(), r.y_range(), s);
            }
        }
    });
    changed
}

/// M3 switch: outlined track with a small thumb when off, filled track with
/// a large thumb when on.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let size = Vec2::new(52.0, 32.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *on, ""));
    if ui.is_rect_visible(rect) {
        let p = theme::current(ui.ctx());
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let painter = ui.painter();
        let radius = rect.height() / 2.0;
        let track = lerp_color(p.raised, p.accent_bg, t);
        painter.rect_filled(rect, radius, track);
        if t < 1.0 {
            let outline = Color32::from_rgba_unmultiplied(
                p.outline.r(),
                p.outline.g(),
                p.outline.b(),
                ((1.0 - t) * 255.0) as u8,
            );
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(2.0, outline),
                egui::StrokeKind::Inside,
            );
        }
        let pressed = resp.is_pointer_button_down_on();
        let r = if pressed {
            14.0
        } else {
            egui::lerp(8.0..=12.0, t)
        };
        let x = egui::lerp((rect.left() + 16.0)..=(rect.right() - 16.0), t);
        let c = egui::pos2(x, rect.center().y);
        if resp.hovered() {
            painter.circle_filled(c, 20.0, p.text.gamma_multiply(0.08));
        }
        painter.circle_filled(c, r, lerp_color(p.outline, p.on_accent, t));
    }
    resp
}

/// M3 slider: rounded track, primary active part and a round primary handle.
/// `log` spreads values logarithmically (needs `min > 0`).
pub fn slider(ui: &mut Ui, v: &mut f32, min: f32, max: f32, log: bool) -> Response {
    let log = log && min > 0.0;
    let w = ui.spacing().slider_width;
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, 32.0), Sense::click_and_drag());
    let r = 10.0;
    let (x0, x1) = (rect.left() + r, rect.right() - r);
    if let Some(pos) = resp.interact_pointer_pos() {
        let t = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0);
        let nv = if log {
            min * (max / min).powf(t)
        } else {
            min + (max - min) * t
        };
        if nv != *v {
            *v = nv;
            resp.mark_changed();
        }
    }
    if ui.is_rect_visible(rect) {
        let p = theme::current(ui.ctx());
        let t = if log {
            (v.max(min) / min).ln() / (max / min).ln()
        } else {
            (*v - min) / (max - min)
        }
        .clamp(0.0, 1.0);
        let c = egui::pos2(egui::lerp(x0..=x1, t), rect.center().y);
        let track = egui::Rect::from_x_y_ranges(rect.x_range(), (c.y - 3.0)..=(c.y + 3.0));
        let painter = ui.painter();
        painter.rect_filled(track, 3.0, p.raised);
        painter.rect_filled(track.with_max_x(c.x), 3.0, p.accent_bg);
        if resp.hovered() || resp.dragged() {
            painter.circle_filled(c, 18.0, p.text.gamma_multiply(0.10));
        }
        painter.circle_filled(c, r, p.accent_bg);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Converts a linear peak to a 0..1 meter position over a -60..0 dBFS scale.
pub fn meter_pos(peak: f32) -> f32 {
    let db = 20.0 * peak.max(1e-6).log10();
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

const METER_LABEL: f32 = 64.0;
const METER_VALUE: f32 = 78.0;
const METER_SEGMENTS: usize = 48;

/// Segmented LED meters sharing one dB scale. Levels are smoothed linear peaks.
/// Level meters: rows of (label, level, peak hold), all linear.
pub fn meters(ui: &mut Ui, rows: &[(&str, f32, f32)]) {
    let p = theme::current(ui.ctx());
    let bar_w = (ui.available_width() - METER_LABEL - METER_VALUE).max(80.0);
    for &(label, level, hold) in rows {
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(METER_LABEL, 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(METER_LABEL);
                    ui.label(RichText::new(label).weak());
                },
            );
            let (rect, _) = ui.allocate_exact_size(Vec2::new(bar_w, 14.0), Sense::hover());
            let lit = (meter_pos(level) * METER_SEGMENTS as f32).round() as usize;
            let gap = 2.0;
            let seg_w = (rect.width() - gap * (METER_SEGMENTS - 1) as f32) / METER_SEGMENTS as f32;
            for k in 0..METER_SEGMENTS {
                let x = rect.left() + k as f32 * (seg_w + gap);
                let r =
                    egui::Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(seg_w, 14.0));
                let db = (k as f32 + 1.0) / METER_SEGMENTS as f32 * 60.0 - 60.0;
                // Monochrome: only clipping gets a color.
                let color = if db > -3.0 { p.error } else { p.accent_bg };
                let fill = if k < lit { color } else { p.meter_track };
                ui.painter().rect_filled(r, 2.0, fill);
            }
            // Peak hold: one segment, solid even when above the live level.
            let k = (meter_pos(hold) * METER_SEGMENTS as f32).round() as usize;
            if k > 0 {
                let x = rect.left() + (k - 1) as f32 * (seg_w + gap);
                let r =
                    egui::Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(seg_w, 14.0));
                let clip = k as f32 / METER_SEGMENTS as f32 * 60.0 - 60.0 > -3.0;
                ui.painter()
                    .rect_filled(r, 2.0, if clip { p.error } else { p.text });
            }
            let db = 20.0 * hold.max(1e-6).log10();
            let text = if db <= -60.0 {
                "−∞ dB".to_string()
            } else {
                format!("{db:.1} dB")
            };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(text).font(FontId::monospace(13.0)));
            });
        });
    }
    // Scale under the bars.
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 14.0), Sense::hover());
    let left = rect.left() + METER_LABEL + ui.spacing().item_spacing.x;
    for db in [-60, -48, -36, -24, -12, -6, 0] {
        let x = left + bar_w * (db as f32 + 60.0) / 60.0;
        let align = match db {
            -60 => Align2::LEFT_TOP,
            0 => Align2::RIGHT_TOP,
            _ => Align2::CENTER_TOP,
        };
        ui.painter().text(
            egui::pos2(x, rect.top()),
            align,
            db.to_string(),
            FontId::proportional(10.5),
            p.text_weak,
        );
    }
}

/// Status dot with a soft halo.
pub fn dot(ui: &mut Ui, color: Color32, r: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(r * 3.0), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), r * 1.5, color.gamma_multiply(0.25));
    ui.painter().circle_filled(rect.center(), r, color);
}

#[cfg(test)]
mod tests {
    #[test]
    fn meter_scale() {
        assert_eq!(super::meter_pos(1.0), 1.0);
        assert_eq!(super::meter_pos(0.0), 0.0);
        assert!((super::meter_pos(0.001) - 0.0).abs() < 1e-6);
        assert!((super::meter_pos(0.0316) - 0.5).abs() < 0.01);
    }
}
