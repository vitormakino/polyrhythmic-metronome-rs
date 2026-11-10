//! Pulse cards (web `PolyrhythmControl`) and the scrolling grid that lays
//! them out. Every card's height is its own, so opening one card's settings
//! never resizes its neighbours.

use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Painter, Pos2, Rect, Role, ScrollArea, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, pos2, vec2,
};
use pm_core::{Intensity, MAX_PULSES, Pulse, PulseId, SoundType};

use super::widgets;
use crate::i18n::{Messages, fill};
use crate::state::{UiAction, UiState};
use crate::theme::{self, Theme};

const CARD_GAP: f32 = 12.0;
/// Narrowest pulse card that still fits its header icons and title.
const CARD_MIN_WIDTH: f32 = 132.0;
const MAX_COLUMNS: usize = 3;
const PULSE_RADIUS: u8 = 16;
/// Room around the cards inside the scroll area for their glow and the scroll bar.
const LIST_INSET: f32 = 6.0;
const SCROLL_BAR_ROOM: f32 = 8.0;

/// An edit requested by a pulse card, applied after the list is drawn.
enum Edit {
    Beats(PulseId, u32),
    Enabled(PulseId, bool),
    Remove(PulseId),
    Settings(PulseId),
    Cycle(PulseId, usize),
    Sound(PulseId, SoundType),
}

/// What the drag handles reported this frame.
#[derive(Default)]
struct DragOutcome {
    started: Option<usize>,
    /// Card under the pointer while a drag is in progress.
    hovered: Option<usize>,
    stopped: bool,
}

pub(crate) fn pulse_list(
    ui: &mut Ui,
    rect: Rect,
    grid: &Grid,
    state: &mut UiState,
    actions: &mut Vec<UiAction>,
) {
    if rect.height() < 1.0 {
        return;
    }
    let mut edits = Vec::new();
    let mut drag = DragOutcome::default();
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ScrollArea::vertical()
            .id_salt("pulse-list")
            .auto_shrink(false)
            .show(ui, |ui| pulse_grid(ui, grid, state, &mut edits, &mut drag));
    });

    let mut changed = false;
    for edit in edits {
        let p = &mut state.pattern;
        match edit {
            Edit::Beats(id, beats) => p.set_beats(id, beats),
            Edit::Enabled(id, on) => p.set_enabled(id, on),
            Edit::Remove(id) => {
                if !p.remove(id) {
                    continue;
                }
            }
            Edit::Settings(id) => {
                state.toggle_settings(id);
                continue;
            }
            Edit::Cycle(id, beat) => p.cycle_intensity(id, beat),
            Edit::Sound(id, sound) => p.set_sound(id, sound),
        }
        changed = true;
    }

    if let Some(from) = drag.started {
        state.dragging = Some((from, from));
    }
    if let (Some((from, _)), Some(target)) = (state.dragging, drag.hovered) {
        state.dragging = Some((from, target));
    }
    if drag.stopped
        && let Some((from, to)) = state.dragging.take()
        && from != to
    {
        state.pattern.move_pulse(from, to);
        changed = true;
    }
    if changed {
        actions.push(UiAction::PatternChanged);
    }
}

/// Card sizes and positions (relative to the list's top-left) for a list `width` wide.
pub(crate) struct Grid {
    columns: usize,
    card_w: f32,
    metrics: Vec<CardMetrics>,
    /// Top of each row.
    tops: Vec<f32>,
    /// Content height, insets included.
    pub(crate) height: f32,
}

impl Grid {
    pub(crate) fn new(painter: &Painter, state: &UiState, width: f32) -> Self {
        let width = width - 2.0 * LIST_INSET - SCROLL_BAR_ROOM;
        let fit = ((width + CARD_GAP) / (CARD_MIN_WIDTH + CARD_GAP)).floor() as usize;
        let columns = fit.clamp(1, MAX_COLUMNS);
        let card_w = ((width - CARD_GAP * (columns - 1) as f32) / columns as f32).floor();
        let metrics: Vec<CardMetrics> = state
            .pattern
            .pulses
            .iter()
            .map(|p| {
                CardMetrics::new(
                    painter,
                    p,
                    card_w,
                    state.settings_open(p.id),
                    &state.messages,
                )
            })
            .collect();
        // Rows are as tall as their tallest card; each card keeps its own height.
        let mut tops = Vec::new();
        let mut y = 0.0;
        for chunk in metrics.chunks(columns) {
            tops.push(y);
            y += chunk.iter().map(|m| m.height).fold(0.0, f32::max) + CARD_GAP;
        }
        Self {
            columns,
            card_w,
            metrics,
            tops,
            height: (y - CARD_GAP).max(0.0) + 2.0 * LIST_INSET,
        }
    }

