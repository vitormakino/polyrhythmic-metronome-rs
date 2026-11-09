//! Live edits (tempo, pattern, sample rate) through the public API only.
//!
//! The owner's requirement: no beat is ever lost or doubled and the grid never
//! drifts, whatever is edited while playing and at whatever frame.

use pm_core::{ApplyMode, BeatHit, Bpm, Pattern, PulseId, SoundType, Timeline};
use proptest::prelude::*;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// Builds a pattern whose pulses have exactly the given beat counts.
fn pattern_of(beats: &[u32]) -> Pattern {
    assert!(!beats.is_empty());
    let mut p = Pattern::default();
    while p.pulses.len() < beats.len() {
        p.add_pulse();
    }
    while p.pulses.len() > beats.len() {
        let id = p.pulses.last().unwrap().id;
        p.remove(id);
    }
    let ids: Vec<_> = p.pulses.iter().map(|x| x.id).collect();
    for (id, &n) in ids.iter().zip(beats) {
        p.set_beats(*id, n);
    }
    p
}

/// Renders `[from, to)` in chunks of `chunk` frames, appending to `out`.
fn render_into(
    tl: &mut Timeline,
    p: &Pattern,
    from: u64,
    to: u64,
    chunk: u64,
    out: &mut Vec<BeatHit>,
) {
    let mut t = from;
    while t < to {
        let end = (t + chunk).min(to);
        tl.events_in(p, t, end, out);
        t = end;
    }
}

fn render(tl: &mut Timeline, p: &Pattern, from: u64, to: u64) -> Vec<BeatHit> {
    let mut out = Vec::new();
    render_into(tl, p, from, to, 512, &mut out);
    out
}

fn frames_of(hits: &[BeatHit], id: PulseId) -> Vec<u64> {
    hits.iter()
        .filter(|h| h.pulse == id)
        .map(|h| h.frame)
        .collect()
}

fn beats_of(hits: &[BeatHit], id: PulseId) -> Vec<(u64, u32)> {
    hits.iter()
        .filter(|h| h.pulse == id)
        .map(|h| (h.frame, h.beat))
        .collect()
}

/// For each pulse, successive hits have beat `(prev + 1) % n` and strictly
/// increasing frames, starting from beat 0.
fn assert_consecutive(hits: &[BeatHit], p: &Pattern) {
    assert_beats_consecutive(hits, p);
    assert_frames_increase(hits, p);
}

fn assert_frames_increase(hits: &[BeatHit], p: &Pattern) {
    for pulse in &p.pulses {
        let mine: Vec<_> = hits.iter().filter(|h| h.pulse == pulse.id).collect();
        for w in mine.windows(2) {
            assert!(w[1].frame > w[0].frame, "{:?} then {:?}", w[0], w[1]);
        }
    }
}

/// For each pulse, successive hits have beat `(prev + 1) % n`, starting from
/// beat 0.
fn assert_beats_consecutive(hits: &[BeatHit], p: &Pattern) {
    for pulse in &p.pulses {
        let mine: Vec<_> = hits.iter().filter(|h| h.pulse == pulse.id).collect();
        assert_eq!(
            mine.first().map(|h| h.beat),
            Some(0),
            "pulse {:?} starts at 0",
            pulse.id
        );
        for w in mine.windows(2) {
            assert_eq!(
                w[1].beat,
                (w[0].beat + 1) % pulse.beats,
                "pulse {:?}: {:?} then {:?}",
                pulse.id,
                w[0],
                w[1]
            );
        }
    }
}

// ---------------------------------------------------------------- tempo ----

#[test]
fn bpm_change_preserves_measure_fraction() {
    let p = Pattern::default();
    let (a, b) = (p.pulses[0].id, p.pulses[1].id);
    let mut tl = Timeline::new(48_000, Bpm::new(120), &p, 0);
    let mut hits = render(&mut tl, &p, 0, 48_000);
    tl.set_bpm(48_000, Bpm::new(60));
    hits.extend(render(&mut tl, &p, 48_000, 144_001));
    assert_eq!(
        beats_of(&hits, a),
        vec![(0, 0), (24_000, 1), (48_000, 2), (96_000, 3), (144_000, 0)]
    );
    // B's beat 2 sits at 2/3 of the measure: 1/6 measure = 16000 frames at
    // 120 BPM, now 32000 at 60 BPM.
    assert_eq!(frames_of(&hits, b), vec![0, 32_000, 80_000, 144_000]);
    let pos = tl.position_at(48_000);
    assert_eq!((pos.measure, pos.ticks), (0, pos.ticks_per_measure / 2));
}

