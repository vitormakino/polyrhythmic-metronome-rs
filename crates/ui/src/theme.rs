//! Colors, fonts and the painted primitives (card, glow, top accent) that give
//! the native UI the look of the web app: a theme of roles (background, card,
//! text, accent, pulse colors) and glowing cards painted from it.

use egui::epaint::{Mesh, Shadow};
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Painter, Pos2, Rect,
    Stroke, StrokeKind, Visuals,
};
use std::sync::Arc;

use crate::palette::{Mode, Palette};

/// Pulse colors of the v1 look, cyclic by pulse index. Stored pulse colors
/// equal to one of these follow their slot in the active theme.
const V1_PULSES: [Color32; 6] = [
    Color32::from_rgb(0xf9, 0x73, 0x16),
    Color32::from_rgb(0x3b, 0x82, 0xf6),
    Color32::from_rgb(0x10, 0xb9, 0x81),
    Color32::from_rgb(0xa8, 0x55, 0xf7),
    Color32::from_rgb(0xf4, 0x3f, 0x5e),
    Color32::from_rgb(0xea, 0xb3, 0x08),
];

/// Every color the views paint with, by role.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub mode: Mode,
    /// Window background.
    pub bg: Color32,
    /// Card background.
    pub card: Color32,
    /// Card and widget borders.
    pub border: Color32,
    /// Inactive beat dot.
    pub dot_inactive: Color32,
    /// Primary text.
    pub text: Color32,
    /// Secondary text.
    pub text_dim: Color32,
    /// Labels and idle icons.
    pub text_muted: Color32,
    /// Faint captions.
    pub text_faint: Color32,
    /// Text between primary and secondary.
    pub text_soft: Color32,
    /// Text of a hovered control.
    pub text_hot: Color32,
    /// Fill of a hovered control.
    pub hover_fill: Color32,
    pub accent: Color32,
    /// End of the top accent gradient: lighter than `accent` in dark themes,
    /// darker in light ones.
    pub accent_light: Color32,
    /// Strongest foreground: BPM digits, topology light, selected number.
    pub strong: Color32,
    /// Text on an accent-filled button.
    pub on_accent: Color32,
    /// Card drop shadow.
    pub shadow: Color32,
    /// Dimming layer behind overlays.
    pub overlay: Color32,
    /// Pulse colors, cyclic by pulse index.
    pub pulses: [Color32; 6],
}

/// The v1 look, exactly: dark zinc with an orange accent.
pub const DARK: Theme = Theme {
    mode: Mode::Dark,
    bg: Color32::from_rgb(0x09, 0x09, 0x0b),
    card: Color32::from_rgb(0x18, 0x18, 0x1b),
    border: Color32::from_rgb(0x27, 0x27, 0x2a),
    dot_inactive: Color32::from_rgb(0x3f, 0x3f, 0x46),
    text: Color32::from_rgb(0xf4, 0xf4, 0xf5),
    text_dim: Color32::from_rgb(0xa1, 0xa1, 0xaa),
    text_muted: Color32::from_rgb(0x71, 0x71, 0x7a),
    text_faint: Color32::from_rgb(0x52, 0x52, 0x5b),
    text_soft: Color32::from_rgb(0xd4, 0xd4, 0xd8),
    text_hot: Color32::from_rgb(0xe4, 0xe4, 0xe7),
    hover_fill: Color32::from_rgb(0x2f, 0x2f, 0x33),
    accent: Color32::from_rgb(0xf9, 0x73, 0x16),
    accent_light: Color32::from_rgb(0xfd, 0xba, 0x74),
    strong: Color32::WHITE,
    on_accent: Color32::WHITE,
    shadow: Color32::from_black_alpha(90),
    overlay: Color32::from_black_alpha(51),
    pulses: V1_PULSES,
};

/// WCAG relative luminance of an sRGB color (0 black, 1 white).
fn relative_luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio between two colors (1 identical, 21 black on white).
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

