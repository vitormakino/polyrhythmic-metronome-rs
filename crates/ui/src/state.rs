//! What the views draw from and edit: the pattern, the tempo, transport and
//! window mode, plus the visual beat state (which beat is lit, hit ages)
//! kept in sync with the audio through `visual_offset`.
//!
//! Views never talk to the audio engine: they edit [`UiState`] and return
//! [`UiAction`]s for the app to forward.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pm_audio::BeatEvent;
use pm_core::session::{PulseMask, SessionProgress};
use pm_core::{Bpm, Pattern, PulseId, TapTempo};
use serde::{Deserialize, Serialize};

use crate::clock::MeasureClock;
use crate::i18n::Messages;
use crate::theme::{DARK, Theme};
use crate::views::topology::TopologyKind;

/// A practice session the user can start: the name shown in the list and the
/// plan it runs. The plan is shared and never mutated, so a session that is
/// running is not affected when its file is edited.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionEntry {
    pub name: String,
    pub plan: Arc<pm_core::session::SessionPlan>,
}

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
    /// The user picked another session (or none); `UiState::session` already holds it.
    SelectSession,
    /// The user turned the pulse at this position (0-based) on or off while
    /// the running stage overrides it: the user wins until the stage ends.
    /// `PatternChanged` follows with the pattern's new `enabled`.
    TakeOverPulse(usize),
}

/// Time constant of the flash envelope, in seconds.
const FLASH_TAU: f32 = 0.12;
/// Time constant of a dot lighting up, in seconds.
const DOT_RISE_TAU: f32 = 0.03;
/// Time constant of the previous beat's dot fading out, in seconds.
const DOT_FALL_TAU: f32 = 0.06;
/// A hit younger than this keeps the app repainting, in seconds.
const ANIMATING_SECS: f32 = 1.0;

/// The last hit of a pulse, as shown: which beat, which audio frame and how
/// long ago it touched its mark.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// Beat index within the pulse.
    pub beat: u32,
    /// Audio frame of the hit, to tell hits of one instant from stale ones.
    pub frame: u64,
    /// Seconds since the hit's `at + visual_offset`, as of the last `advance`.
    pub age: f32,
}

/// The beat a pulse lit last, the one before it, and their ages.
#[derive(Clone, Copy, Debug)]
struct Lit {
    hit: Hit,
    /// Beat lit before this one, still fading out.
    prev: Option<u32>,
    /// The hit's `ev.at`; the age counts from here in the shown time domain.
    at: Instant,
    /// The pulse was shown enabled at the last `advance`.
    on: bool,
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
    /// Why the output could not be opened, shown on hover over the
    /// "no audio output" status.
    pub audio_error: Option<String>,
    tap: TapTempo,
    tap_epoch: Instant,
    pending: VecDeque<BeatEvent>,
    lit: HashMap<PulseId, Lit>,
    settings_open: HashSet<PulseId>,
    /// Continuous measure position, fed by every beat event while playing.
    clock: MeasureClock,
    /// `playing` as of the last `advance`, to catch the Play and Stop edges.
    was_playing: bool,
    /// The measure length last given to the clock, in seconds.
    measure_secs: Option<f64>,
    /// Measure phase at the last `advance`.
    measure_phase: Option<f64>,
    /// Whole measures since Play (or since the last `clear_beats`), from the
    /// clock's unwrapped position.
    measure_count: u64,
    /// The clock's whole measures that count as 0; `None` until the first
    /// `advance` after Play or `clear_beats` sets it.
    count_origin: Option<f64>,
    /// The `now` of the last `advance`.
    now: Instant,
    /// While a pulse card is dragged by its handle: (from, current target) indices.
    pub(crate) dragging: Option<(usize, usize)>,
    /// Every text the views show. Assigning a new value switches the language;
    /// no action is involved.
    pub messages: Arc<Messages>,
    /// Every color the views paint with. Assigning a new value switches the
    /// theme (call `theme::apply` too, for the egui visuals); no action is involved.
    pub theme: Arc<Theme>,
    /// Sessions the user can pick, in the order shown. Empty: no session strip.
    pub sessions: Arc<[SessionEntry]>,
    /// Name of the selected session; `None` is "No session".
    pub session: Option<String>,
    /// Where the engine's session is, set by the app every frame.
    pub session_progress: Option<SessionProgress>,
    /// The progress of the session that finished, shown until the next Play
    /// or another selection.
    pub session_summary: Option<SessionProgress>,
    /// The session the last Play started, as it was sent (set by the app).
    /// While it plays, and with its summary, the views describe it rather
    /// than the selected entry, whose file may have changed or gone since.
    pub running_plan: Option<SessionEntry>,
    /// Show the pin-to-mini button in the tempo card (off where there is no mini window).
    pub show_mini_button: bool,
    /// Show the session strip in the full window (off where sessions are not offered).
    pub show_sessions: bool,
    /// The drawing the full window shows (the mini always shows the circle).
    pub topology: TopologyKind,
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
            settings_open: HashSet::new(),
            clock: MeasureClock::default(),
            was_playing: false,
            measure_secs: None,
            measure_phase: None,
            measure_count: 0,
            count_origin: None,
            now: Instant::now(),
            dragging: None,
            messages: Arc::new(Messages::english()),
            theme: Arc::new(DARK),
            sessions: Arc::new([]),
            session: None,
            session_progress: None,
            session_summary: None,
            running_plan: None,
            show_mini_button: true,
            show_sessions: true,
            topology: TopologyKind::Circle,
        }
    }
}

