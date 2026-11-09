//! Starting a timeline somewhere other than measure 0 (a rebuilt audio
//! stream resuming the music), restarting it, and the fixed pulse capacity
//! that keeps every real-time call allocation-free.

use pm_core::{BeatHit, Bpm, MAX_PULSES, MusicalPosition, Pattern, Timeline};
use proptest::prelude::*;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// Builds a pattern whose pulses have exactly the given beat counts.
fn pattern_of(beats: &[u32]) -> Pattern {
    assert!(!beats.is_empty());
    let mut p = Pattern::default();
    while p.pulses.len() < beats.len() {
        p.add_pulse().unwrap();
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

fn render(tl: &mut Timeline, p: &Pattern, from: u64, to: u64) -> Vec<BeatHit> {
    let mut out = Vec::new();
    let mut t = from;
    while t < to {
        let end = (t + 4096).min(to);
        tl.events_in(p, t, end, &mut out);
        t = end;
    }
    out
}

fn frames_beats(hits: &[BeatHit]) -> Vec<(u64, u32)> {
    hits.iter().map(|h| (h.frame, h.beat)).collect()
}

#[test]
fn resume_at_a_beat_frame_plays_that_beat_first() {
    // 120 BPM, 48 kHz: pulse A beat 1 is at frame 24000.
    let p = Pattern::default();
    let a = p.pulses[0].id;
    let b = p.pulses[1].id;
    let old = Timeline::new(48_000, Bpm::new(120), &p, 0);
    let pos = old.position_at(24_000);
    let mut tl = Timeline::resume(48_000, Bpm::new(120), &p, pos, 0);
    let hits = render(&mut tl, &p, 0, 72_000);
    assert_eq!(hits[0].pulse, a);
    assert_eq!((hits[0].frame, hits[0].beat), (0, 1));
    // Then B beat 1 (old frame 32000), A beat 2 (48000), A beat 3, the one.
    assert_eq!(hits[1].pulse, b);
    assert_eq!((hits[1].frame, hits[1].beat), (8_000, 1));
    assert_eq!((hits[2].frame, hits[2].beat), (24_000, 2));
    assert_eq!(tl.position_at(72_000).measure, 1);
    assert_eq!(tl.position_at(72_000).ticks, 0);

    // One frame later, beat 1 was already played by the old stream.
    let pos = old.position_at(24_001);
    let mut tl = Timeline::resume(48_000, Bpm::new(120), &p, pos, 0);
    let hits = render(&mut tl, &p, 0, 72_000);
    assert_eq!(hits[0].pulse, b);
    assert_eq!((hits[0].frame, hits[0].beat), (7_999, 1));
}

#[test]
fn resume_converts_a_position_from_another_sample_rate() {
    // Old stream 48 kHz stopped right at A beat 1; new stream runs at 44.1 kHz.
    let p = Pattern::default();
    let old = Timeline::new(48_000, Bpm::new(120), &p, 0);
    let pos = old.position_at(24_000);
    assert_eq!(pos.ticks_per_measure, 4 * 60 * 48_000);
    let mut tl = Timeline::resume(44_100, Bpm::new(120), &p, pos, 1_000);
    assert_eq!(
        tl.position_at(1_000),
        MusicalPosition {
            measure: 0,
            ticks: 4 * 60 * 44_100 / 4,
            ticks_per_measure: 4 * 60 * 44_100,
        }
    );
    let hits = render(&mut tl, &p, 1_000, 1_000 + 2 * 88_200);
    assert_eq!(frames_beats(&hits[..1]), [(1_000, 1)]);
    // Next one: 1.5 s after the resume point.
    let one = hits.iter().find(|h| h.beat == 0).unwrap();
    assert_eq!(one.frame, 1_000 + 66_150);
}

#[test]
fn resume_with_a_position_from_the_same_rate_matches_continuing() {
    let p = pattern_of(&[7, 11, 5]);
    let mut old = Timeline::new(44_100, Bpm::new(137), &p, 0);
    old.set_bpm(12_345, Bpm::new(211));
    let f = 300_001;
    let pos = old.position_at(f);
    let cont = render(&mut old, &p, f, f + 500_000);
    let mut tl = Timeline::resume(44_100, Bpm::new(211), &p, pos, 0);
    let resumed: Vec<_> = render(&mut tl, &p, 0, 500_000)
        .into_iter()
        .map(|h| BeatHit {
            frame: h.frame + f,
            ..h
        })
        .collect();
    assert_eq!(resumed, cont);
}

#[test]
fn degenerate_position_starts_at_the_position_measure() {
    let p = Pattern::default();
    let pos = MusicalPosition {
        measure: 5,
        ticks: 0,
        ticks_per_measure: 0,
    };
    let tl = Timeline::resume(48_000, Bpm::new(120), &p, pos, 10);
    assert_eq!(tl.position_at(10).measure, 5);
    assert_eq!(tl.position_at(10).ticks, 0);
}

/// `MusicalPosition::start_of(m)` is the documented "start at the one" input
/// of `resume`: every pulse plays beat 0 at `start_frame`, and from there the
/// grid is a fresh timeline's, whatever the sample rate.
#[test]
fn start_of_a_measure_plays_every_one_at_the_start_frame() {
    let p = pattern_of(&[4, 3, 5]);
    for sr in [16_000u32, 44_100, 48_000] {
        let pos = MusicalPosition::start_of(7);
        assert_eq!(pos.ticks_per_measure, 0);
        let mut tl = Timeline::resume(sr, Bpm::new(97), &p, pos, 1_000);
        assert_eq!(tl.position_at(1_000).measure, 7);
        let mut got = Vec::new();
        tl.events_in(&p, 1_000, 1_000 + 10 * u64::from(sr), &mut got);
        let mut want = Vec::new();
        Timeline::new(sr, Bpm::new(97), &p, 1_000).events_in(
            &p,
            1_000,
            1_000 + 10 * u64::from(sr),
            &mut want,
        );
        assert_eq!(got, want, "sr {sr}");
        let firsts: Vec<_> = got.iter().take(3).map(|h| (h.frame, h.beat)).collect();
        assert_eq!(firsts, vec![(1_000, 0); 3], "sr {sr}");
    }
}

fn resume_case() -> impl Strategy<Value = (u32, u32, u16, u16, Vec<u32>, u64, Option<i8>)> {
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
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// A fresh timeline resumed from `position_at(f)` of the old one emits
    /// exactly what `rebase_sample_rate(f, ..)` on the old one would: no beat
    /// lost or doubled at the seam, including when `f` is on (or one frame
    /// around) a beat frame.
    #[test]
    fn resume_matches_rebase((old_sr, new_sr, bpm0, bpm1, beats, at, snap) in resume_case()) {
        let p = pattern_of(&beats);
        let bpm0 = Bpm::new(i32::from(bpm0));
        let bpm1 = Bpm::new(i32::from(bpm1));
        let first = at / 3 + 1;
        let twin = |f: u64| {
            let mut tl = Timeline::new(old_sr, bpm0, &p, 0);
            render(&mut tl, &p, 0, first);
            tl.set_bpm(first, bpm1);
            render(&mut tl, &p, first, f);
            tl
        };
        // Optionally snap the seam onto the next hit, shifted by -1..=1.
        let mut probe = twin(first);
        let from = at.max(first);
        let next = render(&mut probe, &p, from, from + 400_000);
        let f = match (snap, next.first()) {
            (Some(d), Some(hit)) => hit.frame.saturating_add_signed(i64::from(d)).max(first),
            _ => from,
        };
        let mut rebased = twin(f);
        let pos = rebased.position_at(f);
        rebased.rebase_sample_rate(f, new_sr, 0);
        let end = 3 * u64::from(beats[0]) * 60 * u64::from(new_sr) / 40;
        let want = render(&mut rebased, &p, 0, end);
        let mut resumed = Timeline::resume(new_sr, bpm1, &p, pos, 0);
        let got = render(&mut resumed, &p, 0, end);
        prop_assert_eq!(got, want);
    }
}

#[test]
fn restart_plays_measure_zero_again() {
    let p = Pattern::default();
    let mut tl = Timeline::new(48_000, Bpm::new(120), &p, 0);
    tl.set_bpm(10_000, Bpm::new(97));
    render(&mut tl, &p, 0, 100_000);
    let mut three = p.clone();
    three.set_beats(three.pulses[0].id, 3);
    tl.sync_pattern(&three);
    assert_no_alloc::assert_no_alloc(|| tl.restart(&three, 200_000));
    assert_eq!(tl.pending_switch(), None);
    assert_eq!(
        tl.position_at(200_000),
        MusicalPosition {
            measure: 0,
            ticks: 0,
            ticks_per_measure: 3 * 60 * 48_000,
        }
    );
    let mut fresh = Timeline::new(48_000, Bpm::new(97), &three, 200_000);
    assert_eq!(
        render(&mut tl, &three, 200_000, 600_000),
        render(&mut fresh, &three, 200_000, 600_000)
    );
}

#[test]
fn sync_pattern_up_to_max_pulses_does_not_allocate() {
    let patterns: Vec<Pattern> = (2..=MAX_PULSES).map(|n| pattern_of(&vec![4; n])).collect();
    let mut tl = Timeline::new(48_000, Bpm::new(120), &patterns[0], 0);
    let mut out: Vec<BeatHit> = Vec::with_capacity(4 * MAX_PULSES);
    assert_no_alloc::assert_no_alloc(|| {
        for w in patterns.windows(2) {
            tl.sync_pattern(&w[1]);
            tl.apply_pattern(0, &w[0], &w[1]);
        }
        out.clear();
        tl.events_in(patterns.last().unwrap(), 0, 1, &mut out);
    });
    // Every pulse fires its beat 0 at frame 0 (all added before any frame).
    assert_eq!(out.len(), MAX_PULSES);
}

#[test]
fn pattern_cannot_grow_past_max_pulses() {
    let mut p = Pattern::default();
    while p.pulses.len() < MAX_PULSES {
        assert!(p.add_pulse().is_some());
    }
    let before = p.clone();
    assert_eq!(p.add_pulse(), None);
    assert_eq!(p, before);
}
