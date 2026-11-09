//! What the views draw from and edit: the pattern, the tempo, transport and
//! window mode, plus the visual beat state (which beat is lit, flash springs)
//! kept in sync with the audio through `visual_offset`.
//!
//! Views never talk to the audio engine: they edit [`UiState`] and return
//! [`UiAction`]s for the app to forward.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use pm_audio::BeatEvent;
use pm_core::{Bpm, Pattern, PulseId, TapTempo};
use serde::{Deserialize, Serialize};

use crate::spring::Spring;

/// Most beats queued for [`UiState::advance`]; older ones are dropped
/// (e.g. while the window is hidden and nothing advances).
pub const PENDING_BEATS_CAP: usize = 256;

/// Which window the app shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowMode {
    Full,
    Mini,
}

/// What a view asks the app to do after it has already updated [`UiState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAction {
    /// Start or stop; `UiState::playing` already holds the new value.
    TogglePlay,
    /// `UiState::bpm` changed to this tempo.
    SetBpm(Bpm),
    /// A tap at this time since the state's tap epoch. When the taps give a
    /// tempo it is applied to `UiState::bpm` and a `SetBpm` follows.
    Tap(Duration),
    /// `UiState::pattern` was edited.
    PatternChanged,
    /// Switch between the full and the mini window; `UiState::mode` already flipped.
    ToggleMini,
    SetVisualOffset(Duration),
}

/// The beat a pulse lit last and its flash animation.
#[derive(Clone, Copy, Debug)]
struct Lit {
    beat: u32,
    /// Audio frame of the hit, to tell hits of one instant from stale ones.
    frame: u64,
    /// 1 right after the beat, springs back to 0.
    flash: Spring,
}

pub struct UiState {
    pub pattern: Pattern,
    pub bpm: Bpm,
    pub playing: bool,
    pub mode: WindowMode,
    /// How long after its DAC time a beat is shown (display latency compensation).
    pub visual_offset: Duration,
    pub underruns: u64,
    pub device_name: Option<String>,
    /// Why the output could not be opened, shown on hover over "sem saída
    /// de áudio".
    pub audio_error: Option<String>,
    tap: TapTempo,
    tap_epoch: Instant,
    pending: VecDeque<BeatEvent>,
    lit: HashMap<PulseId, Lit>,
    /// Per pulse, one spring per beat: 0 idle, 1 lit. Drives the topology dots.
    dots: HashMap<PulseId, Vec<Spring>>,
    settings_open: HashSet<PulseId>,
    /// While a pulse card is dragged by its handle: (from, current target) indices.
    pub(crate) dragging: Option<(usize, usize)>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            pattern: Pattern::default(),
            bpm: Bpm::new(120),
            playing: false,
            mode: WindowMode::Full,
            visual_offset: Duration::ZERO,
            underruns: 0,
            device_name: None,
            audio_error: None,
            tap: TapTempo::default(),
            tap_epoch: Instant::now(),
            pending: VecDeque::with_capacity(PENDING_BEATS_CAP),
            lit: HashMap::new(),
            dots: HashMap::new(),
            settings_open: HashSet::new(),
            dragging: None,
        }
    }
}

impl UiState {
    /// Queues a beat from the engine; it lights in [`Self::advance`] once its time comes.
    /// Ignored while stopped, so beats still in flight after Stop never light.
    /// At most [`PENDING_BEATS_CAP`] beats wait; the oldest is dropped first.
    pub fn on_beat(&mut self, ev: &BeatEvent) {
        if !self.playing {
            return;
        }
        if self.pending.len() == PENDING_BEATS_CAP {
            self.pending.pop_front();
        }
        self.pending.push_back(*ev);
    }