impl UiState {
    /// Queues a beat from the engine; it lights in [`Self::advance`] once its time comes.
    /// Ignored while stopped, so beats still in flight after Stop never light.
    /// Every event, muted or not, anchors the measure clock first.
    /// At most [`PENDING_BEATS_CAP`] beats wait; the oldest is dropped first.
    /// A hit the session silenced (`muted`) does not light, unless the
    /// session plan shows muted visuals.
    pub fn on_beat(&mut self, ev: &BeatEvent) {
        if !self.playing {
            return;
        }
        // Muted hits still say where the measure is, lit or not.
        if let Some(pulse) = self.pattern.pulse(ev.pulse) {
            self.clock.anchor(ev.at, ev.beat, pulse.beats);
        }
        if ev.muted
            && !self
                .shown_session()
                .is_some_and(|s| s.plan.show_muted_visuals)
        {
            return;
        }
        if self.pending.len() == PENDING_BEATS_CAP {
            self.pending.pop_front();
        }
        self.pending.push_back(*ev);
    }

    /// Lights every queued beat whose `at + visual_offset` has passed and
    /// ages every hit to the shown time (`now - visual_offset`). `dt` is unused:
    /// the lights are functions of time, not integrated.
    pub fn advance(&mut self, now: Instant, _dt: f32) {
        let offset = self.visual_offset;
        self.now = now;
        let shown_at = now.checked_sub(offset).unwrap_or(now);
        if self.playing != self.was_playing {
            if self.playing {
                // Hold for one reference beat at the shown tempo.
                let beat = 60.0 / f64::from(self.shown_bpm().get());
                self.clock.start(shown_at, beat);
            } else {
                self.clock.stop();
            }
            self.measure_count = 0;
            self.count_origin = None;
            self.was_playing = self.playing;
        }
        let reference = self.pattern.reference_beats();
        if reference > 0 {
            let secs = f64::from(reference) * 60.0 / f64::from(self.shown_bpm().get());
            if self.measure_secs != Some(secs) {
                self.clock.set_measure(shown_at, secs);
                self.measure_secs = Some(secs);
            }
        }
        let position = self.clock.position(shown_at);
        self.measure_phase = self.clock.phase(shown_at);
        if let Some(p) = position {
            let origin = *self.count_origin.get_or_insert(p.floor());
            // The count follows the position both ways, so it always agrees
            // with the phase, even across a hitch or a correction.
            self.measure_count = (p.floor() - origin).max(0.0) as u64;
        }
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
            let prev = self.lit.get(&ev.pulse).map(|l| l.hit.beat);
            self.lit.insert(
                ev.pulse,
                Lit {
                    hit: Hit {
                        beat: ev.beat,
                        frame: ev.frame,
                        age: 0.0,
                    },
                    prev,
                    at: ev.at,
                    on: true,
                },
            );
        }
        let shown = self.shown_enabled();
        let pattern = &self.pattern;
        self.lit.retain(|id, _| pattern.pulse(*id).is_some());
        for (pulse, &enabled) in pattern.pulses.iter().zip(&shown) {
            if let Some(lit) = self.lit.get_mut(&pulse.id) {
                lit.hit.age = shown_at.saturating_duration_since(lit.at).as_secs_f32();
                lit.on = enabled;
            }
        }
    }

    /// Whole measures shown since Play: 0 at the start, +1 each time
    /// [`Self::measure_phase`] crosses the bar line forwards (and -1 if a
    /// correction takes it back across). Counted from the clock's unwrapped
    /// position, not from wraps between frames. 0 while stopped, and again
    /// from 0 after [`Self::clear_beats`].
    pub fn measure_count(&self) -> u64 {
        self.measure_count
    }

    /// Where the measure is, in `[0, 1)`, at the shown time of the last
    /// [`Self::advance`]. `None` while stopped.
    pub fn measure_phase(&self) -> Option<f64> {
        self.measure_phase
    }

    /// Position of the pulse at `index` in its own cycle: the integer part is
    /// its beat index, the fraction the progress to the next beat.
    pub fn pulse_phase(&self, index: usize) -> Option<f64> {
        let beats = self.pattern.pulses.get(index)?.beats;
        self.measure_phase.map(|p| p * f64::from(beats))
    }

    /// The `now` of the last [`Self::advance`].
    pub fn now(&self) -> Instant {
        self.now
    }

    /// The beat of `pulse` that is lit now, if any.
    pub fn active_beat(&self, pulse: PulseId) -> Option<u32> {
        self.lit.get(&pulse).map(|l| l.hit.beat)
    }

    /// The last hit of `pulse` that was shown, with its age.
    pub fn hit(&self, pulse: PulseId) -> Option<Hit> {
        self.lit.get(&pulse).map(|l| l.hit)
    }

    /// The beat `pulse` lit before its last hit, still fading out; `None`
    /// for the first hit after Play or a reset.
    pub fn prev_beat(&self, pulse: PulseId) -> Option<u32> {
        self.lit.get(&pulse).and_then(|l| l.prev)
    }

    /// A stable seed for the effects of one hit, from the pulse and the audio
    /// frame. A pure hash (FNV-1a), no RNG: equal inputs always give equal seeds.
    pub fn seed(pulse: PulseId, frame: u64) -> u64 {
        struct Fnv(u64);
        impl Hasher for Fnv {
            fn finish(&self) -> u64 {
                self.0
            }
            fn write(&mut self, bytes: &[u8]) {
                for &b in bytes {
                    self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
        }
        let mut h = Fnv(0xcbf2_9ce4_8422_2325);
        pulse.hash(&mut h);
        frame.hash(&mut h);
        h.finish()
    }

    /// Flash strength of `pulse` in 0..=1: 1 on a beat, `exp(-age / 0.12)` after.
    pub fn flash(&self, pulse: PulseId) -> f32 {
        self.lit.get(&pulse).map_or(0.0, |l| flash_of(l.hit.age))
    }

    /// Beats queued and not yet lit.
    pub fn pending_beats(&self) -> usize {
        self.pending.len()
    }

    /// How active dot `beat` of `pulse` is: 0 idle, 1 lit. The lit beat rises
    /// as `1 - exp(-age / 0.03)`; the one before it fades as `exp(-age / 0.06)`
    /// of the newer hit's age. A pulse not shown enabled stays at 0.
    pub fn dot(&self, pulse: PulseId, beat: u32) -> f32 {
        let Some(l) = self.lit.get(&pulse).filter(|l| l.on) else {
            return 0.0;
        };
        if self.pattern.pulse(pulse).is_none_or(|p| beat >= p.beats) {
            return 0.0;
        }
        let age = l.hit.age.max(0.0);
        if beat == l.hit.beat {
            1.0 - (-age / DOT_RISE_TAU).exp()
        } else if l.prev == Some(beat) {
            (-age / DOT_FALL_TAU).exp()
        } else {
            0.0
        }
    }

    /// True when every enabled pulse is on beat 0 and all those hits share one audio frame.
    pub fn coincidence(&self) -> bool {
        self.coincident_hit().is_some()
    }

    /// Age of the hit that made [`Self::coincidence`] true, in seconds.
    pub fn coincidence_age(&self) -> Option<f32> {
        self.coincident_hit().map(|h| h.age)
    }

    /// The shared hit of a coincidence: the youngest of the hits that agree.
    fn coincident_hit(&self) -> Option<Hit> {
        if !self.playing {
            return None;
        }
        let mut found: Option<Hit> = None;
        let shown = self.shown_enabled();
        for (pulse, _) in self.pattern.pulses.iter().zip(shown).filter(|(_, on)| *on) {
            match self.lit.get(&pulse.id) {
                Some(l) if l.hit.beat == 0 && found.is_none_or(|f| f.frame == l.hit.frame) => {
                    found = Some(match found {
                        Some(f) if f.age <= l.hit.age => f,
                        _ => l.hit,
                    });
                }
                _ => return None,
            }
        }
        found
    }

    /// Strongest flash among enabled pulses sitting on beat 0 (0 if none): the
    /// central glow of the topology.
    pub fn downbeat_flash(&self) -> f32 {
        self.pattern
            .pulses
            .iter()
            .zip(self.shown_enabled())
            .filter(|(_, on)| *on)
            .filter_map(|(p, _)| self.lit.get(&p.id).filter(|l| l.hit.beat == 0))
            .map(|l| flash_of(l.hit.age))
            .fold(0.0, f32::max)
    }

    /// True while a beat is waiting to light or a hit is younger than a second,
    /// so the app keeps repainting.
    pub fn animating(&self) -> bool {
        !self.pending.is_empty() || self.lit.values().any(|l| l.hit.age < ANIMATING_SECS)
    }

    /// Unlights everything and drops queued beats (on stop).
    pub fn clear_beats(&mut self) {
        self.pending.clear();
        self.lit.clear();
        self.clock.reset();
        self.measure_count = 0;
        self.count_origin = None;
    }

    /// A tap at `now` (the TAP button, the global hotkey): pushes `Tap` and,
    /// when the taps give a tempo, applies it and pushes `SetBpm`.
    pub fn tap(&mut self, now: Instant, actions: &mut Vec<UiAction>) {
        let at = now.saturating_duration_since(self.tap_epoch);
        actions.push(UiAction::Tap(at));
        if let Some(bpm) = self.tap.tap(at) {
            self.set_shown_bpm(bpm, actions);
        }
    }

    /// The selected session's entry, if it is still in [`Self::sessions`].
    pub fn selected_session(&self) -> Option<&SessionEntry> {
        let name = self.session.as_deref()?;
        self.sessions.iter().find(|s| s.name == name)
    }

    /// The session the views describe: the one the last Play started
    /// ([`Self::running_plan`]) while playing or showing its summary, else
    /// the selected one.
    pub fn shown_session(&self) -> Option<&SessionEntry> {
        self.running_plan
            .as_ref()
            .filter(|_| self.playing || self.session_summary.is_some())
            .or_else(|| self.selected_session())
    }

    /// The progress of the session playing now. A finished one still counts
    /// while playing (`OnFinish::Continue`): the base setup comes back only on Stop.
    pub fn running_session(&self) -> Option<&SessionProgress> {
        self.session_progress.as_ref().filter(|_| self.playing)
    }

    /// The tempo the views show: the session's while one runs, else the base.
    pub fn shown_bpm(&self) -> Bpm {
        self.running_session().map_or(self.bpm, |p| p.moment.bpm)
    }

    /// The user set the shown tempo to `new`: the base moves by the same
    /// amount (clamped), so the session's own changes never reach the setup,
    /// and `SetBpm(new)` is pushed. Without a running session this is
    /// `bpm = new`.
    pub fn set_shown_bpm(&mut self, new: Bpm, actions: &mut Vec<UiAction>) {
        let shift = i32::from(new.get()) - i32::from(self.shown_bpm().get());
        self.bpm = Bpm::new(i32::from(self.bpm.get()) + shift);
        if self.running_session().is_some()
            && let Some(p) = &mut self.session_progress
        {
            // Shown at once; the engine's next progress confirms it.
            p.moment.bpm = new;
        }
        actions.push(UiAction::SetBpm(new));
    }

    /// Whether pulse `index` (0-based) shows as on: the pattern's `enabled`
    /// with the running session's mask applied (`off` hides it, `forced_on`
    /// shows it).
    pub fn pulse_shown_enabled(&self, index: usize) -> bool {
        let Some(pulse) = self.pattern.pulses.get(index) else {
            return false;
        };
        let mask = self.running_session().map(|p| p.moment.pulses);
        shown(pulse.enabled, mask, index)
    }

    /// Whether the running session's stage overrides pulse `index` (0-based):
    /// off, muted or forced on.
    pub fn pulse_overridden(&self, index: usize) -> bool {
        let bit = 1u16.checked_shl(index as u32).unwrap_or(0);
        self.running_session().is_some_and(|p| {
            (p.moment.pulses.off | p.moment.pulses.muted | p.moment.pulses.forced_on) & bit != 0
        })
    }

    /// The user turned pulse `index` on or off. If the running stage
    /// overrides it, the user takes it over until the stage ends: the
    /// override is dropped from the shown progress at once (the engine's
    /// next progress confirms it) and `TakeOverPulse(index)` is pushed. The
    /// caller sets the pattern's `enabled` to what the user clicked.
    pub fn take_over_pulse(&mut self, index: usize, actions: &mut Vec<UiAction>) {
        if !self.pulse_overridden(index) {
            return;
        }
        if let Some(p) = &mut self.session_progress {
            let keep = !(1u16 << index);
            p.moment.pulses.off &= keep;
            p.moment.pulses.muted &= keep;
            p.moment.pulses.forced_on &= keep;
        }
        actions.push(UiAction::TakeOverPulse(index));
    }

    fn shown_enabled(&self) -> Vec<bool> {
        (0..self.pattern.pulses.len())
            .map(|i| self.pulse_shown_enabled(i))
            .collect()
    }

    /// Keeps a finished progress as the summary, so it outlives the app
    /// dropping the progress after the session stopped itself.
    pub(crate) fn keep_session_summary(&mut self) {
        if let Some(p) = self.session_progress.filter(|p| p.finished) {
            self.session_summary = Some(p);
        }
    }

    /// Selects session `name` (`None`: no session) and forgets the summary
    /// and the last run's plan.
    pub(crate) fn select_session(&mut self, name: Option<String>, actions: &mut Vec<UiAction>) {
        if self.session != name {
            self.session = name;
            self.session_summary = None;
            self.running_plan = None;
            actions.push(UiAction::SelectSession);
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

/// `enabled` as `mask` overrides it at position `index`.
/// Flash envelope of a hit `age` seconds old.
fn flash_of(age: f32) -> f32 {
    (-age.max(0.0) / FLASH_TAU).exp().clamp(0.0, 1.0)
}

fn shown(enabled: bool, mask: Option<PulseMask>, index: usize) -> bool {
    let Some(mask) = mask else {
        return enabled;
    };
    let bit = 1u16.checked_shl(index as u32).unwrap_or(0);
    if mask.off & bit != 0 {
        false
    } else if mask.forced_on & bit != 0 {
        true
    } else {
        enabled
    }
}