#[test]
fn same_bpm_is_a_noop() {
    let p = pattern_of(&[7, 11]);
    let mut plain = Timeline::new(44_100, Bpm::new(137), &p, 0);
    let want = render(&mut plain, &p, 0, 200_000);
    let mut tl = Timeline::new(44_100, Bpm::new(137), &p, 0);
    let mut got = render(&mut tl, &p, 0, 77_777);
    tl.set_bpm(77_777, Bpm::new(137));
    got.extend(render(&mut tl, &p, 77_777, 200_000));
    assert_eq!(got, want);
}

/// Regression for the Task 3 review: at 44.1 kHz / 137 BPM, A's beat 1
/// targets tick 2_646_000, which the phase crosses strictly inside frame
/// 19314 (phase(19313) = 2_645_881 < target < phase(19314) = 2_646_018).
/// Re-anchoring at that very frame must still emit it, exactly once, there.
#[test]
fn reanchor_on_a_beat_frame_keeps_that_beat() {
    let p = Pattern::default();
    let a = p.pulses[0].id;
    let mut tl = Timeline::new(44_100, Bpm::new(137), &p, 0);
    let mut hits = render(&mut tl, &p, 0, 19_314);
    assert_eq!(beats_of(&hits, a), vec![(0, 0)]);
    tl.set_bpm(19_314, Bpm::new(200));
    hits.extend(render(&mut tl, &p, 19_314, 19_315));
    assert_eq!(beats_of(&hits, a), vec![(0, 0), (19_314, 1)]);
    // And the next beat follows the new tempo from the exact phase.
    hits.extend(render(&mut tl, &p, 19_315, 60_000));
    let q = 4 * 60 * 44_100u64;
    let phase = 19_314 * 137u64;
    let beat2 = 19_314 + (q / 2 - phase).div_ceil(200);
    assert_eq!(beats_of(&hits, a)[2], (beat2, 2));
}

/// Beat targets are rational (`idx·Q/n`). Here 3:11 at 96 kHz, after a tempo
/// change: pulse B's beat 8 of measure 1 targets 12_567_272 8/11 ticks into
/// the measure, just above phase(325273) = 12_567_272. Re-anchoring at 325274
/// must still emit it there (an integer `phase + 1` bound would drop it).
#[test]
fn reanchor_keeps_a_beat_with_a_fractional_target() {
    let p = pattern_of(&[3, 11]);
    let b = p.pulses[1].id;
    let mut tl = Timeline::new(96_000, Bpm::new(170), &p, 0);
    let mut hits = render(&mut tl, &p, 0, 107_759);
    tl.set_bpm(107_759, Bpm::new(53));
    hits.extend(render(&mut tl, &p, 107_759, 325_274));
    tl.set_bpm(325_274, Bpm::new(200));
    hits.extend(render(&mut tl, &p, 325_274, 325_275));
    let last = *hits.iter().rfind(|h| h.pulse == b).unwrap();
    assert_eq!((last.frame, last.beat), (325_274, 8));
    assert_beats_consecutive(&hits, &p);
}

/// The same frame, but via a sample-rate rebase.
#[test]
fn rebase_on_a_beat_frame_keeps_that_beat() {
    let p = Pattern::default();
    let a = p.pulses[0].id;
    let mut tl = Timeline::new(44_100, Bpm::new(137), &p, 0);
    let mut hits = render(&mut tl, &p, 0, 19_314);
    tl.rebase_sample_rate(19_314, 48_000, 500);
    hits.extend(render(&mut tl, &p, 500, 60_000));
    let a_beats: Vec<u32> = hits
        .iter()
        .filter(|h| h.pulse == a)
        .map(|h| h.beat)
        .collect();
    assert_eq!(&a_beats[..3], &[0, 1, 2]);
    assert_eq!(beats_of(&hits, a)[1], (500, 1));
}