    fn rect(&self, origin: Pos2, i: usize) -> Rect {
        let (row, col) = (i / self.columns, i % self.columns);
        Rect::from_min_size(
            origin + vec2(col as f32 * (self.card_w + CARD_GAP), self.tops[row]),
            vec2(self.card_w, self.metrics[i].height),
        )
    }
}

fn pulse_grid(
    ui: &mut Ui,
    grid: &Grid,
    state: &UiState,
    edits: &mut Vec<Edit>,
    drag: &mut DragOutcome,
) {
    let (_, area) = ui.allocate_space(vec2(ui.available_width(), grid.height));
    let origin = area.min + Vec2::splat(LIST_INSET);
    let pulses = &state.pattern.pulses;
    let rects: Vec<Rect> = (0..pulses.len()).map(|i| grid.rect(origin, i)).collect();

    if state.dragging.is_some()
        && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
    {
        drag.hovered = rects.iter().position(|r| r.contains(pos));
    }
    let drop_target = state
        .dragging
        .and_then(|(from, to)| Some(drag.hovered.unwrap_or(to)).filter(|&t| t != from));
    for (i, (pulse, metrics)) in pulses.iter().zip(&grid.metrics).enumerate() {
        Card {
            m: &state.messages,
            theme: &state.theme,
            index: i,
            id: ui.make_persistent_id(("pulse", pulse.id)),
            pulse,
            rect: rects[i],
            metrics,
            can_remove: pulses.len() > 1,
            open: state.settings_open(pulse.id),
            enabled: state.pulse_shown_enabled(i),
            lit: state.playing
                && state.pulse_shown_enabled(i)
                && state.active_beat(pulse.id).is_some(),
            flash: state.flash(pulse.id),
            drop_target: drop_target == Some(i),
        }
        .show(ui, edits, drag);
    }
}

const ICON_BUTTON: f32 = 24.0;
const ICON_ROW: f32 = 28.0;
const TITLE_ROW: f32 = 16.0;
const STEP_H: f32 = 22.0;
const NUMBER_H: f32 = 38.0;
const BAR_W: f32 = 16.0;
const BAR_H: f32 = 48.0;
const BAR_GAP: f32 = 4.0;
const BAR_LABEL: f32 = 12.0;
const BAR_ROW_GAP: f32 = 6.0;
const SETTINGS_PAD: f32 = 10.0;
const SOUND_CAPTION: f32 = 12.0;
const SOUND_H: f32 = 20.0;
const SOUND_GAP: f32 = 4.0;
const BOTTOM_BAR: f32 = 4.0;

/// Heading of pulse number `n` (1-based), in capitals.
fn pulse_title(m: &Messages, n: usize) -> String {
    fill(&m.pulse, &[("n", &n)]).to_uppercase()
}

fn title_font() -> FontId {
    theme::font_mono_medium(9.0)
}

/// Vertical metrics of a pulse card; they depend only on the card's own content.
struct CardMetrics {
    /// The title gets its own row when it does not fit between the icons.
    title_row: bool,
    /// Height of everything above the settings panel.
    base: f32,
    settings: f32,
    /// Intensity bars per row in the settings panel.
    bars_per_row: usize,
    height: f32,
}

impl CardMetrics {
    fn new(painter: &Painter, pulse: &Pulse, width: f32, open: bool, m: &Messages) -> Self {
        // grip + settings on the left, power + trash on the right.
        let icons = 2.0 * (4.0 + 2.0 * ICON_BUTTON);
        let widest = pulse_title(m, MAX_PULSES);
        let title_w = widgets::text_width(painter, &widest, title_font(), 1.0);
        let title_row = width - icons < title_w + 8.0;
        let base =
            ICON_ROW + if title_row { TITLE_ROW } else { 0.0 } + 2.0 * STEP_H + NUMBER_H + 8.0;
        let fit = ((width - 2.0 * SETTINGS_PAD + BAR_GAP) / (BAR_W + BAR_GAP)).floor();
        let bars_per_row = (fit as usize).max(1);
        let settings = if open {
            let rows = (pulse.intensities.len().max(1)).div_ceil(bars_per_row) as f32;
            SETTINGS_PAD
                + rows * (BAR_H + BAR_LABEL)
                + (rows - 1.0) * BAR_ROW_GAP
                + 12.0
                + SOUND_CAPTION
                + 2.0 * SOUND_H
                + SOUND_GAP
                + SETTINGS_PAD
        } else {
            0.0
        };
        Self {
            title_row,
            base,
            settings,
            bars_per_row,
            height: base + settings + BOTTOM_BAR,
        }
    }
}

