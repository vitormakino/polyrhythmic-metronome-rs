//! Exact, drift-free timeline.
//!
//! Musical phase is counted in integer ticks. With sample rate `sr` and
//! reference beats `R` (the first pulse's beat count), one measure is
//! `Q = R · 60 · sr` ticks and phase advances `bpm` ticks per frame. Every
//! hit frame is computed directly from an anchor
//! `(anchor_frame, anchor_measure, anchor_ticks)` with `i128` integer math,
//! so no error can accumulate however long the session runs. There are no
//! floats here except the hit gain.

use crate::model::{Bpm, Intensity, Pattern, PulseId, SoundType};

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

/// Where the tempo comes from. v1 uses the internal BPM; v2 may plug in Link.
pub trait TempoSource {
    fn bpm(&self) -> Bpm;
}

impl TempoSource for Bpm {
    fn bpm(&self) -> Bpm {
        *self
    }
}

/// The point the whole timeline is computed from: at `frame`, the music is
/// `ticks` (in `0..Q`) into measure `measure`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor {
    frame: u64,
    measure: u64,
    ticks: u64,
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
/// pattern, sample rate) are meant to re-anchor at the edit frame.
pub struct Timeline {
    bpm: Bpm,
    /// Ticks per measure, `R · 60 · sr`.
    q: u64,
    anchor: Anchor,
    cursors: Vec<Cursor>,
}

impl Timeline {
    /// Starts measure 0 exactly at `start_frame`.
    pub fn new(sample_rate: u32, bpm: Bpm, pattern: &Pattern, start_frame: u64) -> Self {
        let reference = u64::from(pattern.reference_beats().max(1));
        let mut tl = Timeline {
            bpm,
            q: reference * 60 * u64::from(sample_rate),
            anchor: Anchor {
                frame: start_frame,
                measure: 0,
                ticks: 0,
            },
            cursors: Vec::new(),
        };
        tl.sync_pattern(pattern);
        tl
    }

    /// Sizes the cursor storage for `pattern` and invalidates every cursor.
    /// Allocates, so call it outside the real-time thread. It does not change
    /// the anchor or the measure length.
    pub fn sync_pattern(&mut self, pattern: &Pattern) {
        self.cursors.clear();
        self.cursors.resize(pattern.pulses.len(), Cursor::INVALID);
    }

    /// Appends every hit with `from <= frame < to`, ascending by frame, ties
    /// in pulse-index order. Disabled pulses and muted beats are skipped but
    /// still counted in beat numbering. Never allocates by itself: `out` must
    /// have room (e.g. a `Vec` with enough reserved capacity). Pulses beyond
    /// those seen by the last `sync_pattern` are ignored.
    pub fn events_in(
        &mut self,
        pattern: &Pattern,
        from: u64,
        to: u64,
        out: &mut impl Extend<BeatHit>,
    ) {
        debug_assert!(
            pattern.pulses.len() <= self.cursors.len(),
            "pattern has more pulses than synced; call sync_pattern first"
        );
        let count = pattern.pulses.len().min(self.cursors.len());
        for i in 0..count {
            let n = pattern.pulses[i].beats.max(1);
            if !self.cursor_fits(i, n, from) {
                self.cursors[i] = self.seek(n, from);
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

    /// Musical position at `frame`. Frames before the anchor report the
    /// anchor position.
    pub fn position_at(&self, frame: u64) -> MusicalPosition {
        let elapsed = i128::from(frame.saturating_sub(self.anchor.frame));
        let phase = elapsed * i128::from(self.bpm.get()) + i128::from(self.anchor.ticks);
        let q = i128::from(self.q);
        MusicalPosition {
            measure: self.anchor.measure + (phase / q) as u64,
            ticks: (phase % q) as u64,
            ticks_per_measure: self.q,
        }
    }

    /// True if cursor `i` holds the first beat at or after `from`.
    fn cursor_fits(&self, i: usize, beats: u32, from: u64) -> bool {
        let c = self.cursors[i];
        c.beats == beats && c.frame >= from && c.prev.is_none_or(|p| p < from)
    }

    /// Cursor at the first beat of an `n`-beat pulse whose frame is `>= from`.
    fn seek(&self, n: u32, from: u64) -> Cursor {
        let first = self.first_idx(n);
        let idx = if from <= self.anchor.frame {
            first
        } else {
            // Smallest idx with ceil(num / (n·bpm)) >= d, i.e.
            // idx·Q > (d - 1)·n·bpm + anchor_ticks·n.
            let d = i128::from(from - self.anchor.frame);
            let nn = i128::from(n);
            let x = (d - 1) * nn * i128::from(self.bpm.get()) + i128::from(self.anchor.ticks) * nn;
            to_u64(x / i128::from(self.q) + 1).max(first)
        };
        Cursor {
            beats: n,
            idx,
            frame: self.frame_of(n, idx),
            prev: (idx > first).then(|| self.frame_of(n, idx - 1)),
        }
    }

    /// First beat index (from the anchor measure start) not before the anchor:
    /// `ceil(anchor_ticks · n / Q)`.
    fn first_idx(&self, n: u32) -> u64 {
        to_u64(div_ceil(
            i128::from(self.anchor.ticks) * i128::from(n),
            i128::from(self.q),
        ))
    }

    /// Frame of beat `idx` (counted from the anchor measure start) of an
    /// `n`-beat pulse: `anchor_frame + ceil((idx·Q − anchor_ticks·n) / (n·bpm))`.
    fn frame_of(&self, n: u32, idx: u64) -> u64 {
        let nn = i128::from(n);
        let num = i128::from(idx) * i128::from(self.q) - i128::from(self.anchor.ticks) * nn;
        debug_assert!(num >= 0, "beat before the anchor");
        let offset = div_ceil(num, nn * i128::from(self.bpm.get()));
        to_u64(i128::from(self.anchor.frame) + offset)
    }
}

/// Ceiling division for `a >= 0`, `b > 0`.
fn div_ceil(a: i128, b: i128) -> i128 {
    (a + b - 1) / b
}

/// Saturating conversion; frames past `u64::MAX` are never reached.
fn to_u64(v: i128) -> u64 {
    u64::try_from(v.max(0)).unwrap_or(u64::MAX)
}
