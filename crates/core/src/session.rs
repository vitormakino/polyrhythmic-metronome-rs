//! Practice sessions: a staged plan and the pure state machine that plays it.
//!
//! A [`SessionPlan`] is a list of [`Stage`]s. Each stage can jump the tempo,
//! ramp it, mute whole bars on a cycle or at random, and turn individual
//! pulses off or muted. The [`Runner`] is driven by the audio engine: it is
//! called at the exact frame of every reference "one" ([`Runner::bar`]) and,
//! for smooth ramps, at every beat of pulse 1 ([`Runner::beat`]). It is `Copy`
//! and heap-free so it can live on the real-time audio thread.

use crate::model::{Bpm, MAX_PULSES};

/// A length in bars or in seconds. The amount is always above 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    Bars(u32),
    Seconds(u32),
}

impl Span {
    fn amount(self) -> u32 {
        match self {
            Span::Bars(n) | Span::Seconds(n) => n.max(1),
        }
    }
}

/// One-off tempo change at the start of a stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    By(i32),
    To(Bpm),
}

/// Tempo change over the stage: `change` BPM every `every`, optionally up to
/// `limit` (a maximum when `change > 0`, a minimum when `change < 0`). A
/// smooth ramp moves in whole-BPM steps at every pulse-1 beat instead of one
/// step per `every`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ramp {
    pub change: i32,
    pub every: Span,
    pub limit: Option<Bpm>,
    pub smooth: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tempo {
    pub jump: Option<Jump>,
    pub ramp: Option<Ramp>,
}

/// Plays `play`, then mutes `mute`, over and over. Both sides use the same unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MuteCycle {
    pub play: Span,
    pub mute: Span,
}

/// Mutes a share of the bars at random: `percent`, plus `increase` every
/// `every_bars` bars, capped at `max`. `increase == 0` keeps it fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RandomMute {
    pub percent: u8,
    pub increase: u8,
    pub every_bars: u32,
    pub max: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PulseState {
    Off,
    Muted,
    On,
}

/// What happens after the last stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnFinish {
    Stop,
    Continue,
    Repeat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stage {
    pub length: Span,
    pub tempo: Option<Tempo>,
    pub mute: Option<MuteCycle>,
    pub random_mute: Option<RandomMute>,
    /// Per pulse position (0-based): `None` leaves the pulse as the pattern has it.
    pub pulses: [Option<PulseState>; MAX_PULSES],
}

/// A whole session. `stages` is never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionPlan {
    pub name: String,
    pub stages: Vec<Stage>,
    pub on_finish: OnFinish,
    pub show_muted_visuals: bool,
    /// Seed of the random-mute draws.
    pub seed: u64,
}

/// Pulse overrides for one bar; bit `i` is pulse position `i` (0-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PulseMask {
    pub off: u16,
    pub muted: u16,
    pub forced_on: u16,
}

/// What the engine plays for one bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moment {
    pub bpm: Bpm,
    pub pulses: PulseMask,
    /// The whole bar is muted by the stage's mute cycle.
    pub silent: bool,
    /// Chance, in percent, of each pulse's bar being randomly muted.
    pub random_percent: u8,
}

/// Snapshot for the UI and for [`Runner::resume`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionProgress {
    /// Current stage, counting from 1 (0 for an empty plan).
    pub stage: u32,
    pub stages: u32,
    /// Bars completed in the stage when the latest bar started (0 on its first bar).
    pub bars_in_stage: u32,
    /// Frames from the stage start to the latest call, at `sample_rate`.
    pub frames_in_stage: u64,
    pub sample_rate: u32,
    pub moment: Moment,
    /// Highest effective tempo seen, the user's changes included.
    pub max_bpm: Bpm,
    /// Frames from the session start to the latest call, at `sample_rate`.
    pub total_frames: u64,
    /// The last stage ended (`Stop` or `Continue`).
    pub finished: bool,
    /// Everything `resume` needs beyond the public fields.
    state: Runner,
}

/// Length of [`SessionProgress::to_words`].
pub const PROGRESS_WORDS: usize = 12;

