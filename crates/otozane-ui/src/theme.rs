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

/// Monochrome Material 3 color roles. Field names describe where a color is
/// used; the M3 role each one maps to is noted next to it.
#[derive(Clone, Copy)]
pub struct Palette {
    /// surface
    pub window: Color32,
    /// surface-container-low (navigation drawer)
    pub sidebar: Color32,
    /// surface-container (filled cards)
    pub card: Color32,
    /// surface-container-highest (controls and tiles on a card)
    pub raised: Color32,
    /// outline-variant (dividers)
    pub border: Color32,
    /// outline (unselected switch and chip outlines)
    pub outline: Color32,
    /// on-surface
    pub text: Color32,
    /// on-surface-variant
    pub text_weak: Color32,
    /// primary as a text/icon color
    pub accent: Color32,
    /// primary as a fill
    pub accent_bg: Color32,
    /// on-primary
    pub on_accent: Color32,
    /// secondary-container (active indicator, tonal buttons, selected chips)
    pub accent_soft: Color32,
    /// inverse-surface / inverse-on-surface (snackbars)
    pub inverse: Color32,
    pub on_inverse: Color32,
    pub error: Color32,
    pub meter_track: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            window: rgb(0x121212),
            sidebar: rgb(0x181818),
            card: rgb(0x1f1f1f),
            raised: rgb(0x2c2c2c),
            border: rgb(0x2e2e2e),
            outline: rgb(0x8e8e8e),
            text: rgb(0xe6e6e6),
            text_weak: rgb(0xa6a6a6),
            accent: rgb(0xffffff),
            accent_bg: rgb(0xe6e6e6),
            on_accent: rgb(0x161616),
            accent_soft: rgb(0x3a3a3a),
            inverse: rgb(0xe6e6e6),
            on_inverse: rgb(0x2a2a2a),
            error: rgb(0xffb4ab),
            meter_track: rgb(0x2a2a2a),
        }
    } else {
        Palette {
            window: rgb(0xfafafa),
            sidebar: rgb(0xf2f2f2),
            card: rgb(0xeeeeee),
            raised: rgb(0xe2e2e2),
            border: rgb(0xdedede),
            outline: rgb(0x767676),
            text: rgb(0x1b1b1b),
            text_weak: rgb(0x5e5e5e),
            accent: rgb(0x111111),
            accent_bg: rgb(0x1f1f1f),
            on_accent: rgb(0xffffff),
            accent_soft: rgb(0xd9d9d9),
            inverse: rgb(0x2f2f2f),
            on_inverse: rgb(0xf2f2f2),
            error: rgb(0xba1a1a),
            meter_track: rgb(0xdedede),
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
    // M3 shapes: full (pill) for buttons and fields, large for menus.
    let full = CornerRadius::same(20);
    v.panel_fill = p.window;
    v.window_fill = p.card;
    v.window_stroke = Stroke::NONE;
    v.window_corner_radius = CornerRadius::same(16);
    v.menu_corner_radius = CornerRadius::same(12);
    v.extreme_bg_color = p.raised;
    v.text_edit_bg_color = Some(p.raised);
    v.faint_bg_color = p.sidebar;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent_bg;
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.popup_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 120 } else { 36 }),
    };
    v.window_shadow = v.popup_shadow;
    v.warn_fg_color = p.text;
    v.error_fg_color = p.error;
    v.slider_trailing_fill = true;
    v.override_text_color = None;
    // State layers: on-surface at 8% (hover) and 12% (pressed) over the container.
    let layer = |a: u8| {
        p.raised.blend(Color32::from_rgba_unmultiplied(
            p.text.r(),
            p.text.g(),
            p.text.b(),
            a,
        ))
    };
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.bg_fill = p.card;
    for (wv, fill) in [
        (&mut w.inactive, p.raised),
        (&mut w.hovered, layer(20)),
        (&mut w.active, layer(31)),
        (&mut w.open, layer(31)),
    ] {
        wv.bg_fill = fill;
        wv.weak_bg_fill = fill;
        wv.bg_stroke = Stroke::NONE;
        wv.corner_radius = full;
        wv.fg_stroke = Stroke::new(1.0, p.text);
        wv.expansion = 0.0;
    }
    w.noninteractive.corner_radius = CornerRadius::same(12);
    s.visuals = v;
    // App text is not a document: no I-beam cursor or selection on labels.
    s.interaction.selectable_labels = false;
    s.spacing.item_spacing = Vec2::new(8.0, 8.0);
    s.spacing.button_padding = Vec2::new(16.0, 9.0);
    s.spacing.interact_size = Vec2::new(40.0, 36.0);
    s.spacing.slider_rail_height = 6.0;
    s.spacing.slider_width = 220.0;
    s.spacing.menu_margin = Margin::same(8);
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
