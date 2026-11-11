//! Small custom widgets: switch, level meter, cards, and icon text.

use crate::theme::{self, MEDIUM};
use egui::{
    Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Response, RichText, Sense, Stroke,
    Ui, Vec2, WidgetInfo,
};

/// Icon glyph at the given size, in the current text color unless styled further.
pub fn icon(glyph: &str, size: f32) -> RichText {
    RichText::new(glyph).size(size)
}

/// Icon followed by a label, for buttons and menu entries.
pub fn icon_text(glyph: &str, label: &str) -> String {
    format!("{glyph}  {label}")
}

pub fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).heading());
    ui.add_space(4.0);
}

/// Small section title above a card, in the style of libadwaita preference groups.
pub fn section(ui: &mut Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(
        RichText::new(text)
            .family(FontFamily::Name(MEDIUM.into()))
            .size(14.0),
    );
    ui.add_space(2.0);
}

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = theme::current(ui.ctx());
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Row with a title (and optional subtitle) on the left and controls on the right.
pub fn row(ui: &mut Ui, title: &str, subtitle: Option<&str>, controls: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(36.0);
        ui.vertical(|ui| {
            ui.add_space(2.0);
            ui.label(title);
            if let Some(s) = subtitle {
                ui.label(RichText::new(s).small().weak());
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), controls);
    });
}

/// libadwaita-style toggle switch.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let size = Vec2::new(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *on, ""));
    if ui.is_rect_visible(rect) {
        let p = theme::current(ui.ctx());
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let off = ui.visuals().widgets.inactive.bg_fill;
        let track = lerp_color(off, p.accent_bg, t);
        let track = if resp.hovered() {
            track.gamma_multiply(0.9)
        } else {
            track
        };
        let radius = rect.height() / 2.0;
        ui.painter().rect_filled(rect, radius, track);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        ui.painter()
            .circle_filled(egui::pos2(x, rect.center().y), radius - 3.0, Color32::WHITE);
    }
    resp
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

/// Horizontal level meter. `level` is a smoothed linear peak.
pub fn meter(ui: &mut Ui, label: &str, level: f32) {
    let p = theme::current(ui.ctx());
    ui.horizontal(|ui| {
        ui.add_sized([56.0, 18.0], egui::Label::new(RichText::new(label).weak()));
        let width = (ui.available_width() - 72.0).max(60.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 10.0), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 5.0, p.meter_track);
        let pos = meter_pos(level);
        if pos > 0.0 {
            let mut fill = rect;
            fill.set_width(rect.width() * pos);
            let db = pos * 60.0 - 60.0;
            let color = if db > -3.0 {
                p.error
            } else if db > -12.0 {
                p.warning
            } else {
                p.accent_bg
            };
            painter.rect_filled(fill, 5.0, color);
        }
        let db = 20.0 * level.max(1e-6).log10();
        let text = if db <= -60.0 {
            "-inf dB".to_string()
        } else {
            format!("{db:.1} dB")
        };
        ui.add_sized(
            [64.0, 18.0],
            egui::Label::new(RichText::new(text).font(FontId::monospace(12.0)).weak()),
        );
    });
}

/// Colored status dot followed by a label.
pub fn status(ui: &mut Ui, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.5, color);
        ui.label(text);
    });
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