impl SessionProgress {
    /// The snapshot as plain words, so it can cross threads through atomics
    /// (a seqlock) without copying its bytes. [`from_words`](Self::from_words)
    /// gives it back: `frames_in_stage` and `total_frames` as stored, every
    /// other public field derived from the runner state, as
    /// [`Runner::progress`] does. Never allocates.
    pub fn to_words(&self) -> [u64; PROGRESS_WORDS] {
        let r = &self.state;
        let lo_hi = |lo: u32, hi: u32| u64::from(lo) | u64::from(hi) << 32;
        let flags = u64::from(r.silent)
            | u64::from(r.first) << 1
            | u64::from(r.frozen) << 2
            | u64::from(r.finished) << 3;
        [
            lo_hi(r.sample_rate, r.stage),
            lo_hi(r.stages, r.bars_in_stage),
            r.stage_start,
            r.session_start,
            r.last_frame,
            u64::from(r.current.get())
                | u64::from(r.max_bpm.get()) << 16
                | u64::from(r.user_bits) << 32
                | u64::from(r.random_percent) << 48
                | flags << 56,
            lo_hi(r.origin as u32, r.ramp_delta as u32),
            u64::from(r.base as u32) | u64::from(r.mask.off) << 32 | u64::from(r.mask.muted) << 48,
            u64::from(r.mask.forced_on),
            r.rng,
            self.frames_in_stage,
            self.total_frames,
        ]
    }

    /// Inverse of [`to_words`](Self::to_words).
    pub fn from_words(w: &[u64; PROGRESS_WORDS]) -> SessionProgress {
        let lo = |x: u64| x as u32;
        let hi = |x: u64| (x >> 32) as u32;
        let bpm = |x: u64| Bpm::new(i32::from(x as u16));
        let flags = w[5] >> 56;
        let r = Runner {
            sample_rate: lo(w[0]),
            stage: hi(w[0]),
            stages: lo(w[1]),
            bars_in_stage: hi(w[1]),
            stage_start: w[2],
            session_start: w[3],
            last_frame: w[4],
            current: bpm(w[5]),
            max_bpm: bpm(w[5] >> 16),
            user_bits: (w[5] >> 32) as u16,
            random_percent: (w[5] >> 48) as u8,
            silent: flags & 1 != 0,
            first: flags & 2 != 0,
            frozen: flags & 4 != 0,
            finished: flags & 8 != 0,
            origin: lo(w[6]) as i32,
            ramp_delta: hi(w[6]) as i32,
            base: lo(w[7]) as i32,
            mask: PulseMask {
                off: (w[7] >> 32) as u16,
                muted: (w[7] >> 48) as u16,
                forced_on: w[8] as u16,
            },
            rng: w[9],
        };
        SessionProgress {
            frames_in_stage: w[10],
            total_frames: w[11],
            ..r.progress()
        }
    }
}

const DEFAULT_RNG_STATE: u64 = 0x9E37_79B9_7F4A_7C15;

/// The session state machine. `Copy`, no heap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Runner {
    sample_rate: u32,
    /// Current stage, counting from 1.
    stage: u32,
    stages: u32,
    bars_in_stage: u32,
    stage_start: u64,
    session_start: u64,
    last_frame: u64,
    current: Bpm,
    /// Tempo the ramp counts from; the user's changes shift it.
    origin: i32,
    /// What the ramp added to `origin` at the latest bar or beat, before clamping.
    ramp_delta: i32,
    /// Tempo `Repeat` restarts from: the start base plus the user's changes.
    base: i32,
    max_bpm: Bpm,
    rng: u64,
    /// Positions the user took over until the stage ends.
    user_bits: u16,
    /// The stage's own pulse overrides, before the user's.
    mask: PulseMask,
    silent: bool,
    random_percent: u8,
    /// The first `bar()` is the stage's bar 0, not a new bar.
    first: bool,
    /// Past the last stage with `Stop`/`Continue`: the ramp no longer moves.
    frozen: bool,
    finished: bool,
}

impl Runner {
    /// Starts a session whose first "one" is at `frame`; the first
    /// [`bar`](Self::bar) call is for that frame and returns stage 1, bar 0.
    pub fn start(plan: &SessionPlan, base: Bpm, sample_rate: u32, frame: u64) -> Runner {
        let mut r = Runner {
            sample_rate: sample_rate.max(1),
            stage: 0,
            stages: 0,
            bars_in_stage: 0,
            stage_start: frame,
            session_start: frame,
            last_frame: frame,
            current: base,
            origin: i32::from(base.get()),
            ramp_delta: 0,
            base: i32::from(base.get()),
            max_bpm: base,
            rng: if plan.seed == 0 {
                DEFAULT_RNG_STATE
            } else {
                plan.seed
            },
            user_bits: 0,
            mask: PulseMask::default(),
            silent: false,
            random_percent: 0,
            first: true,
            frozen: false,
            finished: false,
        };
        r.begin_stage(plan, 0, frame, base);
        r
    }