impl Theme {
    /// Derives every role from an Omarchy-style palette.
    pub fn from_palette(p: &Palette) -> Theme {
        let mix = |a: Color32, b: Color32, t: f32| a.lerp_to_gamma(b, t);
        let dark = p.mode == Mode::Dark;
        Theme {
            mode: p.mode,
            bg: p.background,
            card: p.lighter_background,
            border: p.selection,
            dot_inactive: p.muted,
            text: p.foreground,
            text_soft: mix(p.foreground, p.background, 0.15),
            text_hot: mix(p.foreground, p.background, 0.07),
            text_dim: mix(p.foreground, p.background, 0.35),
            text_muted: p.dark_foreground,
            text_faint: mix(p.dark_foreground, p.background, 0.3),
            hover_fill: mix(p.selection, p.muted, 0.4),
            accent: p.accent,
            accent_light: mix(p.accent, p.bright_foreground, 0.4),
            strong: p.bright_foreground,
            on_accent: if contrast_ratio(Color32::WHITE, p.accent) >= 3.0 {
                Color32::WHITE
            } else if relative_luminance(p.background) <= relative_luminance(p.foreground) {
                p.background
            } else {
                p.foreground
            },
            shadow: Color32::from_black_alpha(if dark { 90 } else { 30 }),
            overlay: Color32::from_black_alpha(if dark { 51 } else { 20 }),
            pulses: [p.orange, p.blue, p.green, p.magenta, p.red, p.yellow],
        }
    }

    /// The embedded theme called `name`, if any.
    pub fn builtin(name: &str) -> Option<Theme> {
        match name.to_ascii_lowercase().as_str() {
            "default" => Some(DARK),
            "light" => Some(Theme::from_palette(&Palette::LIGHT)),
            _ => None,
        }
    }

    /// The color to paint a pulse whose stored color is `stored`: a color that
    /// came from the v1 palette follows its slot in this theme.
    pub fn pulse_color(&self, stored: [u8; 3]) -> Color32 {
        V1_PULSES
            .iter()
            .position(|c| [c.r(), c.g(), c.b()] == stored)
            .map_or(Color32::from_rgb(stored[0], stored[1], stored[2]), |i| {
                self.pulses[i]
            })
    }

    /// The egui visuals (panels, windows, widgets) for this theme.
    pub fn visuals(&self) -> Visuals {
        let mut v = match self.mode {
            Mode::Dark => Visuals::dark(),
            Mode::Light => Visuals::light(),
        };
        v.panel_fill = self.bg;
        v.window_fill = self.card;
        v.window_stroke = Stroke::new(1.0, self.border);
        v.extreme_bg_color = self.bg;
        v.faint_bg_color = self.card;
        v.override_text_color = Some(self.text);
        v.hyperlink_color = self.accent;
        v.selection.bg_fill = self.accent.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, self.accent);
        let radius = CornerRadius::same(12);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = radius;
        }
        v.widgets.noninteractive.bg_fill = self.card;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        v.widgets.inactive.bg_fill = self.card;
        v.widgets.inactive.weak_bg_fill = self.card;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, self.border);
        v.widgets.hovered.bg_fill = self.border;
        v.widgets.hovered.weak_bg_fill = self.border;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, self.dot_inactive);
        v.widgets.active.bg_fill = self.dot_inactive;
        v.widgets.active.weak_bg_fill = self.dot_inactive;
        v.widgets.active.bg_stroke = Stroke::new(1.0, self.accent);
        v
    }
}

/// Corner radius of cards (Tailwind `rounded-3xl`).
pub const CARD_RADIUS: u8 = 24;
const MAX_GLOW_LAYERS: usize = 8;

const FAMILY_LIGHT: &str = "Inter-Light";
const FAMILY_SEMIBOLD: &str = "Inter-SemiBold";
const FAMILY_BOLD: &str = "Inter-Bold";
const FAMILY_MONO_MEDIUM: &str = "JetBrainsMono-Medium";

/// Inter Light (300).
pub fn font_light(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_LIGHT.into()))
}
/// Inter Regular (400).
pub fn font_regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
/// Inter SemiBold (600).
pub fn font_semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_SEMIBOLD.into()))
}
/// Inter Bold (700).
pub fn font_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_BOLD.into()))
}
/// JetBrains Mono Regular (400).
pub fn font_mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}
/// JetBrains Mono Medium (500).
pub fn font_mono_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_MONO_MEDIUM.into()))
}

fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let files: [(&str, &[u8]); 6] = [
        (
            "Inter-Light",
            include_bytes!("../assets/fonts/Inter-Light.ttf"),
        ),
        (
            "Inter-Regular",
            include_bytes!("../assets/fonts/Inter-Regular.ttf"),
        ),
        (
            "Inter-SemiBold",
            include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
        ),
        (
            "Inter-Bold",
            include_bytes!("../assets/fonts/Inter-Bold.ttf"),
        ),
        (
            "JetBrainsMono-Regular",
            include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
        ),
        (
            "JetBrainsMono-Medium",
            include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"),
        ),
    ];
    for (name, bytes) in files {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    // Defaults stay behind ours as glyph fallbacks (symbols, emoji).
    let prepend = |fonts: &mut FontDefinitions, family: FontFamily, name: &str| {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, name.to_owned());
    };
    prepend(&mut fonts, FontFamily::Proportional, "Inter-Regular");
    prepend(&mut fonts, FontFamily::Monospace, "JetBrainsMono-Regular");
    // egui has no weight axis: every weight is its own named family.
    for (family, name) in [
        (FAMILY_LIGHT, "Inter-Light"),
        (FAMILY_SEMIBOLD, "Inter-SemiBold"),
        (FAMILY_BOLD, "Inter-Bold"),
        (FAMILY_MONO_MEDIUM, "JetBrainsMono-Medium"),
    ] {
        let mut chain = vec![name.to_owned()];
        chain.extend(
            fonts.families[&FontFamily::Proportional]
                .iter()
                .filter(|f| *f != "Inter-Regular")
                .cloned(),
        );
        fonts
            .families
            .insert(FontFamily::Name(family.into()), chain);
    }
    fonts
}

/// Makes `theme` the egui visuals of `ctx`, whatever the OS appearance is.
/// egui keeps one style per dark/light slot and follows the system, so the
/// visuals go into both slots: a switch of the OS appearance never brings
/// back stock visuals. The theme preference is left alone (pinning it to
/// the theme's mode would change how egui renders text and move every
/// light-theme snapshot).
pub fn apply(ctx: &egui::Context, theme: &Theme) {
    let visuals = theme.visuals();
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);
}

/// Install fonts and the default theme's visuals. Call once at startup.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    apply(ctx, &DARK);
}

/// `color` with its alpha replaced by `alpha` (0..=1).
pub fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    let [r, g, b, _] = color.to_array();
    Color32::from_rgba_unmultiplied(r, g, b, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Number of concentric layers `paint_glow` draws for `intensity` (0 means none, never more than 8).
pub fn glow_layer_count(intensity: f32) -> usize {
    if intensity.is_nan() || intensity <= 0.0 {
        0
    } else {
        MAX_GLOW_LAYERS
    }
}

/// Card with the web look: theme card fill, 1 px theme border, rounded-3xl, a
/// drop shadow, and a colored halo (`0 0 20px color33`) scaled by `glow` (0..=1).
/// `border` is also the glow color.
pub fn paint_card(painter: &Painter, theme: &Theme, rect: Rect, border: Color32, glow: f32) {
    paint_card_with_radius(painter, theme, rect, border, glow, CARD_RADIUS);
}

/// [`paint_card`] with another corner radius (pulse cards use 16, `rounded-2xl`).
pub fn paint_card_with_radius(
    painter: &Painter,
    theme: &Theme,
    rect: Rect,
    border: Color32,
    glow: f32,
    radius: u8,
) {
    let radius = CornerRadius::same(radius);
    let glow = if glow.is_nan() {
        0.0
    } else {
        glow.clamp(0.0, 1.0)
    };
    // shadow-2xl
    painter.add(
        Shadow {
            offset: [0, 10],
            blur: 40,
            spread: 0,
            color: theme.shadow,
        }
        .as_shape(rect, radius),
    );
    if glow > 0.0 {
        painter.add(
            Shadow {
                offset: [0, 0],
                blur: 20,
                spread: 2,
                color: with_alpha(border, 0.2 * glow),
            }
            .as_shape(rect, radius),
        );
    }
    painter.rect_filled(rect, radius, theme.card);
    let stroke_color = if glow > 0.0 {
        theme.border.lerp_to_gamma(border, glow)
    } else {
        theme.border
    };
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, stroke_color),
        StrokeKind::Inside,
    );
}

/// Soft halo around a point: up to 8 concentric discs with decreasing alpha.
/// `radius` is the outer radius of the halo, `intensity` (0..=1) scales alpha.
pub fn paint_glow(painter: &Painter, center: Pos2, radius: f32, color: Color32, intensity: f32) {
    let layers = glow_layer_count(intensity);
    if layers == 0 || radius <= 0.0 {
        return;
    }
    let intensity = intensity.clamp(0.0, 1.0);
    for i in 0..layers {
        let t = i as f32 / layers as f32; // 0 = outermost
        let r = radius * (1.0 - t * 0.7);
        // Overlapping discs accumulate towards the center.
        let alpha = 0.10 * intensity;
        painter.circle_filled(center, r, with_alpha(color, alpha));
    }
}

