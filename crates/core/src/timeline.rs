//! Exact, drift-free timeline.
//!
//! Musical phase is counted in integer ticks. With sample rate `sr` and
//! reference beats `R` (the first pulse's beat count), one measure is
//! `Q = R · 60 · sr` ticks and phase advances `bpm` ticks per frame. Every
//! hit frame is computed directly from an anchor
//! `(anchor_frame, anchor_measure, anchor_ticks)` with `i128` integer math,
//! so no error can accumulate however long the session runs. There are no
//! floats here except the hit gain.
//!
//! Live edits re-anchor. The phase is carried over exactly, and so is the set
//! of beats already emitted: the anchor remembers the phase up to which every
//! beat has been played (`done`), so a beat the old timeline would have fired
//! at the edit frame itself is never lost, and one already fired is never
//! repeated.

use crate::model::{Bpm, Intensity, MAX_PULSES, Pattern, PulseId, SoundType};

/// One click to be rendered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatHit {
    pub pulse: PulseId,
    /// Beat index within the measure, `0..beats`.
    pub beat: u32,
    /// Absolute frame at which the click starts.
    pub frame: u64,
    pub gain: f32,
    pub sound: SoundType,
    /// True for beat 0.
    pub accent: bool,
}

/// Position in the music at a given frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MusicalPosition {
    pub measure: u64,
    /// Phase within the measure, `0..ticks_per_measure`.
    pub ticks: u64,
    pub ticks_per_measure: u64,
}

impl MusicalPosition {
    /// The very start of `measure`, before its "one" has played, at any
    /// sample rate: `ticks_per_measure` is 0. Given to
    /// [`Timeline::resume`] it plays every pulse's beat 0 at the start frame.
    pub const fn start_of(measure: u64) -> MusicalPosition {
        MusicalPosition {
            measure,
            ticks: 0,
            ticks_per_measure: 0,
        }
    }
}

/// Where the tempo comes from. v1 uses the internal BPM; v2 may plug in Link.
pub trait TempoSource {
    fn bpm(&self) -> Bpm;
}

impl TempoSource for Bpm {
    fn bpm(&self) -> Bpm {
        *self
    }
}

/// How a pattern edit takes effect (see [`Timeline::apply_pattern`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyMode {
    /// The new pattern applies from the edit frame on.
    Now,
    /// The reference cycle changed: keep rendering the old pattern for frames
    /// before this one, the new pattern from it on. It is the next "one".
    AtNextOne(u64),
}

/// The point the whole timeline is computed from: at `frame`, the music is
/// `ticks` (in `0..Q`) into measure `measure`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor {
    frame: u64,
    measure: u64,
    ticks: u64,
    /// Every beat whose target, in ticks from the start of `measure`, is
    /// `<= done_num / done_den` has been emitted; the others have not. Beat
    /// targets `idx·Q/n` are rational, hence the strict bound. It is `ticks`
    /// minus less than one frame of phase (it may be negative), so at most
    /// one beat per pulse lies in `(done, ticks]`; that beat fires at
    /// `frame`. The denominator is 1 except after a sample-rate rebase.
    done_num: i128,
    done_den: i128,
}

impl Anchor {
    /// Measure `measure` starts exactly at `frame`, nothing emitted yet.
    fn measure_start(frame: u64, measure: u64) -> Anchor {
        Anchor {
            frame,
            measure,
            ticks: 0,
            // Any value in (-Q/n, 0): the beat at tick 0 is still to come.
            done_num: -1,
            done_den: 1,
        }
    }
}

/// A reference-cycle change waiting for measure `measure` to start.
#[derive(Clone, Copy, Debug)]
struct Switch {
    measure: u64,
    /// New reference beat count `R`.
    reference: u64,
}

