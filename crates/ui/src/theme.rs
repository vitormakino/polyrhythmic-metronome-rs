//! Colors, fonts and the painted primitives (card, glow, top accent) that give
//! the native UI the look of the web app: dark zinc, orange accent, glowing cards.

use egui::epaint::{Mesh, Shadow};
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Painter, Pos2, Rect,
    Stroke, StrokeKind, Visuals,
};
use std::sync::Arc;

/// zinc-950, window background.
pub const BG: Color32 = Color32::from_rgb(0x09, 0x09, 0x0b);
/// zinc-900, card background.
pub const CARD: Color32 = Color32::from_rgb(0x18, 0x18, 0x1b);
/// zinc-800, card and widget borders.
pub const BORDER: Color32 = Color32::from_rgb(0x27, 0x27, 0x2a);
/// zinc-700, inactive beat dot.
pub const DOT_INACTIVE: Color32 = Color32::from_rgb(0x3f, 0x3f, 0x46);
/// zinc-400, secondary text.
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xa1, 0xa1, 0xaa);
/// zinc-500, labels and idle icons.
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x71, 0x71, 0x7a);
/// zinc-600, faint captions.
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x52, 0x52, 0x5b);
/// zinc-100, primary text.
pub const TEXT: Color32 = Color32::from_rgb(0xf4, 0xf4, 0xf5);
/// orange-500.
pub const ACCENT: Color32 = Color32::from_rgb(0xf9, 0x73, 0x16);
/// orange-300, end of the top accent gradient.
pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(0xfd, 0xba, 0x74);

/// Pulse colors, cyclic by pulse index.
pub const PALETTE: [Color32; 6] = [
    Color32::from_rgb(0xf9, 0x73, 0x16),
    Color32::from_rgb(0x3b, 0x82, 0xf6),
    Color32::from_rgb(0x10, 0xb9, 0x81),
    Color32::from_rgb(0xa8, 0x55, 0xf7),
    Color32::from_rgb(0xf4, 0x3f, 0x5e),
    Color32::from_rgb(0xea, 0xb3, 0x08),
];

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

fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
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
    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_fill = CARD;
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.hovered.bg_fill = BORDER;
    v.widgets.hovered.weak_bg_fill = BORDER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, DOT_INACTIVE);
    v.widgets.active.bg_fill = DOT_INACTIVE;
    v.widgets.active.weak_bg_fill = DOT_INACTIVE;
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    v
}

/// Install fonts and the dark visuals. Call once at startup.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_visuals(visuals());
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

/// Card with the web look: zinc-900 fill, 1 px border, rounded-3xl, a dark
/// drop shadow, and a colored halo (`0 0 20px color33`) scaled by `glow` (0..=1).
/// `border` is also the glow color.
pub fn paint_card(painter: &Painter, rect: Rect, border: Color32, glow: f32) {
    paint_card_with_radius(painter, rect, border, glow, CARD_RADIUS);
}

/// [`paint_card`] with another corner radius (pulse cards use 16, `rounded-2xl`).
pub fn paint_card_with_radius(
    painter: &Painter,
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
            color: Color32::from_black_alpha(90),
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
    painter.rect_filled(rect, radius, CARD);
    let stroke_color = if glow > 0.0 {
        BORDER.lerp_to_gamma(border, glow)
    } else {
        BORDER
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

/// 4 px orange-500 to orange-300 gradient line along the top edge of a card
/// (inside its 1 px border), at 50 % opacity, clipped to the card's rounded
/// top corners like the web's `overflow-hidden`.
pub fn paint_top_accent(painter: &Painter, rect: Rect) {
    let inner = rect.shrink(1.0);
    let band = Rect::from_min_size(inner.min, egui::vec2(inner.width(), 4.0));
    let left = with_alpha(ACCENT, 0.5);
    let right = with_alpha(ACCENT_LIGHT, 0.5);
    paint_clipped_band(painter, inner, f32::from(CARD_RADIUS) - 1.0, band, |x| {
        left.lerp_to_gamma(right, (x - inner.min.x) / inner.width())
    });
}