/// Horizontal inset of a rounded rect's outline at height `y`: how far the
/// curved corner pulls the edge in, 0 away from the corners.
fn corner_inset(rect: Rect, radius: f32, y: f32) -> f32 {
    let dist = (y - rect.min.y).min(rect.max.y - y).max(0.0);
    if dist >= radius {
        return 0.0;
    }
    let d = radius - dist;
    radius - (radius * radius - d * d).max(0.0).sqrt()
}

/// Fills `band` (a horizontal strip, e.g. a bar along a card edge) where it
/// lies inside the rounded rect `card` (corner `radius`), with per-x color
/// `color_at`. Stands in for CSS `overflow-hidden` on rounded cards.
fn paint_clipped_band(
    painter: &Painter,
    card: Rect,
    radius: f32,
    band: Rect,
    color_at: impl Fn(f32) -> Color32,
) {
    const ROWS: usize = 8;
    let radius = radius.min(card.width() * 0.5).min(card.height() * 0.5);
    let mut mesh = Mesh::default();
    for i in 0..=ROWS {
        let y = band.min.y + band.height() * i as f32 / ROWS as f32;
        let inset = corner_inset(card, radius, y);
        let xl = band.min.x.max(card.min.x + inset);
        let xr = band.max.x.min(card.max.x - inset).max(xl);
        mesh.colored_vertex(Pos2::new(xl, y), color_at(xl));
        mesh.colored_vertex(Pos2::new(xr, y), color_at(xr));
        if i > 0 {
            let b = (2 * i) as u32;
            mesh.add_triangle(b - 2, b - 1, b + 1);
            mesh.add_triangle(b - 2, b + 1, b);
        }
    }
    painter.add(mesh);
}

/// Fills `band` with `color`, clipped to the rounded rect `card`.
pub fn paint_band(painter: &Painter, card: Rect, radius: f32, band: Rect, color: Color32) {
    paint_clipped_band(painter, card, radius, band, |_| color);
}

