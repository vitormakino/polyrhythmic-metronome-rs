//! The session strip, right above Play in the full window: the session
//! selector, and while a session runs its stage, counter, description and a
//! stage bar (one segment per stage). The mini window shows the stage bar alone.
//!
//! With no session files the strip is not drawn and the layout is unchanged.

use egui::{
    Align2, ComboBox, CornerRadius, Id, Painter, Rect, RichText, Role, Sense, Ui, UiBuilder,
    WidgetInfo, pos2, vec2,
};
use pm_core::session::{Jump, Moment, OnFinish, SessionProgress, Span, Stage};

use super::widgets;
use crate::i18n::{Messages, fill};
use crate::state::{UiAction, UiState};
use crate::theme::{self, Theme};

/// Height of the strip with a session selected: selector line, description
/// line and stage bar.
pub const STRIP_H: f32 = 64.0;
/// Height of the strip with "No session" selected: the selector alone.
pub const STRIP_LINE_H: f32 = 38.0;

const PAD_X: f32 = 12.0;
const PAD_Y: f32 = 8.0;
const LINE_H: f32 = 22.0;
const BAR_H: f32 = 4.0;
const SEGMENT_GAP: f32 = 3.0;
const RADIUS: u8 = 16;
const SEPARATOR: &str = " · ";

/// Height the strip takes in the full window: 0 when there are no sessions.
pub fn strip_height(state: &UiState) -> f32 {
    if state.sessions.is_empty() {
        0.0
    } else if state.session.is_none() {
        STRIP_LINE_H
    } else {
        STRIP_H
    }
}

/// Draws the strip in `rect`. Picking a session sets `state.session`, drops
/// the summary and pushes `SelectSession`.
pub fn session_strip(ui: &mut Ui, rect: Rect, state: &mut UiState, actions: &mut Vec<UiAction>) {
    if state.sessions.is_empty() || rect.height() < 1.0 {
        return;
    }
    let t = state.theme.clone();
    let m = state.messages.clone();
    let painter = ui.painter().clone();
    theme::paint_card_with_radius(&painter, &t, rect, t.border, 0.0, RADIUS);
    let inner = rect.shrink2(vec2(PAD_X, PAD_Y));
    let line = Rect::from_min_size(inner.min, vec2(inner.width(), LINE_H));

    let active = state.running_session().copied();
    // Its stages are still going; a finished one shows the summary.
    let running = active.filter(|p| !p.finished);
    let combo_w = (inner.width() * 0.4).clamp(96.0, 200.0);
    let combo = Rect::from_min_size(line.min, vec2(combo_w, LINE_H));
    if let Some(picked) = selector(ui, combo, state, active.is_none()) {
        state.select_session(picked, actions);
    }
    if state.session.is_none() {
        return;
    }

    // After the selector: stage and counter while running, or the summary.
    let status_left = combo.right() + 12.0;
    let mut status_right = line.right();
    // The running plan, not the file's: it may have changed or gone.
    let plan = state.shown_session().map(|s| s.plan.clone());
    let stage = |p: &SessionProgress| {
        plan.as_ref()
            .and_then(|plan| plan.stages.get((p.stage as usize).checked_sub(1)?))
    };
    let status = if let Some(p) = &running {
        if p.moment.silent {
            status_right = muted_badge(&painter, &t, &m, line).left() - 8.0;
        }
        let mut parts = vec![fill(
            &m.session_stage,
            &[("n", &p.stage), ("total", &p.stages)],
        )];
        if let Some(stage) = stage(p) {
            parts.push(counter(&m, stage, p));
        }
        Some((parts.join(SEPARATOR), t.text_dim))
    } else {
        let finished = active.filter(|p| p.finished).or(state.session_summary);
        finished.map(|p| {
            let continuing = active.is_some()
                && plan
                    .as_ref()
                    .is_some_and(|plan| plan.on_finish == OnFinish::Continue);
            let text = if continuing {
                m.session_done_continue.clone()
            } else {
                let secs = p.total_frames / u64::from(p.sample_rate.max(1));
                fill(
                    &m.session_done,
                    &[("time", &clock(secs)), ("bpm", &p.max_bpm.get())],
                )
            };
            (text, t.text_soft)
        })
    };
    if let Some((text, color)) = status {
        let painted = widgets::elided(
            &painter,
            pos2(status_left, line.center().y),
            Align2::LEFT_CENTER,
            &text,
            theme::font_mono(10.0),
            color,
            status_right - status_left,
        );
        label_node(ui, painted, ui.make_persistent_id("session-status"), &text);
    }

    if let Some(p) = &running
        && let Some(stage) = stage(p)
    {
        let text = describe(&m, stage, &p.moment, state.pattern.pulses.len()).join(SEPARATOR);
        let y = (line.bottom() + inner.bottom() - BAR_H) / 2.0;
        let painted = widgets::elided(
            &painter,
            pos2(inner.left(), y),
            Align2::LEFT_CENTER,
            &text,
            theme::font_mono(10.0),
            t.text_muted,
            inner.width(),
        );
        label_node(ui, painted, ui.make_persistent_id("session-stage"), &text);
    }

    let bar = Rect::from_min_max(pos2(inner.left(), inner.bottom() - BAR_H), inner.max);
    stage_bar(&painter, bar, state);
}