    /// Lights every queued beat whose `at + visual_offset` has passed and
    /// steps the flash springs by `dt` seconds.
    pub fn advance(&mut self, now: Instant, dt: f32) {
        let offset = self.visual_offset;
        let mut due = Vec::new();
        self.pending.retain(|ev| {
            if now >= ev.at + offset {
                due.push(*ev);
                false
            } else {
                true
            }
        });
        due.sort_by_key(|ev| ev.frame);
        for ev in due {
            let mut flash = Spring::new(1.0);
            flash.target = 0.0;
            self.lit.insert(
                ev.pulse,
                Lit {
                    beat: ev.beat,
                    frame: ev.frame,
                    flash,
                },
            );
        }
        let pattern = &self.pattern;
        self.lit.retain(|id, _| pattern.pulse(*id).is_some());
        for lit in self.lit.values_mut() {
            lit.flash.step(dt);
        }
        self.dots.retain(|id, _| pattern.pulse(*id).is_some());
        for pulse in &pattern.pulses {
            let lit_beat = self
                .lit
                .get(&pulse.id)
                .filter(|_| pulse.enabled)
                .map(|l| l.beat as usize);
            let springs = self.dots.entry(pulse.id).or_default();
            springs.resize(pulse.beats as usize, Spring::new(0.0));
            for (i, spring) in springs.iter_mut().enumerate() {
                spring.target = if lit_beat == Some(i) { 1.0 } else { 0.0 };
                spring.step(dt);
            }
        }
    }

    /// The beat of `pulse` that is lit now, if any.
    pub fn active_beat(&self, pulse: PulseId) -> Option<u32> {
        self.lit.get(&pulse).map(|l| l.beat)
    }

    /// Flash strength of `pulse` in 0..=1: 1 on a beat, springing back to 0.
    pub fn flash(&self, pulse: PulseId) -> f32 {
        self.lit
            .get(&pulse)
            .map_or(0.0, |l| l.flash.value.clamp(0.0, 1.0))
    }

    /// Beats queued and not yet lit.
    pub fn pending_beats(&self) -> usize {
        self.pending.len()
    }

    /// How active dot `beat` of `pulse` is: 0 idle, 1 lit (springs, may overshoot a little).
    pub fn dot(&self, pulse: PulseId, beat: u32) -> f32 {
        self.dots
            .get(&pulse)
            .and_then(|springs| springs.get(beat as usize))
            .map_or(0.0, |s| s.value.clamp(0.0, 1.5))
    }

    /// True when every enabled pulse is on beat 0 and all those hits share one audio frame.
    pub fn coincidence(&self) -> bool {
        if !self.playing {
            return false;
        }
        let mut frame = None;
        for pulse in self.pattern.pulses.iter().filter(|p| p.enabled) {
            match self.lit.get(&pulse.id) {
                Some(l) if l.beat == 0 && frame.is_none_or(|f| f == l.frame) => {
                    frame = Some(l.frame);
                }
                _ => return false,
            }
        }
        frame.is_some()
    }

    /// Strongest flash among enabled pulses sitting on beat 0 (0 if none): the
    /// central glow of the topology.
    pub fn downbeat_flash(&self) -> f32 {
        self.pattern
            .pulses
            .iter()
            .filter(|p| p.enabled)
            .filter_map(|p| self.lit.get(&p.id).filter(|l| l.beat == 0))
            .map(|l| l.flash.value.clamp(0.0, 1.0))
            .fold(0.0, f32::max)
    }

    /// True while a beat is waiting to light or a flash is still moving,
    /// so the app keeps repainting.
    pub fn animating(&self) -> bool {
        !self.pending.is_empty()
            || self.lit.values().any(|l| !l.flash.settled())
            || self.dots.values().flatten().any(|s| !s.settled())
    }

    /// Unlights everything and drops queued beats (on stop).
    pub fn clear_beats(&mut self) {
        self.pending.clear();
        self.lit.clear();
        self.dots.clear();
    }

    /// A tap at `now` (the TAP button, the global hotkey): pushes `Tap` and,
    /// when the taps give a tempo, applies it and pushes `SetBpm`.
    pub fn tap(&mut self, now: Instant, actions: &mut Vec<UiAction>) {
        let at = now.saturating_duration_since(self.tap_epoch);
        actions.push(UiAction::Tap(at));
        if let Some(bpm) = self.tap.tap(at) {
            self.bpm = bpm;
            actions.push(UiAction::SetBpm(bpm));
        }
    }

    /// Whether the settings panel of `pulse` is open.
    pub fn settings_open(&self, pulse: PulseId) -> bool {
        self.settings_open.contains(&pulse)
    }

    pub(crate) fn toggle_settings(&mut self, pulse: PulseId) {
        if !self.settings_open.remove(&pulse) {
            self.settings_open.insert(pulse);
        }
    }
}