    /// At the frame of every "one". Returns the moment for this bar; `Some(true)`
    /// in the second field means "finished and must stop now" (`OnFinish::Stop`),
    /// `Some(false)` finished and continuing.
    pub fn bar(&mut self, plan: &SessionPlan, frame: u64) -> (Moment, Option<bool>) {
        self.last_frame = frame;
        if self.first {
            self.first = false;
        } else {
            self.bars_in_stage = self.bars_in_stage.saturating_add(1);
        }
        let mut outcome = None;
        if !self.frozen && self.stage_ended(plan, frame) {
            if (self.stage as usize) < plan.stages.len() {
                let next = self.stage as usize;
                self.begin_stage(plan, next, frame, self.current);
            } else {
                match plan.on_finish {
                    OnFinish::Stop => {
                        self.freeze();
                        outcome = Some(true);
                    }
                    OnFinish::Continue => {
                        self.freeze();
                        outcome = Some(false);
                    }
                    OnFinish::Repeat => {
                        self.begin_stage(plan, 0, frame, Bpm::new(self.base));
                    }
                }
            }
        }
        self.update_bar(plan, frame);
        (self.moment(), outcome)
    }

    /// At the frame of every non-zero beat of pulse 1; only matters when the
    /// stage ramp is smooth.
    pub fn beat(
        &mut self,
        plan: &SessionPlan,
        frame: u64,
        beat: u32,
        beats_per_bar: u32,
    ) -> Moment {
        self.last_frame = frame;
        if !self.frozen
            && let Some(ramp) = self.ramp(plan)
            && ramp.smooth
        {
            self.apply_ramp(&ramp, frame, beat, beats_per_bar);
        }
        self.moment()
    }

    /// The user set the effective tempo: the ramp continues from there.
    pub fn user_bpm(&mut self, effective: Bpm) {
        let shift = i32::from(effective.get()) - i32::from(self.current.get());
        // Re-anchor on the delta the ramp applies now, not on the clamped
        // tempo: past a limit the two differ and the ramp would snap back.
        self.origin = i32::from(effective.get()).saturating_sub(self.ramp_delta);
        self.base = self.base.saturating_add(shift);
        self.set_current(effective);
    }

    /// The user wins on this pulse position (0-based, as in [`PulseMask`])
    /// until the stage ends.
    pub fn user_pulse(&mut self, position: usize) {
        if position < MAX_PULSES {
            self.user_bits |= 1 << position;
        }
    }

    /// One random-mute draw: true with probability `percent` / 100. A
    /// xorshift64* stream seeded from the plan, so a seed replays the same bars.
    pub fn draw(&mut self, percent: u8) -> bool {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        let out = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (out >> 33) % 100 < u64::from(percent)
    }

    pub fn progress(&self) -> SessionProgress {
        SessionProgress {
            stage: self.stage,
            stages: self.stages,
            bars_in_stage: self.bars_in_stage,
            frames_in_stage: self.last_frame.saturating_sub(self.stage_start),
            sample_rate: self.sample_rate,
            moment: self.moment(),
            max_bpm: self.max_bpm,
            total_frames: self.last_frame.saturating_sub(self.session_start),
            finished: self.finished,
            state: *self,
        }
    }

    /// Rebuilds a runner from a snapshot, for a new sample rate. `frame` is
    /// the instant the snapshot describes (its latest bar or beat), so a
    /// seconds-based stage ends at the same musical time.
    pub fn resume(progress: &SessionProgress, sample_rate: u32, frame: u64) -> Runner {
        let sample_rate = sample_rate.max(1);
        let old = u128::from(progress.sample_rate.max(1));
        let rescale = |frames: u64| (u128::from(frames) * u128::from(sample_rate) / old) as u64;
        let mut r = progress.state;
        r.sample_rate = sample_rate;
        r.last_frame = frame;
        r.stage_start = frame.saturating_sub(rescale(progress.frames_in_stage));
        r.session_start = frame.saturating_sub(rescale(progress.total_frames));
        r
    }

    fn moment(&self) -> Moment {
        let keep = !self.user_bits;
        Moment {
            bpm: self.current,
            pulses: PulseMask {
                off: self.mask.off & keep,
                muted: self.mask.muted & keep,
                forced_on: self.mask.forced_on & keep,
            },
            silent: self.silent,
            random_percent: self.random_percent,
        }
    }

