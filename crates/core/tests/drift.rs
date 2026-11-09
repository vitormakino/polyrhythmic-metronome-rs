//! Drift tests for the exact timeline, through the public API only.

use pm_core::{BeatHit, Bpm, Intensity, Pattern, Timeline};
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

/// Ceiling division for the non-negative values used in the closed formula.
fn ceil_div(a: u128, b: u128) -> u128 {
    a.div_ceil(b)
}

/// Closed formula, independent of the timeline: frame of beat `k` of measure
/// `m` for a pulse of `n` beats, counted from `start`.
fn expected_frame(start: u64, sr: u32, bpm: u16, r: u32, n: u32, m: u64, k: u32) -> u64 {
    let num = (u128::from(m) * u128::from(n) + u128::from(k)) * u128::from(r) * 60 * u128::from(sr);
    start + ceil_div(num, u128::from(n) * u128::from(bpm)) as u64
}

/// Renders `[from, to)` in chunks whose sizes cycle through `chunks`.
fn render(tl: &mut Timeline, p: &Pattern, from: u64, to: u64, chunks: &[u64]) -> Vec<BeatHit> {
    let mut out = Vec::new();
    let mut t = from;
    let mut i = 0;
    while t < to {
        let end = (t + chunks[i % chunks.len()]).min(to);
        tl.events_in(p, t, end, &mut out);
        t = end;
        i += 1;
    }
    out
}

/// Every hit (including muted/disabled ones) of every pulse in `[from, to)`,
/// computed from the closed formula and sorted by (frame, pulse index).
fn expected_hits(start: u64, sr: u32, bpm: u16, beats: &[u32], to: u64) -> Vec<(u64, usize, u32)> {
    let r = beats[0];
    let mut v = Vec::new();
    for (idx, &n) in beats.iter().enumerate() {
        let mut m = 0;
        'outer: loop {
            for k in 0..n {
                let f = expected_frame(start, sr, bpm, r, n, m, k);
                if f >= to {
                    break 'outer;
                }
                v.push((f, idx, k));
            }
            m += 1;
        }
    }
    v.sort();
    v
}

fn assert_sorted(hits: &[BeatHit], p: &Pattern) {
    let index = |h: &BeatHit| p.pulses.iter().position(|x| x.id == h.pulse).unwrap();
    for w in hits.windows(2) {
        assert!(
            (w[0].frame, index(&w[0])) < (w[1].frame, index(&w[1])),
            "out of order: {:?} then {:?}",
            w[0],
            w[1]
        );
    }
}

#[test]
fn four_four_at_120_48k() {
    let p = Pattern::default();
    let (a, b) = (p.pulses[0].id, p.pulses[1].id);
    let mut tl = Timeline::new(48_000, Bpm::new(120), &p, 0);
    let hits = render(&mut tl, &p, 0, 96_001, &[512]);
    let frames_of = |id| {
        hits.iter()
            .filter(|h| h.pulse == id)
            .map(|h| h.frame)
            .collect::<Vec<_>>()
    };
    assert_eq!(frames_of(a), vec![0, 24_000, 48_000, 72_000, 96_000]);
    assert_eq!(frames_of(b), vec![0, 32_000, 64_000, 96_000]);
    // Ties at the same frame come in pulse-index order.
    assert_eq!(hits[0].pulse, a);
    assert_eq!(hits[1].pulse, b);
    assert_sorted(&hits, &p);
    let first = hits[0];
    assert!(first.accent);
    assert_eq!(first.beat, 0);
    assert_eq!(first.gain, Intensity::Hard.gain());
    assert_eq!(first.sound, p.pulses[0].sound);
    let second_a = hits
        .iter()
        .find(|h| h.pulse == a && h.frame == 24_000)
        .unwrap();
    assert!(!second_a.accent);
    assert_eq!(second_a.beat, 1);
    assert_eq!(second_a.gain, Intensity::Medium.gain());
}