/// Per-pulse "next beat" cursor. It is only a cache: `events_in` checks it
/// against `from` and re-seeks with the closed formula when it does not fit,
/// so the output never depends on how the caller chunks time.
#[derive(Clone, Copy, Debug)]
struct Cursor {
    /// Beat count the cursor was computed for (0 = invalid).
    beats: u32,
    /// Beat index counted from the start of the anchor measure.
    idx: u64,
    /// Frame of beat `idx`.
    frame: u64,
    /// Frame of beat `idx - 1`, or `None` if `idx` is the first beat at or
    /// after the anchor.
    prev: Option<u64>,
}

impl Cursor {
    const INVALID: Cursor = Cursor {
        beats: 0,
        idx: 0,
        frame: 0,
        prev: None,
    };
}

/// Hit schedule for a pattern, computed from an anchor
/// `(frame, measure, ticks)` point and the current BPM. Live edits (tempo,
/// pattern, sample rate) re-anchor at the edit frame.
///
/// `set_bpm`, `apply_pattern`, `rebase_sample_rate`, `pending_switch`,
/// `restart` and `events_in` never allocate, nor does `sync_pattern` up to
/// [`MAX_PULSES`] pulses, so they can run on the real-time thread.
pub struct Timeline {
    sample_rate: u32,
    bpm: Bpm,
    /// Ticks per measure, `R · 60 · sr`.
    q: u64,
    anchor: Anchor,
    cursors: Vec<Cursor>,
    /// Per pulse index: absolute measure from which the pulse is audible
    /// (pulses added live wait for the next "one"). Same length as `cursors`.
    starts: Vec<u64>,
    /// Scratch space for re-indexing `starts` without allocating.
    scratch: Vec<u64>,
    switch: Option<Switch>,
}

impl Timeline {
    /// Starts measure 0 exactly at `start_frame`. Room for [`MAX_PULSES`]
    /// pulses is reserved here, so [`sync_pattern`](Self::sync_pattern) with
    /// a pattern of at most that many pulses does not allocate.
    pub fn new(sample_rate: u32, bpm: Bpm, pattern: &Pattern, start_frame: u64) -> Self {
        let reference = u64::from(pattern.reference_beats().max(1));
        let mut tl = Timeline {
            sample_rate,
            bpm,
            q: reference * 60 * u64::from(sample_rate),
            anchor: Anchor::measure_start(start_frame, 0),
            cursors: Vec::with_capacity(MAX_PULSES),
            starts: Vec::with_capacity(MAX_PULSES),
            scratch: Vec::with_capacity(MAX_PULSES),
            switch: None,
        };
        tl.sync_pattern(pattern);
        tl
    }

    /// Starts the timeline at `position` instead of measure 0: frame
    /// `start_frame` is that musical position. This is how a rebuilt audio
    /// stream continues the music of the stream it replaces, with `position`
    /// taken from the old timeline's [`position_at`](Self::position_at) at
    /// the first frame the old stream did not render.
    ///
    /// The position is converted to this timeline's measure length `Q` with
    /// `ticks' = floor(ticks · Q / position.ticks_per_measure)`, exactly as
    /// [`rebase_sample_rate`](Self::rebase_sample_rate) does, so a different
    /// sample rate is fine. Beats the old stream had not played yet (also one
    /// whose frame was exactly the seam) are played here; the ones it played
    /// are not repeated. That bound depends on the old tempo, so `bpm` must be
    /// the tempo the old stream was playing at that position, and `pattern`
    /// the pattern it was playing. Every pulse is audible from
    /// `start_frame` (a pulse still waiting for its first "one" on the old
    /// stream starts right away).
    ///
    /// A `position` with `ticks_per_measure == 0` (see
    /// [`MusicalPosition::start_of`]) has no phase to convert: it means "the
    /// start of `position.measure`, nothing played yet". Every pulse plays
    /// beat 0 at `start_frame` and the grid is then that of
    /// [`new`](Self::new) at `start_frame`, with the measure count starting
    /// at `position.measure`; `ticks` is ignored.
    pub fn resume(
        sample_rate: u32,
        bpm: Bpm,
        pattern: &Pattern,
        position: MusicalPosition,
        start_frame: u64,
    ) -> Self {
        let mut tl = Self::new(sample_rate, bpm, pattern, start_frame);
        let old_q = i128::from(position.ticks_per_measure);
        if old_q == 0 {
            // `MusicalPosition::start_of`: the "one" of that measure is due.
            tl.anchor = Anchor::measure_start(start_frame, position.measure);
            return tl;
        }
        let q = i128::from(tl.q);
        let carry = i128::from(position.ticks) / old_q;
        let ticks = i128::from(position.ticks) % old_q;
        // The old stream played every beat with target <= phase(f - 1),
        // i.e. ticks - bpm in its units.
        let (done_num, done_den) = reduce((ticks - i128::from(bpm.get())) * q, old_q);
        tl.anchor = Anchor {
            frame: start_frame,
            measure: position.measure + carry as u64,
            ticks: to_u64(ticks * q / old_q),
            done_num,
            done_den,
        };
        tl
    }