/// The selector at `rect`; returns the pick, if the user made one this frame.
fn selector(ui: &mut Ui, rect: Rect, state: &UiState, enabled: bool) -> Option<Option<String>> {
    let t = &state.theme;
    let m = &state.messages;
    let current = state
        .shown_session()
        .map_or(m.session_none.as_str(), |s| s.name.as_str());
    let mut picked = None;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            let spacing = ui.spacing_mut();
            spacing.interact_size.y = rect.height();
            spacing.button_padding = vec2(10.0, 3.0);
            ui.name_inputs(&m.session_select, |ui| {
                ComboBox::from_id_salt("session")
                    .width(rect.width())
                    .truncate()
                    .selected_text(
                        RichText::new(current)
                            .font(theme::font_semibold(12.0))
                            .color(t.text),
                    )
                    .show_ui(ui, |ui| {
                        let none = state.session.is_none();
                        if ui.selectable_label(none, &m.session_none).clicked() {
                            picked = Some(None);
                        }
                        for entry in state.sessions.iter() {
                            let on = state.session.as_deref() == Some(entry.name.as_str());
                            if ui.selectable_label(on, &entry.name).clicked() {
                                picked = Some(Some(entry.name.clone()));
                            }
                        }
                    });
            });
        });
    });
    picked
}

/// The "MUTED" mark at the right end of `line`; returns its rect.
fn muted_badge(painter: &Painter, t: &Theme, m: &Messages, line: Rect) -> Rect {
    let font = theme::font_mono_medium(9.0);
    let w = widgets::text_width(painter, &m.session_muted_now, font.clone(), 1.0) + 12.0;
    let badge =
        Rect::from_center_size(pos2(line.right() - w / 2.0, line.center().y), vec2(w, 16.0));
    painter.rect_filled(badge, 4.0, theme::with_alpha(t.accent, 0.15));
    widgets::text(
        painter,
        badge.center(),
        Align2::CENTER_CENTER,
        &m.session_muted_now,
        font,
        t.accent,
        1.0,
    );
    badge
}

/// A text the automation (and screen readers) can find, over `rect`.
fn label_node(ui: &Ui, rect: Rect, id: Id, text: &str) {
    ui.interact(rect, id, Sense::hover())
        .widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
}

/// "bar 6/16" or "0:42/1:00" for the stage `p` is in.
fn counter(m: &Messages, stage: &Stage, p: &SessionProgress) -> String {
    match stage.length {
        Span::Bars(n) => fill(
            &m.session_bar,
            &[("n", &(p.bars_in_stage + 1).min(n.max(1))), ("total", &n)],
        ),
        Span::Seconds(s) => {
            let elapsed = (p.frames_in_stage / u64::from(p.sample_rate.max(1))).min(u64::from(s));
            fill(
                &m.session_time,
                &[
                    ("elapsed", &clock(elapsed)),
                    ("total", &clock(u64::from(s))),
                ],
            )
        }
    }
}