/// 4 px gradient line, `accent` to `accent_light`, along the top edge of a card
/// (inside its 1 px border), at 50 % opacity, clipped to the card's rounded
/// top corners like the web's `overflow-hidden`.
pub fn paint_top_accent(painter: &Painter, theme: &Theme, rect: Rect) {
    let inner = rect.shrink(1.0);
    let band = Rect::from_min_size(inner.min, egui::vec2(inner.width(), 4.0));
    let left = with_alpha(theme.accent, 0.5);
    let right = with_alpha(theme.accent_light, 0.5);
    paint_clipped_band(painter, inner, f32::from(CARD_RADIUS) - 1.0, band, |x| {
        left.lerp_to_gamma(right, (x - inner.min.x) / inner.width())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_wins_over_the_system_theme() {
        let light = Theme::builtin("light").unwrap();
        for theme in [&DARK, &light] {
            for before in [egui::Theme::Dark, egui::Theme::Light] {
                let ctx = egui::Context::default();
                ctx.set_theme(before);
                apply(&ctx, theme);
                // The OS appearance may flip at any time: whichever slot
                // egui follows, the theme shows.
                for system in [egui::Theme::Dark, egui::Theme::Light] {
                    ctx.set_theme(system);
                    assert_eq!(
                        ctx.global_style().visuals.panel_fill,
                        theme.bg,
                        "{:?} theme, {before:?} then {system:?}",
                        theme.mode
                    );
                }
            }
        }
    }

    #[test]
    fn dark_keeps_the_v1_colors() {
        let rgb = Color32::from_rgb;
        assert_eq!(DARK.mode, Mode::Dark);
        assert_eq!(DARK.bg, rgb(0x09, 0x09, 0x0b));
        assert_eq!(DARK.card, rgb(0x18, 0x18, 0x1b));
        assert_eq!(DARK.border, rgb(0x27, 0x27, 0x2a));
        assert_eq!(DARK.dot_inactive, rgb(0x3f, 0x3f, 0x46));
        assert_eq!(DARK.text, rgb(0xf4, 0xf4, 0xf5));
        assert_eq!(DARK.text_dim, rgb(0xa1, 0xa1, 0xaa));
        assert_eq!(DARK.text_muted, rgb(0x71, 0x71, 0x7a));
        assert_eq!(DARK.text_faint, rgb(0x52, 0x52, 0x5b));
        assert_eq!(DARK.text_soft, rgb(0xd4, 0xd4, 0xd8));
        assert_eq!(DARK.text_hot, rgb(0xe4, 0xe4, 0xe7));
        assert_eq!(DARK.hover_fill, rgb(0x2f, 0x2f, 0x33));
        assert_eq!(DARK.accent, rgb(0xf9, 0x73, 0x16));
        assert_eq!(DARK.accent_light, rgb(0xfd, 0xba, 0x74));
        assert_eq!(DARK.strong, Color32::WHITE);
        assert_eq!(DARK.on_accent, Color32::WHITE);
        assert_eq!(DARK.shadow, Color32::from_black_alpha(90));
        assert_eq!(DARK.overlay, Color32::from_black_alpha(51));
        assert_eq!(
            DARK.pulses,
            [
                rgb(0xf9, 0x73, 0x16),
                rgb(0x3b, 0x82, 0xf6),
                rgb(0x10, 0xb9, 0x81),
                rgb(0xa8, 0x55, 0xf7),
                rgb(0xf4, 0x3f, 0x5e),
                rgb(0xea, 0xb3, 0x08),
            ]
        );
    }

    #[test]
    fn from_palette_maps_roles() {
        let t = Theme::from_palette(&Palette::LIGHT);
        assert_eq!(t.bg, Palette::LIGHT.background);
        assert_eq!(t.card, Palette::LIGHT.lighter_background);
        assert_eq!(t.pulses[0], Palette::LIGHT.orange);
        assert_eq!(t.strong, Palette::LIGHT.bright_foreground);
        assert_eq!(t.mode, Mode::Light);
    }

    #[test]
    fn pulse_colors_follow_the_palette_slot() {
        let light = Theme::builtin("light").unwrap();
        assert_eq!(light.pulse_color([0xf9, 0x73, 0x16]), light.pulses[0]);
        assert_eq!(light.pulse_color([0x3b, 0x82, 0xf6]), light.pulses[1]);
        assert_eq!(light.pulse_color([1, 2, 3]), Color32::from_rgb(1, 2, 3));
        assert_eq!(DARK.pulse_color([0x3b, 0x82, 0xf6]), DARK.pulses[1]);
    }

    #[test]
    fn contrast_ratio_matches_wcag() {
        let c = contrast_ratio(Color32::WHITE, Color32::BLACK);
        assert!((c - 21.0).abs() < 0.01, "{c}");
        assert!((contrast_ratio(Color32::RED, Color32::RED) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn on_accent_stays_readable() {
        let mut p = Palette::DEFAULT;
        p.accent = Color32::from_rgb(0xff, 0xf0, 0x80); // light yellow accent
        assert_eq!(Theme::from_palette(&p).on_accent, p.background);
        // White is kept while it reaches 3:1 (light #ea580c: 3.56).
        let light = Theme::from_palette(&Palette::LIGHT);
        assert_eq!(light.on_accent, Color32::WHITE);
        // Orange #f97316 is too bright for white (2.8): the darker of
        // background and foreground takes over.
        let orange = Theme::from_palette(&Palette::DEFAULT);
        assert_eq!(orange.on_accent, Palette::DEFAULT.background);
        // Tokyo Night's blue accent #7aa2f7 gets dark text, readable.
        let text = include_str!("../tests/fixtures/tokyo-night.toml");
        let (tokyo, _) = crate::palette::parse(text, &Palette::DEFAULT).unwrap();
        let t = Theme::from_palette(&tokyo);
        assert_eq!(t.on_accent, tokyo.background);
        assert!(contrast_ratio(t.on_accent, t.accent) >= 4.5);
        // The built-in themes are always readable.
        for theme in [&light, &orange] {
            assert!(contrast_ratio(theme.on_accent, theme.accent) >= 3.0);
        }
    }

    #[test]
    fn builtin_names_ignore_case() {
        assert_eq!(Theme::builtin("Light"), Theme::builtin("light"));
        assert_eq!(Theme::builtin("DEFAULT"), Some(DARK));
    }

    #[test]
    fn builtins() {
        assert_eq!(Theme::builtin("default"), Some(DARK));
        assert!(Theme::builtin("light").is_some());
        assert!(Theme::builtin("omarchy").is_none());
    }
}