    /// Starts measure 0 of `pattern` at `start_frame` again, as
    /// [`new`](Self::new) would (same sample rate, current BPM), but without
    /// allocating: a pending switch is dropped and every pulse plays from the
    /// "one". `pattern` must not have more pulses than synced.
    pub fn restart(&mut self, pattern: &Pattern, start_frame: u64) {
        self.q = self.q_for(u64::from(pattern.reference_beats().max(1)));
        self.anchor = Anchor::measure_start(start_frame, 0);
        self.switch = None;
        self.starts.fill(0);
        self.invalidate();
    }

    /// Makes room for `pattern`'s pulses and invalidates every cursor.
    /// Allocates only past [`MAX_PULSES`] pulses (room for that many is
    /// reserved by [`new`](Self::new)), so up to that it may run on the
    /// real-time thread. Call it before
    /// [`apply_pattern`](Self::apply_pattern) with a pattern that has more
    /// pulses than any seen so far. Storage only grows (extra slots are
    /// ignored), so syncing the new pattern before an edit is always safe.
    /// It does not change the anchor or the measure length.
    pub fn sync_pattern(&mut self, pattern: &Pattern) {
        let len = pattern.pulses.len().max(self.cursors.len());
        self.cursors.clear();
        self.cursors.resize(len, Cursor::INVALID);
        self.starts.resize(len, 0);
        self.scratch.resize(len, 0);
    }

    /// Changes the tempo from `at_frame` on, keeping the musical position
    /// exactly: the anchor moves to `at_frame` with
    /// `ticks = anchor_ticks + (at_frame − anchor_frame)·bpm_old` reduced mod
    /// `Q` (carry into the measure). A beat the old tempo would have fired at
    /// `at_frame` still fires there. Same BPM is a no-op. `at_frame` is the
    /// first frame not yet rendered; frames before the anchor count as the
    /// anchor frame.
    pub fn set_bpm(&mut self, at_frame: u64, bpm: Bpm) {
        self.commit_switch_until(at_frame);
        if bpm == self.bpm {
            return;
        }
        self.reanchor(at_frame);
        self.bpm = bpm;
        self.invalidate();
    }

