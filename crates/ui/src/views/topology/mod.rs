//! Rhythm topology: the drawing that shows the pattern moving, one of
//! several designs ([`TopologyKind`]). Every design reads the same input,
//! a [`TopologyFrame`] built from [`UiState`], paints into the rectangle it is
//! given, and tells the layout its [`Shape`]. The coincidence and engine
//! lights sit in a strip under any visible design.

mod arcs;
mod circle;
pub mod fx;
mod strings;

use egui::{Align2, Color32, Rect, Response, Role, Sense, Ui, WidgetInfo, pos2, vec2};
use pm_core::PulseId;
use serde::{Deserialize, Serialize};

pub use arcs::{ArcsStyle, arcs_ball, arcs_radius};
pub use circle::{CircleStyle, RingGeom, dot_radius, polygon_visible, ring_gap, ring_layout};
pub use strings::{StringsStyle, cursor_x, displacement};

use super::widgets;
use crate::state::{Hit, UiState};
use crate::theme::{self, Theme};

/// Which drawing the topology shows. Persisted as snake case
/// (`"circle"`, `"strings"`, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyKind {
    /// Rings of dots with ripples and sparks.
    #[default]
    Circle,
    /// One plucked string per pulse.
    Strings,
    /// Half circles over a shared base line.
    Arcs,
    /// A clock: one orbit per pulse.
    Orbits,
    /// A clock: one hand sweeping the measure.
    Sweep,
    /// Live polygons, one per pulse.
    Polygons,
    /// No drawing at all.
    Hidden,
}

impl TopologyKind {
    /// Every kind, in the order the choice lists them.
    pub const ALL: [TopologyKind; 7] = [
        TopologyKind::Circle,
        TopologyKind::Strings,
        TopologyKind::Arcs,
        TopologyKind::Orbits,
        TopologyKind::Sweep,
        TopologyKind::Polygons,
        TopologyKind::Hidden,
    ];
}

/// The room a design asks the layout for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A square.
    Square,
    /// A fixed width : height ratio.
    Aspect(f32),
    /// A low band of this height (pt), as wide as available.
    Low(f32),
    /// No room at all.
    Hidden,
}

/// The shape `kind` wants for `state`'s pattern.
pub fn shape(kind: TopologyKind, state: &UiState) -> Shape {
    match kind {
        TopologyKind::Strings => Shape::Low(strings::card_height(state.pattern.pulses.len())),
        TopologyKind::Arcs => Shape::Aspect(2.0),
        TopologyKind::Hidden => Shape::Hidden,
        TopologyKind::Circle
        | TopologyKind::Orbits
        | TopologyKind::Sweep
        | TopologyKind::Polygons => Shape::Square,
    }
}

/// One pulse as every design sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PulseView {
    /// The pulse this view shows.
    pub id: PulseId,
    /// The theme's color for the pulse.
    pub color: Color32,
    /// Beats in the pulse's cycle (at least 1).
    pub beats: u32,
    /// Shown enabled (the session's mask applied). A disabled pulse keeps
    /// moving, dimmed, with no effects.
    pub enabled: bool,
    /// Position in its cycle ([`UiState::pulse_phase`]): the integer part is
    /// the beat, the fraction the way to the next. `None` while stopped.
    pub phase: Option<f64>,
    /// The last hit, only while playing and enabled, and only on a beat the
    /// pulse still has.
    pub hit: Option<Hit>,
    /// The beat lit before `hit`, still fading out ([`UiState::prev_beat`]);
    /// `None` without a hit, for the first hit after Play, or when that beat
    /// no longer exists.
    pub prev: Option<u32>,
    /// Seed for the hit's random-looking effects ([`UiState::seed`]); 0 without a hit.
    pub seed: u64,
}

/// The single input every design paints from.
pub struct TopologyFrame<'a> {
    /// The pattern's pulses, in order.
    pub pulses: Vec<PulseView>,
    /// Where the measure is, in `[0, 1)`; `None` while stopped.
    pub measure_phase: Option<f64>,
    /// Length of the measure, seconds (pulse 0's beats at the shown tempo):
    /// what turns phase steps into time.
    pub measure_secs: f64,
    /// Seconds since the coincidence, while one shows.
    pub coincidence_age: Option<f32>,
    /// The transport is running.
    pub playing: bool,
    /// The colors to paint with.
    pub theme: &'a Theme,
}

/// The frame `state` shows now.
pub fn frame(state: &UiState) -> TopologyFrame<'_> {
    let t: &Theme = &state.theme;
    let pulses = state
        .pattern
        .pulses
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let beats = p.beats.max(1);
            let enabled = state.pulse_shown_enabled(i);
            let hit = state
                .hit(p.id)
                .filter(|h| state.playing && enabled && h.beat < beats);
            let prev = hit
                .and_then(|_| state.prev_beat(p.id))
                .filter(|&b| b < beats);
            PulseView {
                id: p.id,
                color: t.pulse_color(p.color),
                beats,
                enabled,
                phase: state.pulse_phase(i),
                hit,
                prev,
                seed: hit.map_or(0, |h| UiState::seed(p.id, h.frame)),
            }
        })
        .collect();
    TopologyFrame {
        pulses,
        measure_phase: state.measure_phase(),
        measure_secs: f64::from(state.pattern.reference_beats().max(1)) * 60.0
            / f64::from(state.shown_bpm().get()),
        coincidence_age: state.coincidence_age(),
        playing: state.playing,
        theme: t,
    }
}