    fn stage_def<'a>(&self, plan: &'a SessionPlan) -> Option<&'a Stage> {
        plan.stages.get((self.stage as usize).checked_sub(1)?)
    }

    fn ramp(&self, plan: &SessionPlan) -> Option<Ramp> {
        self.stage_def(plan)?.tempo?.ramp
    }

    fn frames_in_stage(&self, frame: u64) -> u64 {
        frame.saturating_sub(self.stage_start)
    }

    fn stage_ended(&self, plan: &SessionPlan, frame: u64) -> bool {
        let Some(stage) = self.stage_def(plan) else {
            return false;
        };
        match stage.length {
            Span::Bars(n) => self.bars_in_stage >= n,
            Span::Seconds(s) => {
                self.frames_in_stage(frame) >= u64::from(s) * u64::from(self.sample_rate)
            }
        }
    }

    fn freeze(&mut self) {
        self.frozen = true;
        self.finished = true;
    }

    /// Starts stage `index` (0-based) at `frame`, from the running tempo
    /// `running` with the stage's jump applied.
    fn begin_stage(&mut self, plan: &SessionPlan, index: usize, frame: u64, running: Bpm) {
        self.stage = index as u32 + 1;
        self.stages = plan.stages.len() as u32;
        self.bars_in_stage = 0;
        self.stage_start = frame;
        self.user_bits = 0;
        self.frozen = false;
        let stage = plan.stages.get(index);
        let running = i32::from(running.get());
        let start = match stage.and_then(|s| s.tempo).and_then(|t| t.jump) {
            Some(Jump::By(n)) => running.saturating_add(n),
            Some(Jump::To(b)) => i32::from(b.get()),
            None => running,
        };
        self.origin = i32::from(Bpm::new(start).get());
        self.ramp_delta = 0;
        self.set_current(Bpm::new(start));
        self.mask = stage.map_or(PulseMask::default(), |s| {
            let mut m = PulseMask::default();
            for (i, state) in s.pulses.iter().enumerate() {
                match state {
                    Some(PulseState::Off) => m.off |= 1 << i,
                    Some(PulseState::Muted) => m.muted |= 1 << i,
                    Some(PulseState::On) => m.forced_on |= 1 << i,
                    None => {}
                }
            }
            m
        });
    }

    /// Per-bar state: the ramp step, the mute cycle and the random percentage.
    fn update_bar(&mut self, plan: &SessionPlan, frame: u64) {
        let Some(stage) = self.stage_def(plan) else {
            return;
        };
        if !self.frozen
            && let Some(ramp) = stage.tempo.and_then(|t| t.ramp)
        {
            self.apply_ramp(&ramp, frame, 0, 1);
        }
        self.silent = stage.mute.is_some_and(|m| {
            let (play, cycle) = (
                u64::from(m.play.amount()),
                u64::from(m.play.amount()) + u64::from(m.mute.amount()),
            );
            let pos = match m.play {
                Span::Bars(_) => u64::from(self.bars_in_stage),
                Span::Seconds(_) => self.frames_in_stage(frame) / u64::from(self.sample_rate),
            };
            pos % cycle >= play
        });
        self.random_percent = stage.random_mute.map_or(0, |r| {
            let grown = u32::from(r.increase) * (self.bars_in_stage / r.every_bars.max(1));
            u32::from(r.percent)
                .saturating_add(grown)
                .min(u32::from(r.max)) as u8
        });
    }

    /// Sets the tempo the ramp gives `beat` beats (of `beats_per_bar`) into
    /// the current bar.
    fn apply_ramp(&mut self, ramp: &Ramp, frame: u64, beat: u32, beats_per_bar: u32) {
        let change = i128::from(ramp.change);
        let every = i128::from(ramp.every.amount());
        let sr = i128::from(self.sample_rate);
        let frames = i128::from(self.frames_in_stage(frame));
        let bars = i128::from(self.bars_in_stage);
        let delta = match (ramp.smooth, ramp.every) {
            (true, Span::Bars(_)) => {
                let bpb = i128::from(beats_per_bar.max(1));
                change * (bars * bpb + i128::from(beat)) / (every * bpb)
            }
            (true, Span::Seconds(_)) => change * frames / (every * sr),
            (false, Span::Bars(_)) => change * (bars / every),
            (false, Span::Seconds(_)) => change * (frames / sr / every),
        };
        self.ramp_delta = delta.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32;
        let mut bpm = i128::from(self.origin) + delta;
        if let Some(limit) = ramp.limit {
            let limit = i128::from(limit.get());
            if ramp.change > 0 {
                bpm = bpm.min(limit);
            } else if ramp.change < 0 {
                bpm = bpm.max(limit);
            }
        }
        let bpm = bpm.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32;
        self.set_current(Bpm::new(bpm));
    }

    fn set_current(&mut self, bpm: Bpm) {
        self.current = bpm;
        self.max_bpm = self.max_bpm.max(bpm);
    }
}