    /// Applies an edit from `old` (the pattern rendered so far) to `new`
    /// at `at_frame` (the first frame not yet rendered).
    ///
    /// If the reference cycle changes (pulse 0's beat count, or a different
    /// pulse 0 after a reorder or removal) the result is
    /// [`ApplyMode::AtNextOne`]: the caller keeps passing `old` to `events_in`
    /// for frames before the returned frame and `new` from it on; there the
    /// timeline re-anchors with `ticks = 0` and the new `Q`. Until then
    /// [`pending_switch`](Self::pending_switch) reports the frame, which
    /// follows later tempo changes. A later call while a switch is pending
    /// must pass the same `old`; it replaces the pending switch, or cancels it
    /// when the reference is back to `old`'s.
    ///
    /// Otherwise the result is [`ApplyMode::Now`] and `new` is rendered from
    /// `at_frame` on. Pulses not in `old` (by id) become audible from the
    /// next "one". A pulse whose beat count changed resumes at its first
    /// new-grid beat with frame `>= at_frame`; everything else (sound,
    /// intensities, enabled, removed pulses) is immediate.
    ///
    /// Never allocates: if `new` has more pulses than any pattern seen so
    /// far, call [`sync_pattern`](Self::sync_pattern) with it first (pulses
    /// past the synced count are ignored).
    pub fn apply_pattern(&mut self, at_frame: u64, old: &Pattern, new: &Pattern) -> ApplyMode {
        self.commit_switch_until(at_frame);
        let reference = |p: &Pattern| p.pulses.first().map(|x| (x.id, x.beats));
        if reference(old) != reference(new) {
            let measure = self.next_one(at_frame);
            self.switch = Some(Switch {
                measure,
                reference: u64::from(new.reference_beats().max(1)),
            });
            return ApplyMode::AtNextOne(self.measure_frame(measure));
        }
        self.switch = None;
        debug_assert!(
            new.pulses.len() <= self.scratch.len(),
            "new pattern has more pulses than synced; call sync_pattern first"
        );
        let next_one = self.next_one(at_frame);
        for (slot, pulse) in self.scratch.iter_mut().zip(&new.pulses) {
            *slot = match old.pulses.iter().position(|o| o.id == pulse.id) {
                Some(j) => self.starts.get(j).copied().unwrap_or(0),
                None => next_one,
            };
        }
        core::mem::swap(&mut self.starts, &mut self.scratch);
        self.invalidate();
        ApplyMode::Now
    }

    /// Frame of a pending reference-cycle switch, if any (see
    /// [`apply_pattern`](Self::apply_pattern)).
    pub fn pending_switch(&self) -> Option<u64> {
        self.switch.map(|s| self.measure_frame(s.measure))
    }

    /// The output now runs at `new_sr`, and frame `new_start_frame` of the new
    /// stream continues where frame `at_frame_old` of the old one stopped
    /// (the first old frame not rendered). The position is converted with
    /// `ticks' = floor(ticks · new_sr / old_sr)`; beats not yet emitted on the
    /// old stream are emitted on the new one, the others are not repeated.
    pub fn rebase_sample_rate(&mut self, at_frame_old: u64, new_sr: u32, new_start_frame: u64) {
        self.commit_switch_until(at_frame_old);
        self.reanchor(at_frame_old);
        let (old, new) = (i128::from(self.sample_rate), i128::from(new_sr));
        let a = self.anchor;
        let (num, den) = reduce(a.done_num * new, a.done_den * old);
        self.anchor = Anchor {
            frame: new_start_frame,
            measure: a.measure,
            ticks: to_u64(i128::from(a.ticks) * new / old),
            done_num: num,
            done_den: den,
        };
        self.q = self.q / u64::from(self.sample_rate) * u64::from(new_sr);
        self.sample_rate = new_sr;
        self.invalidate();
    }