struct Card<'a> {
    m: &'a Messages,
    theme: &'a Theme,
    index: usize,
    id: Id,
    pulse: &'a Pulse,
    rect: Rect,
    metrics: &'a CardMetrics,
    can_remove: bool,
    open: bool,
    /// The pulse shows as on: the pattern's `enabled` with the session's mask applied.
    enabled: bool,
    /// The pulse has a lit beat while playing.
    lit: bool,
    flash: f32,
    drop_target: bool,
}

impl Card<'_> {
    fn label(&self, what: &str) -> String {
        format!(
            "{}: {what}",
            fill(&self.m.pulse, &[("n", &(self.index + 1))])
        )
    }

    fn show(&self, ui: &Ui, edits: &mut Vec<Edit>, drag: &mut DragOutcome) {
        let (pulse, rect, id) = (self.pulse, self.rect, self.id);
        let t = self.theme;
        let mut painter = ui.painter().clone();
        if !self.enabled {
            painter.multiply_opacity(0.5);
        }
        let color = if self.enabled {
            t.pulse_color(pulse.color)
        } else {
            t.dot_inactive
        };

        // The card's own node, so automation can find it and read its rect.
        let card = ui.interact(rect, id.with("card"), Sense::hover());
        let name = fill(&self.m.pulse, &[("n", &(self.index + 1))]);
        card.widget_info(|| WidgetInfo::labeled(Role::Group, true, &name));

        let glow = if self.lit {
            0.5 + 0.5 * self.flash
        } else {
            0.0
        };
        let border = if self.lit { color } else { t.border };
        theme::paint_card_with_radius(&painter, t, rect, border, glow, PULSE_RADIUS);
        let radius = CornerRadius::same(PULSE_RADIUS);
        if self.lit && self.flash > 0.01 {
            let ring = Stroke::new(2.0, theme::with_alpha(color, 0.8 * self.flash));
            painter.rect_stroke(rect, radius, ring, StrokeKind::Outside);
        }
        if self.drop_target {
            let ring = Stroke::new(1.5, t.accent);
            painter.rect_stroke(rect, radius, ring, StrokeKind::Outside);
        }

        self.header(ui, &painter, edits, drag);
        self.counter(ui, &painter, color, edits);

        let inner = rect.shrink(1.0);
        let inner_radius = f32::from(PULSE_RADIUS) - 1.0;
        if self.open {
            let panel = Rect::from_min_size(
                pos2(inner.left(), rect.top() + self.metrics.base),
                vec2(inner.width(), self.metrics.settings),
            );
            self.settings(ui, &painter, panel, color, edits);
        }

        // Bottom bar: one segment per beat, alpha by intensity.
        let bar = Rect::from_min_max(pos2(inner.left(), inner.bottom() - BOTTOM_BAR), inner.max);
        theme::paint_band(&painter, inner, inner_radius, bar, t.border);
        let seg = bar.width() / pulse.intensities.len().max(1) as f32;
        for (k, intensity) in pulse.intensities.iter().enumerate() {
            let alpha = if self.enabled {
                intensity.gain() * 0.7
            } else {
                0.3
            };
            let x0 = bar.left() + seg * k as f32;
            let r = Rect::from_min_max(pos2(x0, bar.top()), pos2(x0 + seg - 0.5, bar.bottom()));
            theme::paint_band(
                &painter,
                inner,
                inner_radius,
                r,
                theme::with_alpha(color, alpha),
            );
        }
    }

    /// Drag handle and settings on the left, mute and remove on the right, title between.
    fn header(&self, ui: &Ui, painter: &Painter, edits: &mut Vec<Edit>, drag: &mut DragOutcome) {
        let (pulse, rect, id) = (self.pulse, self.rect, self.id);
        let t = self.theme;
        let icon_y = rect.top() + 4.0 + ICON_BUTTON / 2.0;
        let slot = |x: f32| Rect::from_center_size(pos2(x, icon_y), Vec2::splat(ICON_BUTTON));
        let left = rect.left() + 4.0 + ICON_BUTTON / 2.0;
        let right = rect.right() - 4.0 - ICON_BUTTON / 2.0;

        let grip_rect = slot(left);
        let grip = widgets::handle(
            ui,
            grip_rect,
            id.with("move"),
            &self.label(&self.m.pulse_move),
        );
        let grip_color = if grip.hovered() || grip.dragged() {
            t.text_muted
        } else {
            t.dot_inactive
        };
        widgets::grip(painter, grip_rect.center(), 14.0, grip_color);
        if grip.drag_started() {
            drag.started = Some(self.index);
        }
        if grip.drag_stopped() {
            drag.stopped = true;
        }

        let gear_rect = slot(left + ICON_BUTTON);
        let gear = widgets::toggle(
            ui,
            gear_rect,
            id.with("settings"),
            &self.label(&self.m.pulse_settings),
            self.open,
        );
        let gear_color = if self.open {
            t.accent
        } else {
            widgets::idle_color(t, &gear, t.text_muted)
        };
        widgets::settings(painter, gear_rect.center(), 16.0, gear_color);
        if gear.clicked() {
            edits.push(Edit::Settings(pulse.id));
        }

        let trash_rect = slot(right);
        let trash = widgets::button(
            ui,
            trash_rect,
            id.with("remove"),
            &self.label(&self.m.pulse_remove),
            self.can_remove,
        );
        let trash_color = if !self.can_remove {
            t.border
        } else if trash.hovered() {
            t.pulses[4]
        } else {
            t.text_muted
        };
        widgets::trash(painter, trash_rect.center(), 15.0, trash_color);
        if trash.clicked() {
            edits.push(Edit::Remove(pulse.id));
        }

        let power_rect = slot(right - ICON_BUTTON);
        let power = widgets::toggle(
            ui,
            power_rect,
            id.with("mute"),
            &self.label(&self.m.pulse_mute),
            !self.enabled,
        );
        let green = t.pulses[2];
        let (power_color, power_fill) = if self.enabled {
            let c = if power.hovered() { green } else { t.text_muted };
            (c, Some(theme::with_alpha(green, 0.2)))
        } else {
            (widgets::idle_color(t, &power, t.dot_inactive), None)
        };
        widgets::power(painter, power_rect.center(), 14.0, power_color, power_fill);
        if power.clicked() {
            edits.push(Edit::Enabled(pulse.id, !self.enabled));
        }

        let title_y = if self.metrics.title_row {
            rect.top() + ICON_ROW + TITLE_ROW / 2.0
        } else {
            icon_y
        };
        widgets::text(
            painter,
            pos2(rect.center().x, title_y),
            Align2::CENTER_CENTER,
            &pulse_title(self.m, self.index + 1),
            title_font(),
            t.text_muted,
            1.0,
        );
    }

    /// `+`, the beat count in the pulse color, `-`.
    fn counter(&self, ui: &Ui, painter: &Painter, color: Color32, edits: &mut Vec<Edit>) {
        let (pulse, rect, id) = (self.pulse, self.rect, self.id);
        let t = self.theme;
        let cx = rect.center().x;
        let mut y = rect.top()
            + ICON_ROW
            + if self.metrics.title_row {
                TITLE_ROW
            } else {
                0.0
            };
        let up = Rect::from_min_size(pos2(cx - 16.0, y), vec2(32.0, STEP_H));
        let up_resp = widgets::button(
            ui,
            up,
            id.with("more"),
            &self.label(&self.m.pulse_more_beats),
            true,
        );
        widgets::plus(
            painter,
            up.center(),
            14.0,
            widgets::idle_color(t, &up_resp, t.text_dim),
        );
        if up_resp.clicked() {
            edits.push(Edit::Beats(pulse.id, pulse.beats + 1));
        }
        y += STEP_H;
        widgets::text(
            painter,
            pos2(cx, y + NUMBER_H / 2.0),
            Align2::CENTER_CENTER,
            &pulse.beats.to_string(),
            theme::font_light(30.0),
            color,
            0.0,
        );
        y += NUMBER_H;
        let down = Rect::from_min_size(pos2(cx - 16.0, y), vec2(32.0, STEP_H));
        let can_less = pulse.beats > 1;
        let down_resp = widgets::button(
            ui,
            down,
            id.with("less"),
            &self.label(&self.m.pulse_fewer_beats),
            can_less,
        );
        let down_color = if can_less {
            widgets::idle_color(t, &down_resp, t.text_dim)
        } else {
            t.dot_inactive
        };
        widgets::minus(painter, down.center(), 14.0, down_color);
        if down_resp.clicked() {
            edits.push(Edit::Beats(pulse.id, pulse.beats - 1));
        }
    }

    /// Per-beat intensity bars (wrapped, centered rows) and the 2 x 2 sound grid.
    fn settings(
        &self,
        ui: &Ui,
        painter: &Painter,
        panel: Rect,
        color: Color32,
        edits: &mut Vec<Edit>,
    ) {
        let (pulse, id) = (self.pulse, self.id);
        let t = self.theme;
        painter.rect_filled(panel, 0.0, t.overlay);
        painter.hline(
            panel.x_range(),
            panel.top(),
            Stroke::new(1.0, theme::with_alpha(t.border, 0.5)),
        );

        let per_row = self.metrics.bars_per_row;
        let mut y = panel.top() + SETTINGS_PAD;
        for (row, chunk) in pulse.intensities.chunks(per_row).enumerate() {
            let count = chunk.len() as f32;
            let row_w = count * BAR_W + (count - 1.0) * BAR_GAP;
            let x0 = panel.center().x - row_w / 2.0;
            for (col, intensity) in chunk.iter().enumerate() {
                let k = row * per_row + col;
                let r = Rect::from_min_size(
                    pos2(x0 + col as f32 * (BAR_W + BAR_GAP), y),
                    vec2(BAR_W, BAR_H),
                );
                let label = self.label(&fill(&self.m.pulse_beat, &[("n", &(k + 1))]));
                let resp = widgets::button(ui, r, id.with(("beat", k)), &label, true);
                let r = if resp.is_pointer_button_down_on() {
                    r.shrink(0.6)
                } else {
                    r
                };
                let bg = if resp.hovered() {
                    t.hover_fill
                } else {
                    t.border
                };
                let edge = Stroke::new(1.0, theme::with_alpha(t.dot_inactive, 0.3));
                painter.rect(r, 2.0, bg, edge, StrokeKind::Inside);
                if *intensity != Intensity::Mute {
                    let top = r.bottom() - r.height() * intensity.gain();
                    let fill = Rect::from_min_max(pos2(r.left(), top), r.max);
                    painter.rect_filled(fill, 2.0, color);
                }
                widgets::text(
                    painter,
                    pos2(r.center().x, r.bottom() + BAR_LABEL / 2.0 + 1.0),
                    Align2::CENTER_CENTER,
                    &(k + 1).to_string(),
                    theme::font_mono(8.0),
                    t.text_faint,
                    0.0,
                );
                if resp.clicked() {
                    edits.push(Edit::Cycle(pulse.id, k));
                }
            }
            y += BAR_H + BAR_LABEL + BAR_ROW_GAP;
        }
        y += 12.0 - BAR_ROW_GAP;

        let left = panel.left() + SETTINGS_PAD;
        widgets::text(
            painter,
            pos2(left, y + SOUND_CAPTION / 2.0),
            Align2::LEFT_CENTER,
            &self.m.sound_heading,
            theme::font_mono(8.0),
            t.text_muted,
            1.0,
        );
        y += SOUND_CAPTION;
        let cell_w = (panel.width() - 2.0 * SETTINGS_PAD - SOUND_GAP) / 2.0;
        let sounds = [
            (SoundType::Sine, &self.m.sound_sine),
            (SoundType::Wood, &self.m.sound_wood),
            (SoundType::Bell, &self.m.sound_bell),
            (SoundType::Electronic, &self.m.sound_electronic),
        ];
        for (i, (sound, name)) in sounds.into_iter().enumerate() {
            let (row, col) = (i / 2, i % 2);
            let r = Rect::from_min_size(
                pos2(
                    left + col as f32 * (cell_w + SOUND_GAP),
                    y + row as f32 * (SOUND_H + SOUND_GAP),
                ),
                vec2(cell_w, SOUND_H),
            );
            let selected = pulse.sound == sound;
            let label = self.label(&fill(&self.m.pulse_sound, &[("name", name)]));
            let resp = widgets::toggle(ui, r, id.with(("sound", i)), &label, selected);
            let (bg, edge, text) = if selected {
                (t.dot_inactive, t.text_faint, t.strong)
            } else if resp.hovered() {
                (t.hover_fill, t.border, t.text_dim)
            } else {
                (t.border, t.border, t.text_muted)
            };
            painter.rect(r, 4.0, bg, Stroke::new(1.0, edge), StrokeKind::Inside);
            widgets::elided(
                painter,
                r.center(),
                Align2::CENTER_CENTER,
                &name.to_uppercase(),
                theme::font_semibold(8.0),
                text,
                r.width() - 6.0,
            );
            if resp.clicked() && !selected {
                edits.push(Edit::Sound(pulse.id, sound));
            }
        }
    }
}