/// Paints `kind` over `rect`, a node labelled "Topologia" for AccessKit,
/// with the indicator strip at the bottom. Hidden paints nothing.
pub fn paint(kind: TopologyKind, ui: &mut Ui, state: &UiState, rect: Rect) {
    if kind == TopologyKind::Hidden {
        return;
    }
    let response = ui.allocate_rect(rect, Sense::hover());
    draw(kind, ui, state, rect, &response);
}

/// Paints the circle in a `size` x `size` square: the rings above, the
/// indicators in a strip at the bottom (outside the ring area).
pub fn topology(ui: &mut Ui, state: &UiState, size: f32) {
    topology_in(ui, state, size, size);
}

/// Like [`topology`] in a `width` x `size` area: the rings are as big as
/// `size` allows and stay centered; a wider area only gives the indicators room.
pub fn topology_in(ui: &mut Ui, state: &UiState, width: f32, size: f32) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, size), Sense::hover());
    draw(TopologyKind::Circle, ui, state, rect, &response);
}

/// Labels `response`, then paints `kind` above the indicator strip.
fn draw(kind: TopologyKind, ui: &Ui, state: &UiState, rect: Rect, response: &Response) {
    response.widget_info(|| WidgetInfo::labeled(Role::Image, true, &state.messages.topology));
    let painter = ui.painter_at(rect);
    let strip = strip_height(rect.height());
    let area = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - strip));
    let frame = frame(state);
    match kind {
        TopologyKind::Hidden => {}
        TopologyKind::Strings => strings::paint(&painter, &frame, area),
        TopologyKind::Arcs => arcs::paint(&painter, &frame, area),
        // The other designs come in later tasks; until then they show the circle.
        TopologyKind::Circle
        | TopologyKind::Orbits
        | TopologyKind::Sweep
        | TopologyKind::Polygons => circle::paint(&painter, &frame, area),
    }
    indicators(&painter, rect, strip, state);
}

/// Height of the title strip a short topology card keeps on top.
pub(crate) const CARD_TITLE_STRIP: f32 = 26.0;
/// Padding the card keeps around its drawing area, per side.
pub(crate) const CARD_INNER_PAD: f32 = 6.0;

/// Height reserved under the drawing for the indicators.
pub(crate) fn strip_height(size: f32) -> f32 {
    (size * 0.1).clamp(18.0, 30.0)
}

/// Coincidence light and engine status, centred in the bottom `strip`
/// points of `rect`: under the drawing, or in the pulses header when the
/// topology is hidden. Returns the painted labels' rects, coincidence first;
/// the engine's is `None` when `rect` is too narrow for both.
pub(crate) fn indicators(
    painter: &egui::Painter,
    rect: Rect,
    strip: f32,
    state: &UiState,
) -> (Rect, Option<Rect>) {
    const LIGHT: f32 = 3.0;
    const SPACING: f32 = 1.2;
    const GAP: f32 = 6.0;
    const BETWEEN: f32 = 20.0;
    let m = &state.messages;
    let t = &state.theme;
    let font = theme::font_mono(8.0);
    let y = rect.bottom() - strip / 2.0;
    let coincidence_w = widgets::text_width(painter, &m.coincidence, font.clone(), SPACING);
    let engine_w = widgets::text_width(painter, &m.engine, font.clone(), SPACING);
    let light_w = 2.0 * LIGHT * 1.25;
    let first = light_w + GAP + coincidence_w;
    let second = engine_w + GAP + light_w;
    // The engine light is dropped when the area is too narrow for both.
    let both = first + BETWEEN + second <= rect.width();
    let total = if both {
        first + BETWEEN + second
    } else {
        first
    };
    let mut x = rect.center().x - total / 2.0;

    let lit = state.coincidence();
    let light = pos2(x + light_w / 2.0, y);
    if lit {
        theme::paint_glow(painter, light, 9.0, t.strong, 1.0);
        painter.circle_filled(light, LIGHT * 1.25, t.strong);
    } else {
        painter.circle_filled(light, LIGHT, t.border);
    }
    x += light_w + GAP;
    let coincidence = widgets::text(
        painter,
        pos2(x, y),
        Align2::LEFT_CENTER,
        &m.coincidence,
        font.clone(),
        t.text_muted,
        SPACING,
    );
    if !both {
        return (coincidence, None);
    }
    x += coincidence_w + BETWEEN;
    let engine = widgets::text(
        painter,
        pos2(x, y),
        Align2::LEFT_CENTER,
        &m.engine,
        font,
        t.text_muted,
        SPACING,
    );
    let light = pos2(x + engine_w + GAP + light_w / 2.0, y);
    if state.playing {
        theme::paint_glow(painter, light, 8.0, t.accent, 0.8);
        painter.circle_filled(light, LIGHT, t.accent);
    } else {
        painter.circle_filled(light, LIGHT, t.dot_inactive);
    }
    (coincidence, Some(engine))
}