    /// Appends every hit with `from <= frame < to`, ascending by frame, ties
    /// in pulse-index order. Disabled pulses and muted beats are skipped but
    /// still counted in beat numbering. Never allocates by itself: `out` must
    /// have room (e.g. a `Vec` with enough reserved capacity). Pulses beyond
    /// those seen by the last `sync_pattern` are ignored.
    ///
    /// While a switch is pending (see [`pending_switch`](Self::pending_switch))
    /// a range must not cross it: render `[from, switch)` with the old pattern
    /// and `[switch, to)` with the new one, splitting the caller's buffer at
    /// the switch frame. A crossing range is a bug: it `debug_assert`s, and in
    /// release it is cut at the switch (hits from the switch on are missing).
    pub fn events_in(
        &mut self,
        pattern: &Pattern,
        from: u64,
        to: u64,
        out: &mut impl Extend<BeatHit>,
    ) {
        let mut to = to;
        if let Some(switch) = self.pending_switch() {
            if from >= switch {
                self.commit_switch_until(from);
            } else {
                debug_assert!(
                    to <= switch,
                    "render range crosses the pattern switch; split it there"
                );
                to = to.min(switch);
            }
        }
        debug_assert!(
            pattern.pulses.len() <= self.cursors.len(),
            "pattern has more pulses than synced; call sync_pattern first"
        );
        let count = pattern.pulses.len().min(self.cursors.len());
        for i in 0..count {
            let n = pattern.pulses[i].beats.max(1);
            if !self.cursor_fits(i, n, from) {
                self.cursors[i] = self.seek(i, n, from);
            }
        }
        loop {
            let mut best: Option<usize> = None;
            for i in 0..count {
                let f = self.cursors[i].frame;
                if f < to && best.is_none_or(|b| f < self.cursors[b].frame) {
                    best = Some(i);
                }
            }
            let Some(i) = best else { break };
            let pulse = &pattern.pulses[i];
            let cursor = self.cursors[i];
            let n = cursor.beats;
            let beat = (cursor.idx % u64::from(n)) as u32;
            // A missing intensity (pattern out of sync with `beats`) is silent.
            let intensity = pulse
                .intensities
                .get(beat as usize)
                .copied()
                .unwrap_or(Intensity::Mute);
            if pulse.enabled && intensity != Intensity::Mute {
                out.extend(core::iter::once(BeatHit {
                    pulse: pulse.id,
                    beat,
                    frame: cursor.frame,
                    gain: intensity.gain(),
                    sound: pulse.sound,
                    accent: beat == 0,
                }));
            }
            let idx = cursor.idx + 1;
            self.cursors[i] = Cursor {
                beats: n,
                idx,
                frame: self.frame_of(n, idx),
                prev: Some(cursor.frame),
            };
        }
    }

    /// Musical position at `frame`, including a pending switch at or before
    /// it. Frames before the anchor report the anchor position.
    pub fn position_at(&self, frame: u64) -> MusicalPosition {
        let (anchor, q) = match (self.switch, self.pending_switch()) {
            (Some(s), Some(f)) if frame >= f => {
                (Anchor::measure_start(f, s.measure), self.q_for(s.reference))
            }
            _ => (self.anchor, self.q),
        };
        let elapsed = i128::from(frame.saturating_sub(anchor.frame));
        let phase = elapsed * i128::from(self.bpm.get()) + i128::from(anchor.ticks);
        let qi = i128::from(q);
        MusicalPosition {
            measure: anchor.measure + (phase / qi) as u64,
            ticks: (phase % qi) as u64,
            ticks_per_measure: q,
        }
    }

    fn q_for(&self, reference: u64) -> u64 {
        reference * 60 * u64::from(self.sample_rate)
    }

    fn invalidate(&mut self) {
        self.cursors.fill(Cursor::INVALID);
    }

    /// Phase in ticks from the start of the anchor measure at `frame`
    /// (`>= anchor.frame`).
    fn phase(&self, frame: u64) -> i128 {
        i128::from(frame - self.anchor.frame) * i128::from(self.bpm.get())
            + i128::from(self.anchor.ticks)
    }

    /// Moves the anchor to `at` (clamped to the anchor frame) with the exact
    /// phase there. The beats already emitted are those the current timeline
    /// fires before `at`: target `<= phase(at − 1)`.
    fn reanchor(&mut self, at: u64) {
        if at <= self.anchor.frame {
            return;
        }
        let phase = self.phase(at);
        let q = i128::from(self.q);
        let ticks = phase % q;
        self.anchor = Anchor {
            frame: at,
            measure: self.anchor.measure + (phase / q) as u64,
            ticks: ticks as u64,
            done_num: ticks - i128::from(self.bpm.get()),
            done_den: 1,
        };
    }

    /// Performs a pending switch if its frame is `<= frame`.
    fn commit_switch_until(&mut self, frame: u64) {
        if let (Some(s), Some(f)) = (self.switch, self.pending_switch())
            && f <= frame
        {
            self.anchor = Anchor::measure_start(f, s.measure);
            self.q = self.q_for(s.reference);
            self.switch = None;
            // Everything in the new pattern plays from this "one".
            self.starts.fill(0);
            self.invalidate();
        }
    }

