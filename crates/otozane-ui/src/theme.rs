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

/// Colors loosely following the libadwaita palette.
#[derive(Clone, Copy)]
pub struct Palette {
    pub window: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub accent: Color32,
    pub accent_bg: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub meter_track: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            window: Color32::from_rgb(0x22, 0x22, 0x26),
            sidebar: Color32::from_rgb(0x2e, 0x2e, 0x32),
            card: Color32::from_rgb(0x30, 0x30, 0x35),
            border: Color32::from_rgb(0x3d, 0x3d, 0x42),
            text: Color32::from_rgb(0xf6, 0xf6, 0xf7),
            text_weak: Color32::from_rgb(0xa0, 0xa0, 0xa6),
            accent: Color32::from_rgb(0x78, 0xae, 0xed),
            accent_bg: Color32::from_rgb(0x35, 0x84, 0xe4),
            success: Color32::from_rgb(0x8f, 0xf0, 0xa4),
            warning: Color32::from_rgb(0xf8, 0xe4, 0x5c),
            error: Color32::from_rgb(0xff, 0x7b, 0x63),
            meter_track: Color32::from_rgb(0x3a, 0x3a, 0x3f),
        }
    } else {
        Palette {
            window: Color32::from_rgb(0xfa, 0xfa, 0xfb),
            sidebar: Color32::from_rgb(0xeb, 0xeb, 0xed),
            card: Color32::WHITE,
            border: Color32::from_rgb(0xdd, 0xdd, 0xe0),
            text: Color32::from_rgb(0x1e, 0x1e, 0x22),
            text_weak: Color32::from_rgb(0x6c, 0x6c, 0x73),
            accent: Color32::from_rgb(0x1c, 0x71, 0xd8),
            accent_bg: Color32::from_rgb(0x35, 0x84, 0xe4),
            success: Color32::from_rgb(0x1b, 0x8a, 0x3c),
            warning: Color32::from_rgb(0x9c, 0x6e, 0x03),
            error: Color32::from_rgb(0xc0, 0x1c, 0x28),
            meter_track: Color32::from_rgb(0xe3, 0xe3, 0xe6),
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
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.extreme_bg_color = if dark {
        Color32::from_rgb(0x1d, 0x1d, 0x20)
    } else {
        Color32::WHITE
    };
    v.faint_bg_color = p.sidebar;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent_bg;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.error;
    v.slider_trailing_fill = true;
    v.override_text_color = None;
    let (hover, active) = if dark {
        (
            Color32::from_rgb(0x45, 0x45, 0x4b),
            Color32::from_rgb(0x52, 0x52, 0x59),
        )
    } else {
        (
            Color32::from_rgb(0xe2, 0xe2, 0xe6),
            Color32::from_rgb(0xd4, 0xd4, 0xd9),
        )
    };
    let base = if dark {
        Color32::from_rgb(0x3a, 0x3a, 0x40)
    } else {
        Color32::from_rgb(0xec, 0xec, 0xef)
    };
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
    s.spacing.item_spacing = Vec2::new(8.0, 8.0);
    s.spacing.button_padding = Vec2::new(12.0, 6.0);
    s.spacing.interact_size = Vec2::new(36.0, 30.0);
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
            FontId::new(22.0, FontFamily::Name(MEDIUM.into())),
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
