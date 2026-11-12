//! GTK/libadwaita-inspired styling implemented with egui visuals.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Margin, Stroke, Style, Theme,
    Vec2,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

/// Layered graphite surfaces with the brand blue from the app icon.
#[derive(Clone, Copy)]
pub struct Palette {
    pub window: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    /// Controls and nested surfaces sitting on a card.
    pub raised: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub accent: Color32,
    pub accent_bg: Color32,
    /// Translucent accent for selected rows and badges.
    pub accent_soft: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub meter_track: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            window: rgb(0x141519),
            sidebar: rgb(0x1a1b20),
            card: rgb(0x1f2026),
            raised: rgb(0x2a2c33),
            border: rgb(0x2c2e36),
            text: rgb(0xececf1),
            text_weak: rgb(0x8d909b),
            accent: rgb(0x7cb0ff),
            accent_bg: rgb(0x3584e4),
            accent_soft: Color32::from_rgba_unmultiplied(0x35, 0x84, 0xe4, 0x2e),
            success: rgb(0x4ade80),
            warning: rgb(0xfbbf24),
            error: rgb(0xf87171),
            meter_track: rgb(0x272930),
        }
    } else {
        Palette {
            window: rgb(0xf3f4f7),
            sidebar: rgb(0xe9ebef),
            card: Color32::WHITE,
            raised: rgb(0xeef0f3),
            border: rgb(0xe0e3e8),
            text: rgb(0x16181d),
            text_weak: rgb(0x676b77),
            accent: rgb(0x1c6fd8),
            accent_bg: rgb(0x3584e4),
            accent_soft: Color32::from_rgba_unmultiplied(0x35, 0x84, 0xe4, 0x24),
            success: rgb(0x16a34a),
            warning: rgb(0xc27c0e),
            error: rgb(0xdc2626),
            meter_track: rgb(0xe7e9ee),
        }
    }
}

pub fn current(ctx: &egui::Context) -> Palette {
    palette(ctx.global_style().visuals.dark_mode)
}

pub const MEDIUM: &str = "medium";

fn fonts() -> FontDefinitions {
    let mut f = FontDefinitions::default();
    let add = |f: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        f.font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add(
        &mut f,
        "SpaceGrotesk",
        include_bytes!("../assets/fonts/SpaceGrotesk-Regular.ttf"),
    );
    add(
        &mut f,
        "SpaceGrotesk-Medium",
        include_bytes!("../assets/fonts/SpaceGrotesk-Medium.ttf"),
    );
    add(
        &mut f,
        "Lucide",
        include_bytes!("../assets/fonts/lucide-subset.ttf"),
    );
    // egui's default fonts stay as fallbacks (e.g. Cyrillic device names).
    let defaults = f.families[&FontFamily::Proportional].clone();
    let family = |primary: &str| {
        let mut v = vec![primary.to_string(), "Lucide".to_string()];
        v.extend(defaults.iter().cloned());
        v
    };
    f.families
        .insert(FontFamily::Proportional, family("SpaceGrotesk"));
    f.families.insert(
        FontFamily::Name(MEDIUM.into()),
        family("SpaceGrotesk-Medium"),
    );
    f.families
        .get_mut(&FontFamily::Monospace)
        .expect("default monospace family")
        .push("Lucide".into());
    f
}

fn style(dark: bool) -> Style {
    let p = palette(dark);
    let mut s = Style::default();
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let r8 = CornerRadius::same(8);
    v.panel_fill = p.window;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.extreme_bg_color = if dark { rgb(0x121317) } else { Color32::WHITE };
    v.faint_bg_color = p.sidebar;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent_bg;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 110 } else { 40 }),
    };
    v.window_shadow = v.popup_shadow;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.error;
    v.slider_trailing_fill = true;
    v.override_text_color = None;
    let (hover, active) = if dark {
        (rgb(0x33353d), rgb(0x3d3f48))
    } else {
        (rgb(0xe3e6eb), rgb(0xd6dae1))
    };
    let base = p.raised;
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.bg_fill = p.card;
    for (wv, fill) in [
        (&mut w.inactive, base),
        (&mut w.hovered, hover),
        (&mut w.active, active),
        (&mut w.open, active),
    ] {
        wv.bg_fill = fill;
        wv.weak_bg_fill = fill;
        wv.bg_stroke = Stroke::NONE;
        wv.corner_radius = r8;
        wv.fg_stroke = Stroke::new(1.0, p.text);
        wv.expansion = 0.0;
    }
    w.noninteractive.corner_radius = r8;
    s.visuals = v;
    // App text is not a document: no I-beam cursor or selection on labels.
    s.interaction.selectable_labels = false;
    s.spacing.item_spacing = Vec2::new(8.0, 8.0);
    s.spacing.button_padding = Vec2::new(12.0, 7.0);
    s.spacing.interact_size = Vec2::new(36.0, 32.0);
    s.spacing.slider_width = 220.0;
    s.spacing.menu_margin = Margin::same(6);
    s.spacing.combo_height = 300.0;
    use egui::{FontId, TextStyle};
    s.text_styles = [
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Body, FontId::proportional(14.5)),
        (TextStyle::Button, FontId::proportional(14.5)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
        (
            TextStyle::Heading,
            FontId::new(26.0, FontFamily::Name(MEDIUM.into())),
        ),
    ]
    .into();
    s
}

pub fn install(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_fonts(fonts());
    ctx.set_style_of(Theme::Light, style(false));
    ctx.set_style_of(Theme::Dark, style(true));
    apply(ctx, choice);
}

pub fn apply(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_theme(match choice {
        ThemeChoice::System => egui::ThemePreference::System,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::Dark => egui::ThemePreference::Dark,
    });
}