    /// Absolute measure index of the first "one" whose frame is `>= frame`.
    fn next_one(&self, frame: u64) -> u64 {
        let first = self.pending_idx(1);
        self.anchor.measure + self.seek_idx(1, frame, first)
    }

    /// Frame at which measure `measure` (`>= anchor.measure`) starts.
    fn measure_frame(&self, measure: u64) -> u64 {
        self.frame_of(1, measure.saturating_sub(self.anchor.measure))
    }

    /// True if cursor `i` holds the first beat at or after `from`.
    fn cursor_fits(&self, i: usize, beats: u32, from: u64) -> bool {
        let c = self.cursors[i];
        c.beats == beats && c.frame >= from && c.prev.is_none_or(|p| p < from)
    }

    /// Cursor at the first beat of pulse `i` (`n` beats) whose frame is
    /// `>= from`.
    fn seek(&self, i: usize, n: u32, from: u64) -> Cursor {
        let first = self.first_idx(i, n);
        let idx = self.seek_idx(n, from, first);
        Cursor {
            beats: n,
            idx,
            frame: self.frame_of(n, idx),
            prev: (idx > first).then(|| self.frame_of(n, idx - 1)),
        }
    }

    /// Smallest beat index `>= first` of an `n`-beat grid with frame `>= from`.
    fn seek_idx(&self, n: u32, from: u64, first: u64) -> u64 {
        if from <= self.anchor.frame {
            return first;
        }
        // Smallest idx with ceil(num / (n·bpm)) >= d, i.e.
        // idx·Q > (d - 1)·n·bpm + anchor_ticks·n. Pending beats from before the
        // anchor sit at the anchor frame, before `from`, so they never match.
        let d = i128::from(from - self.anchor.frame);
        let nn = i128::from(n);
        let x = (d - 1) * nn * i128::from(self.bpm.get()) + i128::from(self.anchor.ticks) * nn;
        to_u64(x / i128::from(self.q) + 1).max(first)
    }

    /// First beat index (from the anchor measure start) pulse `i` may emit:
    /// the first beat not emitted yet, or later if the pulse waits for a "one".
    fn first_idx(&self, i: usize, n: u32) -> u64 {
        let pending = self.pending_idx(n);
        match self.starts.get(i) {
            Some(&start) if start > self.anchor.measure => {
                pending.max((start - self.anchor.measure) * u64::from(n))
            }
            _ => pending,
        }
    }

    /// First beat index with target `idx·Q/n > done`:
    /// `floor(done · n / Q) + 1`, at least 0.
    fn pending_idx(&self, n: u32) -> u64 {
        let a = self.anchor;
        let below = (a.done_num * i128::from(n)).div_euclid(i128::from(self.q) * a.done_den);
        to_u64(below + 1)
    }

    /// Frame of beat `idx` (counted from the anchor measure start) of an
    /// `n`-beat pulse: `anchor_frame + ceil((idx·Q − anchor_ticks·n) / (n·bpm))`,
    /// or the anchor frame for a pending beat whose target is not after the
    /// anchor phase.
    fn frame_of(&self, n: u32, idx: u64) -> u64 {
        let nn = i128::from(n);
        let num = i128::from(idx) * i128::from(self.q) - i128::from(self.anchor.ticks) * nn;
        let offset = div_ceil(num, nn * i128::from(self.bpm.get())).max(0);
        to_u64(i128::from(self.anchor.frame) + offset)
    }
}

/// Ceiling division for any `a`, `b > 0`.
fn div_ceil(a: i128, b: i128) -> i128 {
    -((-a).div_euclid(b))
}

/// `num / den` in lowest terms (`den > 0`).
fn reduce(num: i128, den: i128) -> (i128, i128) {
    let (mut x, mut y) = (num.unsigned_abs(), den.unsigned_abs());
    while y != 0 {
        (x, y) = (y, x % y);
    }
    let g = x.max(1) as i128;
    (num / g, den / g)
}

/// Saturating conversion; frames past `u64::MAX` are never reached.
fn to_u64(v: i128) -> u64 {
    u64::try_from(v.max(0)).unwrap_or(u64::MAX)
}