/// Independent reference for a run with tempo changes: the phase is a
/// continuous piecewise-linear integer function of the frame (`bpm` ticks per
/// frame in each segment) and a beat whose target is `T` ticks fires at the
/// first frame whose phase is `>= T`.
struct PhaseRef {
    /// (start frame, bpm, phase at start frame), ascending.
    segments: Vec<(u64, u64, u128)>,
}

impl PhaseRef {
    fn new(bpm: u16) -> Self {
        PhaseRef {
            segments: vec![(0, u64::from(bpm), 0)],
        }
    }

    fn phase(&self, frame: u64) -> u128 {
        let &(s, b, ph) = self
            .segments
            .iter()
            .rev()
            .find(|seg| seg.0 <= frame)
            .unwrap();
        ph + u128::from(frame - s) * u128::from(b)
    }

    fn change(&mut self, frame: u64, bpm: u16) {
        let ph = self.phase(frame);
        self.segments.push((frame, u64::from(bpm), ph));
    }

    /// Frame of the beat whose target is `tn / n` ticks.
    fn fire(&self, tn: u128, n: u128) -> u64 {
        let i = self
            .segments
            .iter()
            .enumerate()
            .position(|(i, _)| match self.segments.get(i + 1) {
                Some(next) => next.2 * n >= tn,
                None => true,
            })
            .unwrap();
        let (s, b, ph) = self.segments[i];
        let num = tn.saturating_sub(ph * n);
        s + num.div_ceil(u128::from(b) * n) as u64
    }

    /// Every hit `(frame, pulse index, beat)` with frame < `to`, sorted.
    fn hits(&self, q: u64, beats: &[u32], to: u64) -> Vec<(u64, usize, u32)> {
        let mut v = Vec::new();
        for (i, &n) in beats.iter().enumerate() {
            let n = u128::from(n);
            for idx in 0u128.. {
                let f = self.fire(idx * u128::from(q), n);
                if f >= to {
                    break;
                }
                v.push((f, i, (idx % n) as u32));
            }
        }
        v.sort();
        v
    }
}

fn indexed(hits: &[BeatHit], p: &Pattern) -> Vec<(u64, usize, u32)> {
    hits.iter()
        .map(|h| {
            let i = p.pulses.iter().position(|x| x.id == h.pulse).unwrap();
            (h.frame, i, h.beat)
        })
        .collect()
}

/// A tempo change: where it lands relative to the next beat of some pulse,
/// and the new tempo. `Offset(d)` lands `d` frames after the previous change;
/// `OnBeat(pulse, delta)` lands on the next beat frame of `pulse` (strictly
/// after the previous change), shifted by `delta` in `-1..=1`.
#[derive(Clone, Debug)]
enum At {
    Offset(u64),
    OnBeat(usize, i8),
}