/// 7 against 11 at BPM 300, sr 48000: walks `seconds` of audio in 512-frame
/// chunks and checks every single hit against the exact closed formula.
fn seven_eleven_exact_for(seconds: u64) {
    const MEASURE: u64 = 67_200;
    let p = pattern_of(&[7, 11]);
    let ids = [p.pulses[0].id, p.pulses[1].id];
    let beats = [7u64, 11];
    let mut tl = Timeline::new(48_000, Bpm::new(300), &p, 0);
    let total = seconds * 48_000;
    let mut out: Vec<BeatHit> = Vec::with_capacity(64);
    // Next expected (measure, beat) per pulse.
    let mut next = [(0u64, 0u64); 2];
    let mut t = 0;
    while t < total {
        let end = (t + 512).min(total);
        out.clear();
        tl.events_in(&p, t, end, &mut out);
        for h in &out {
            let i = ids.iter().position(|&id| id == h.pulse).unwrap();
            let (m, k) = next[i];
            assert_eq!(u64::from(h.beat), k);
            let want = m * MEASURE + (k * MEASURE).div_ceil(beats[i]);
            assert_eq!(h.frame, want, "pulse {i} measure {m} beat {k}");
            if k == 0 {
                assert_eq!(h.frame, m * MEASURE);
            }
            next[i] = if k + 1 == beats[i] {
                (m + 1, 0)
            } else {
                (m, k + 1)
            };
        }
        t = end;
    }
    // Nothing was skipped at the end: each pulse emitted every beat with
    // frame < total.
    for (i, &n) in beats.iter().enumerate() {
        let mut c = 0;
        while (c / n) * MEASURE + ((c % n) * MEASURE).div_ceil(n) < total {
            c += 1;
        }
        assert_eq!(next[i].0 * n + next[i].1, c, "pulse {i} beat count");
    }
}

#[test]
fn seven_eleven_1h_every_one_is_exact() {
    seven_eleven_exact_for(3_600);
}

#[test]
fn seven_eleven_24h_every_one_is_exact() {
    seven_eleven_exact_for(24 * 3_600);
}

/// Ten minutes of 7:11 at an awkward tempo, so the ceil actually rounds.
const TEN_MIN: u64 = 10 * 60 * 44_100;

fn ten_minutes(chunks: &[u64]) -> Vec<BeatHit> {
    let p = pattern_of(&[7, 11]);
    let mut tl = Timeline::new(44_100, Bpm::new(137), &p, 0);
    render(&mut tl, &p, 0, TEN_MIN, chunks)
}