/// `m:ss`.
fn clock(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn span(m: &Messages, span: Span) -> String {
    match span {
        Span::Bars(n) => fill(&m.session_bars, &[("n", &n)]),
        Span::Seconds(n) => fill(&m.session_seconds, &[("n", &n)]),
    }
}

/// The parts of the stage's description, in order: tempo, mutes, pulses. A
/// pulse position past the `on_screen` pulses is a warning instead.
fn describe(m: &Messages, stage: &Stage, moment: &Moment, on_screen: usize) -> Vec<String> {
    use pm_core::session::PulseState;
    let mut parts = Vec::new();
    if let Some(tempo) = stage.tempo {
        match tempo.jump {
            Some(Jump::By(n)) => parts.push(fill(
                &m.session_tempo_jump,
                &[("change", &format!("{n:+}"))],
            )),
            Some(Jump::To(bpm)) => parts.push(fill(&m.session_tempo_set, &[("bpm", &bpm.get())])),
            None => {}
        }
        if let Some(ramp) = tempo.ramp {
            parts.push(fill(
                &m.session_tempo_ramp,
                &[
                    ("change", &format!("{:+}", ramp.change)),
                    ("every", &span(m, ramp.every)),
                ],
            ));
            if let Some(limit) = ramp.limit {
                parts.push(fill(&m.session_tempo_limit, &[("bpm", &limit.get())]));
            }
        }
    }
    if let Some(cycle) = stage.mute {
        parts.push(fill(
            &m.session_mute_cycle,
            &[
                ("play", &span(m, cycle.play)),
                ("mute", &span(m, cycle.mute)),
            ],
        ));
    }
    if stage.random_mute.is_some() {
        parts.push(fill(
            &m.session_random_mute,
            &[("percent", &moment.random_percent)],
        ));
    }
    for (i, state) in stage.pulses.iter().enumerate() {
        let Some(state) = state else { continue };
        let template = if i >= on_screen {
            &m.session_missing_pulse
        } else {
            match state {
                PulseState::Off => &m.session_pulse_off,
                PulseState::Muted => &m.session_pulse_muted,
                PulseState::On => &m.session_pulse_on,
            }
        };
        parts.push(fill(template, &[("n", &(i + 1))]));
    }
    parts
}

/// One segment per stage in `rect`: finished ones `accent`, the current one
/// filling in proportion to its bars or time, the later ones `dot_inactive`.
/// After the session finished every segment is full; before it starts none is.
pub(crate) fn stage_bar(painter: &Painter, rect: Rect, state: &UiState) {
    let t = &state.theme;
    let plan = state.shown_session().map(|s| &s.plan);
    let active = state.running_session();
    let running = active.filter(|p| !p.finished);
    let stages = active
        .map(|p| p.stages as usize)
        .or(plan.map(|p| p.stages.len()))
        .unwrap_or(0)
        .max(1);
    // How full each segment is.
    let fullness = |i: usize| -> f32 {
        if let Some(p) = running {
            let current = (p.stage as usize).saturating_sub(1);
            match i.cmp(&current) {
                std::cmp::Ordering::Less => 1.0,
                std::cmp::Ordering::Greater => 0.0,
                std::cmp::Ordering::Equal => plan
                    .and_then(|plan| plan.stages.get(current))
                    .map_or(0.0, |stage| stage_fraction(stage, p)),
            }
        } else if active.is_some() || state.session_summary.is_some() {
            1.0
        } else {
            0.0
        }
    };
    let seg_w = (rect.width() - SEGMENT_GAP * (stages - 1) as f32) / stages as f32;
    let radius = CornerRadius::same((rect.height() / 2.0) as u8);
    for i in 0..stages {
        let x = rect.left() + i as f32 * (seg_w + SEGMENT_GAP);
        let seg = Rect::from_min_size(pos2(x, rect.top()), vec2(seg_w, rect.height()));
        painter.rect_filled(seg, radius, t.dot_inactive);
        let full = fullness(i);
        if full > 0.0 {
            let done = Rect::from_min_size(seg.min, vec2(seg_w * full, rect.height()));
            painter.rect_filled(done, radius, t.accent);
        }
    }
}

/// How far into `stage` the progress is, 0..=1.
fn stage_fraction(stage: &Stage, p: &SessionProgress) -> f32 {
    match stage.length {
        Span::Bars(n) => p.bars_in_stage as f32 / n.max(1) as f32,
        Span::Seconds(s) => {
            p.frames_in_stage as f32
                / (u64::from(s.max(1)) * u64::from(p.sample_rate.max(1))) as f32
        }
    }
    .clamp(0.0, 1.0)
}