fn tempo_case() -> impl Strategy<Value = (u32, u16, Vec<u32>, Vec<(At, u16)>)> {
    let at = prop_oneof![
        (1u64..=30_000).prop_map(At::Offset),
        (0usize..4, -1i8..=1).prop_map(|(p, d)| At::OnBeat(p, d)),
    ];
    (
        prop::sample::select(vec![44_100u32, 48_000, 96_000]),
        40u16..=300,
        prop::collection::vec(1u32..=16, 1..=4),
        prop::collection::vec((at, 40u16..=300), 1..=40),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Tempo changes at arbitrary frames, including frames that coincide
    /// with a beat (and the frames right around it), reproduce exactly the
    /// piecewise-phase reference: same frames, nothing lost, nothing doubled.
    #[test]
    fn bpm_changes_match_piecewise_phase_reference(
        (sr, bpm0, beats, changes) in tempo_case()
    ) {
        let p = pattern_of(&beats);
        let q = u64::from(beats[0]) * 60 * u64::from(sr);
        let mut reference = PhaseRef::new(bpm0);
        let mut tl = Timeline::new(sr, Bpm::new(i32::from(bpm0)), &p, 0);
        let mut hits = Vec::new();
        let mut t = 0u64;
        for (at, bpm) in &changes {
            let f = match *at {
                At::Offset(d) => t + d,
                At::OnBeat(pulse, delta) => {
                    let n = u128::from(beats[pulse % beats.len()]);
                    let mut idx = 0u128;
                    while reference.fire(idx * u128::from(q), n) <= t {
                        idx += 1;
                    }
                    let f = reference.fire(idx * u128::from(q), n);
                    f.saturating_add_signed(i64::from(delta)).max(t)
                }
            };
            render_into(&mut tl, &p, t, f, 4096, &mut hits);
            tl.set_bpm(f, Bpm::new(i32::from(*bpm)));
            reference.change(f, *bpm);
            t = f;
        }
        let end = t + 3 * q / 40;
        render_into(&mut tl, &p, t, end, 4096, &mut hits);
        prop_assert_eq!(indexed(&hits, &p), reference.hits(q, &beats, end));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]

    #[test]
    fn bpm_spam_keeps_grid_exact(
        changes in prop::collection::vec((1u64..=3_000, 40u16..=300), 10_000)
    ) {
        let p = Pattern::default();
        let a = p.pulses[0].id;
        let mut tl = Timeline::new(48_000, Bpm::new(120), &p, 0);
        let mut hits = Vec::new();
        let mut t = 0;
        for &(d, bpm) in &changes {
            render_into(&mut tl, &p, t, t + d, 512, &mut hits);
            t += d;
            tl.set_bpm(t, Bpm::new(i32::from(bpm)));
        }
        let back = t + 1;
        render_into(&mut tl, &p, t, back, 512, &mut hits);
        tl.set_bpm(back, Bpm::new(120));
        let end = back + 10 * 96_000;
        render_into(&mut tl, &p, back, end, 512, &mut hits);
        assert_consecutive(&hits, &p);
        let ones: Vec<u64> = hits
            .iter()
            .filter(|h| h.pulse == a && h.beat == 0 && h.frame >= back)
            .map(|h| h.frame)
            .collect();
        prop_assert!(ones.len() >= 9);
        for w in ones.windows(2) {
            prop_assert_eq!(w[1] - w[0], 96_000);
        }
    }
}

// -------------------------------------------------------------- pattern ----

#[test]
fn reference_change_waits_for_next_one() {
    let old = Pattern::default();
    let a = old.pulses[0].id;
    let b = old.pulses[1].id;
    let mut new = old.clone();
    new.set_beats(a, 5);
    let mut tl = Timeline::new(48_000, Bpm::new(120), &old, 0);
    let mut hits = render(&mut tl, &old, 0, 30_000);
    let mode = tl.apply_pattern(30_000, &old, &new);
    assert_eq!(mode, ApplyMode::AtNextOne(96_000));
    assert_eq!(tl.pending_switch(), Some(96_000));
    hits.extend(render(&mut tl, &old, 30_000, 96_000));
    assert_eq!(frames_of(&hits, a), vec![0, 24_000, 48_000, 72_000]);
    assert_eq!(frames_of(&hits, b), vec![0, 32_000, 64_000]);
    // From the switch on the caller renders the new pattern.
    let mut after = render(&mut tl, &new, 96_000, 96_000 + 120_000 + 1);
    assert_eq!(tl.pending_switch(), None);
    assert_eq!(
        beats_of(&after, a),
        vec![
            (96_000, 0),
            (120_000, 1),
            (144_000, 2),
            (168_000, 3),
            (192_000, 4),
            (216_000, 0)
        ]
    );
    assert_eq!(
        frames_of(&after, b),
        vec![96_000, 136_000, 176_000, 216_000]
    );
    hits.append(&mut after);
    assert_eq!(tl.position_at(216_000).ticks_per_measure, 5 * 60 * 48_000);
}

#[test]
fn removing_or_reordering_pulse_zero_waits_for_next_one() {
    let old = pattern_of(&[4, 4]);
    let mut moved = old.clone();
    moved.move_pulse(1, 0);
    let mut removed = old.clone();
    removed.remove(old.pulses[0].id);
    for new in [moved, removed] {
        let mut tl = Timeline::new(48_000, Bpm::new(120), &old, 0);
        render(&mut tl, &old, 0, 10_000);
        assert_eq!(
            tl.apply_pattern(10_000, &old, &new),
            ApplyMode::AtNextOne(96_000)
        );
    }
}

#[test]
fn tempo_change_while_switch_pending_moves_the_switch() {
    let old = Pattern::default();
    let a = old.pulses[0].id;
    let mut new = old.clone();
    new.set_beats(a, 5);
    let mut tl = Timeline::new(48_000, Bpm::new(120), &old, 0);
    let mut hits = render(&mut tl, &old, 0, 30_000);
    assert_eq!(
        tl.apply_pattern(30_000, &old, &new),
        ApplyMode::AtNextOne(96_000)
    );
    hits.extend(render(&mut tl, &old, 30_000, 48_000));
    tl.set_bpm(48_000, Bpm::new(60));
    // Half a measure left at half the tempo.
    assert_eq!(tl.pending_switch(), Some(144_000));
    hits.extend(render(&mut tl, &old, 48_000, 144_000));
    hits.extend(render(&mut tl, &new, 144_000, 144_000 + 240_000));
    assert_eq!(
        frames_of(&hits, a),
        vec![
            0, 24_000, 48_000, 96_000, 144_000, 192_000, 240_000, 288_000, 336_000
        ]
    );
}

#[test]
fn reverting_the_reference_cancels_the_switch() {
    let old = Pattern::default();
    let a = old.pulses[0].id;
    let mut five = old.clone();
    five.set_beats(a, 5);
    let mut tl = Timeline::new(48_000, Bpm::new(120), &old, 0);
    render(&mut tl, &old, 0, 30_000);
    assert_eq!(
        tl.apply_pattern(30_000, &old, &five),
        ApplyMode::AtNextOne(96_000)
    );
    assert_eq!(tl.apply_pattern(40_000, &old, &old), ApplyMode::Now);
    assert_eq!(tl.pending_switch(), None);
}

#[test]
fn added_pulse_starts_at_next_one() {
    let old = Pattern::default();
    let mut new = old.clone();
    let c = new.add_pulse();
    let mut tl = Timeline::new(48_000, Bpm::new(120), &old, 0);
    let mut hits = render(&mut tl, &old, 0, 10_000);
    tl.sync_pattern(&new);
    assert_eq!(tl.apply_pattern(10_000, &old, &new), ApplyMode::Now);
    hits.extend(render(&mut tl, &new, 10_000, 96_000 * 2));
    assert_eq!(frames_of(&hits, c)[0], 96_000);
    assert_eq!(beats_of(&hits, c)[0], (96_000, 0));
    // The existing pulses are untouched.
    assert_eq!(
        frames_of(&hits, old.pulses[0].id)[..5],
        [0, 24_000, 48_000, 72_000, 96_000]
    );
    assert_consecutive(&hits, &new);
}

#[test]
fn added_pulse_survives_a_reorder_before_its_one() {
    let old = Pattern::default();
    let mut added = old.clone();
    let c = added.add_pulse();
    let mut reordered = added.clone();
    reordered.move_pulse(2, 1);
    let mut tl = Timeline::new(48_000, Bpm::new(120), &old, 0);
    let mut hits = render(&mut tl, &old, 0, 10_000);
    tl.sync_pattern(&added);
    assert_eq!(tl.apply_pattern(10_000, &old, &added), ApplyMode::Now);
    hits.extend(render(&mut tl, &added, 10_000, 20_000));
    assert_eq!(tl.apply_pattern(20_000, &added, &reordered), ApplyMode::Now);
    hits.extend(render(&mut tl, &reordered, 20_000, 96_000 * 2));
    assert_eq!(frames_of(&hits, c)[0], 96_000);
    // B moved to index 2 and keeps playing right away.
    assert_eq!(
        frames_of(&hits, old.pulses[1].id)[..4],
        [0, 32_000, 64_000, 96_000]
    );
}

#[test]
fn sound_change_is_immediate() {
    let old = Pattern::default();
    let b = old.pulses[1].id;
    let mut new = old.clone();
    new.set_sound(b, SoundType::Bell);
    let mut tl = Timeline::new(48_000, Bpm::new(120), &old, 0);
    let mut hits = render(&mut tl, &old, 0, 33_000);
    assert_eq!(tl.apply_pattern(33_000, &old, &new), ApplyMode::Now);
    hits.extend(render(&mut tl, &new, 33_000, 96_000));
    let at = |f| *hits.iter().find(|h| h.pulse == b && h.frame == f).unwrap();
    assert_eq!(at(32_000).sound, SoundType::Wood);
    assert_eq!(at(64_000).sound, SoundType::Bell);
}

#[test]
fn non_reference_beats_change_resumes_on_new_grid() {
    let old = Pattern::default();
    let b = old.pulses[1].id;
    let mut new = old.clone();
    new.set_beats(b, 5);
    let mut tl = Timeline::new(48_000, Bpm::new(120), &old, 0);
    let mut hits = render(&mut tl, &old, 0, 40_000);
    assert_eq!(tl.apply_pattern(40_000, &old, &new), ApplyMode::Now);
    hits.extend(render(&mut tl, &new, 40_000, 96_001));
    // 5-beat grid: 0, 19200, 38400, 57600, 76800; first >= 40000 is 57600.
    assert_eq!(
        beats_of(&hits, b),
        vec![(0, 0), (32_000, 1), (57_600, 3), (76_800, 4), (96_000, 0)]
    );
}

#[test]
fn non_reference_beats_change_on_a_beat_frame_keeps_it() {
    let old = Pattern::default();
    let b = old.pulses[1].id;
    let mut new = old.clone();
    new.set_beats(b, 6);
    let mut tl = Timeline::new(48_000, Bpm::new(120), &old, 0);
    let mut hits = render(&mut tl, &old, 0, 48_000);
    assert_eq!(tl.apply_pattern(48_000, &old, &new), ApplyMode::Now);
    hits.extend(render(&mut tl, &new, 48_000, 64_001));
    assert_eq!(
        beats_of(&hits, b),
        vec![(0, 0), (32_000, 1), (48_000, 3), (64_000, 4)]
    );
}

#[test]
fn removing_a_non_reference_pulse_is_immediate() {
    let old = pattern_of(&[4, 3, 5]);
    let (a, c) = (old.pulses[0].id, old.pulses[2].id);
    let mut new = old.clone();
    new.remove(old.pulses[1].id);
    let mut tl = Timeline::new(48_000, Bpm::new(120), &old, 0);
    let mut hits = render(&mut tl, &old, 0, 30_000);
    assert_eq!(tl.apply_pattern(30_000, &old, &new), ApplyMode::Now);
    hits.extend(render(&mut tl, &new, 30_000, 96_001));
    assert_eq!(frames_of(&hits, a), vec![0, 24_000, 48_000, 72_000, 96_000]);
    assert_eq!(
        frames_of(&hits, c),
        vec![0, 19_200, 38_400, 57_600, 76_800, 96_000]
    );
}

// ---------------------------------------------------------- sample rate ----

#[test]
fn sample_rate_rebase_keeps_grid() {
    let p = Pattern::default();
    let a = p.pulses[0].id;
    let mut tl = Timeline::new(48_000, Bpm::new(120), &p, 0);
    let mut hits = render(&mut tl, &p, 0, 60_000);
    tl.rebase_sample_rate(60_000, 16_000, 0);
    hits.extend(render(&mut tl, &p, 0, 40_000));
    let a_hits: Vec<_> = hits.iter().filter(|h| h.pulse == a).collect();
    assert_eq!(a_hits[2].frame, 48_000);
    let beat3 = a_hits[3];
    assert_eq!(beat3.beat, 3);
    assert!(beat3.frame.abs_diff(4_000) <= 1, "{beat3:?}");
    // Next "one": 36000 frames at 48k after 60000 = 12000 at 16k.
    let one = a_hits[4];
    assert_eq!(one.beat, 0);
    assert!(one.frame.abs_diff(12_000) <= 1, "{one:?}");
    assert_eq!(tl.position_at(one.frame).ticks_per_measure, 4 * 60 * 16_000);
}

fn rebase_case() -> impl Strategy<Value = (u32, u32, u16, u16, Vec<u32>, u64, Option<i8>, u64)> {
    let rates = vec![
        8_000u32, 16_000, 22_050, 44_100, 48_000, 88_200, 96_000, 192_000,
    ];
    (
        prop::sample::select(rates.clone()),
        prop::sample::select(rates),
        40u16..=300,
        40u16..=300,
        prop::collection::vec(1u32..=12, 1..=4),
        1u64..=400_000,
        prop::option::of(-1i8..=1),
        0u64..=1_000_000,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Tempo change (so the anchor has non-zero ticks), then a rebase at an
    /// arbitrary frame or on/around a beat frame: every pulse's beats stay
    /// consecutive and the total count is exactly the number of beat targets
    /// the phase has reached, so nothing was lost or doubled.
    #[test]
    fn rebase_never_loses_or_doubles_a_beat(
        (old_sr, new_sr, bpm0, bpm1, beats, at, snap, new_start) in rebase_case()
    ) {
        let p = pattern_of(&beats);
        let mut tl = Timeline::new(old_sr, Bpm::new(i32::from(bpm0)), &p, 0);
        let first = at / 3 + 1;
        let mut hits = Vec::new();
        render_into(&mut tl, &p, 0, first, 4096, &mut hits);
        tl.set_bpm(first, Bpm::new(i32::from(bpm1)));
        // Optionally snap the rebase frame onto the next hit (shifted by -1..=1), found
        // on an identical timeline.
        let mut probe = Vec::new();
        let mut twin = Timeline::new(old_sr, Bpm::new(i32::from(bpm0)), &p, 0);
        twin.set_bpm(first, Bpm::new(i32::from(bpm1)));
        let from = at.max(first);
        render_into(&mut twin, &p, from, from + 400_000, 4096, &mut probe);
        let at_old = match (snap, probe.first()) {
            (Some(delta), Some(hit)) => hit.frame.saturating_add_signed(i64::from(delta)).max(first),
            _ => from,
        };
        render_into(&mut tl, &p, first, at_old, 4096, &mut hits);
        tl.rebase_sample_rate(at_old, new_sr, new_start);
        let end = new_start + 3 * u64::from(beats[0]) * 60 * u64::from(new_sr) / 40;
        let mut after = Vec::new();
        render_into(&mut tl, &p, new_start, end, 4096, &mut after);
        // Frame numbering restarts on the new stream.
        assert_frames_increase(&hits, &p);
        assert_frames_increase(&after, &p);
        prop_assert!(after.iter().all(|h| h.frame >= new_start));
        hits.append(&mut after);
        assert_beats_consecutive(&hits, &p);
        // Beats reached by the phase at frame end - 1 on the new device.
        let q_new = u128::from(beats[0]) * 60 * u128::from(new_sr);
        let pos = tl.position_at(end - 1);
        let phase = u128::from(pos.measure) * q_new + u128::from(pos.ticks);
        for pulse in &p.pulses {
            let n = u128::from(pulse.beats);
            let want = phase * n / q_new + 1;
            let got = hits.iter().filter(|h| h.pulse == pulse.id).count() as u128;
            prop_assert_eq!(got, want, "pulse {:?}", pulse.id);
        }
    }
}

// ------------------------------------------------------------ real-time ----

fn chunk(tl: &mut Timeline, p: &Pattern, t: u64, out: &mut Vec<BeatHit>) -> usize {
    out.clear();
    tl.events_in(p, t, t + 512, out);
    out.len()
}

#[test]
fn live_edits_do_not_allocate() {
    let old = pattern_of(&[7, 11, 5]);
    let mut grown = old.clone();
    grown.add_pulse();
    let mut five = grown.clone();
    five.set_beats(five.pulses[0].id, 5);
    let mut tl = Timeline::new(48_000, Bpm::new(300), &old, 0);
    // Capacity for the added pulse is reserved outside the real-time path.
    tl.sync_pattern(&grown);
    let mut out: Vec<BeatHit> = Vec::with_capacity(256);
    let mut emitted = 0;
    assert_no_alloc::assert_no_alloc(|| {
        let mut t = 0;
        for i in 0..2_000u64 {
            if i % 7 == 0 {
                tl.set_bpm(t, Bpm::new(40 + (i % 260) as i32));
            }
            emitted += chunk(&mut tl, &old, t, &mut out);
            t += 512;
        }
        assert_eq!(tl.apply_pattern(t, &old, &grown), ApplyMode::Now);
        let ApplyMode::AtNextOne(switch) = tl.apply_pattern(t, &grown, &five) else {
            panic!("reference change must wait");
        };
        assert_eq!(tl.pending_switch(), Some(switch));
        while t + 512 <= switch {
            emitted += chunk(&mut tl, &grown, t, &mut out);
            t += 512;
        }
        out.clear();
        tl.events_in(&grown, t, switch, &mut out);
        tl.events_in(&five, switch, t + 512, &mut out);
        emitted += out.len();
        t += 512;
        tl.rebase_sample_rate(t, 44_100, 0);
        for i in 0..1_000 {
            emitted += chunk(&mut tl, &five, i * 512, &mut out);
        }
    });
    assert!(emitted > 0);
}