#[test]
fn chunking_does_not_matter() {
    let reference = ten_minutes(&[TEN_MIN]);
    let p = pattern_of(&[7, 11]);
    let want: Vec<_> = expected_hits(0, 44_100, 137, &[7, 11], TEN_MIN)
        .into_iter()
        .map(|(f, i, k)| (f, p.pulses[i].id, k))
        .collect();
    let got: Vec<_> = reference
        .iter()
        .map(|h| (h.frame, h.pulse, h.beat))
        .collect();
    assert_eq!(got, want);
    for size in [1, 64, 512, 4096] {
        assert_eq!(ten_minutes(&[size]), reference, "chunk size {size}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]

    #[test]
    fn chunking_does_not_matter_random_sizes(
        sizes in prop::collection::vec(1u64..=8192, 1000)
    ) {
        prop_assert_eq!(ten_minutes(&sizes), ten_minutes(&[TEN_MIN]));
    }
}

#[test]
fn exact_far_in_the_future() {
    let start = 1u64 << 40;
    let p = Pattern::default();
    let mut tl = Timeline::new(48_000, Bpm::new(120), &p, start);
    let want: Vec<_> = expected_hits(start, 48_000, 120, &[4, 3], start + 10 * 96_000)
        .into_iter()
        .map(|(f, i, k)| (f, p.pulses[i].id, k))
        .collect();
    let hits = render(&mut tl, &p, start, start + 10 * 96_000, &[512]);
    let got: Vec<_> = hits.iter().map(|h| (h.frame, h.pulse, h.beat)).collect();
    assert_eq!(got, want);
    // Jumping ~6 years ahead without rendering the gap still lands exactly.
    let m = 100_000_000u64;
    let far = start + m * 96_000;
    let hits = render(&mut tl, &p, far, far + 96_000, &[512]);
    let got: Vec<_> = hits.iter().map(|h| (h.frame, h.beat)).collect();
    assert_eq!(
        got,
        vec![
            (far, 0),
            (far, 0),
            (far + 24_000, 1),
            (far + 32_000, 1),
            (far + 48_000, 2),
            (far + 64_000, 2),
            (far + 72_000, 3),
        ]
    );
}

fn grid_case() -> impl Strategy<Value = (u32, u16, Vec<u32>, Vec<u64>)> {
    (
        prop::sample::select(vec![44_100u32, 48_000, 96_000]),
        40u16..=300,
        prop::collection::vec(1u32..=64, 1..=6),
        prop::collection::vec(1u64..=4096, 1..=16),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn proptest_grid((sr, bpm, beats, chunks) in grid_case()) {
        let p = pattern_of(&beats);
        let measure_frames = u64::from(beats[0]) * 60 * u64::from(sr) / u64::from(bpm);
        let to = 3 * measure_frames + 1;
        let mut tl = Timeline::new(sr, Bpm::new(i32::from(bpm)), &p, 0);
        let hits = render(&mut tl, &p, 0, to, &chunks);
        let got: Vec<_> = hits
            .iter()
            .map(|h| {
                let i = p.pulses.iter().position(|x| x.id == h.pulse).unwrap();
                (h.frame, i, h.beat)
            })
            .collect();
        prop_assert_eq!(got, expected_hits(0, sr, bpm, &beats, to));
    }
}

#[test]
fn muted_and_disabled_not_emitted_but_numbered() {
    let mut p = Pattern::default();
    let (a, b) = (p.pulses[0].id, p.pulses[1].id);
    p.pulses[0].intensities[1] = Intensity::Mute;
    p.set_enabled(b, false);
    let mut tl = Timeline::new(48_000, Bpm::new(120), &p, 0);
    let hits = render(&mut tl, &p, 0, 96_000, &[512]);
    assert!(hits.iter().all(|h| h.pulse == a));
    let got: Vec<_> = hits.iter().map(|h| (h.frame, h.beat)).collect();
    assert_eq!(got, vec![(0, 0), (48_000, 2), (72_000, 3)]);
}

#[test]
fn position_at_counts_measures_and_ticks() {
    let p = Pattern::default();
    let start = 1_000;
    let tl = Timeline::new(48_000, Bpm::new(120), &p, start);
    let q = 4 * 60 * 48_000;
    let pos = tl.position_at(start);
    assert_eq!((pos.measure, pos.ticks, pos.ticks_per_measure), (0, 0, q));
    // Half a measure in: 48000 frames at 120 ticks per frame.
    let pos = tl.position_at(start + 48_000);
    assert_eq!((pos.measure, pos.ticks), (0, q / 2));
    let pos = tl.position_at(start + 3 * 96_000 + 24_000);
    assert_eq!((pos.measure, pos.ticks), (3, q / 4));
}

#[test]
fn events_in_does_not_allocate() {
    let p = pattern_of(&[7, 11, 5, 64]);
    let mut tl = Timeline::new(48_000, Bpm::new(300), &p, 0);
    let mut out: Vec<BeatHit> = Vec::with_capacity(256);
    let mut emitted = 0;
    assert_no_alloc::assert_no_alloc(|| {
        let mut t = 0;
        while t < 48_000 * 60 {
            out.clear();
            tl.events_in(&p, t, t + 512, &mut out);
            emitted += out.len();
            t += 512;
        }
    });
    assert!(emitted > 0);
}

#[test]
fn sync_pattern_adds_cursors_for_new_pulses() {
    let p = Pattern::default();
    let mut tl = Timeline::new(48_000, Bpm::new(120), &p, 0);
    let mut bigger = p.clone();
    bigger.add_pulse();
    let mut out = Vec::new();
    tl.sync_pattern(&bigger);
    tl.events_in(&bigger, 0, 1, &mut out);
    assert_eq!(out.len(), 3);
}
