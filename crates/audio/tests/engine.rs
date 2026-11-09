//! The real-time engine without a device: commands in, samples and beat
//! events out. The owner's requirements: clicks land on exact frames and
//! never drift, and `render` never allocates, locks or blocks.

use std::time::{Duration, Instant};

use pm_audio::{BeatEvent, Command, Engine, EngineHandle, EngineInit, PatternUpdate};
use pm_core::{Bpm, Intensity, Pattern, SampleBank, SoundType, Timeline};

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const SR: u32 = 48_000;
const BUF: usize = 512;

fn engine(pattern: &Pattern, bpm: i32) -> (Engine, EngineHandle) {
    Engine::new(
        SR,
        1,
        EngineInit {
            pattern: pattern.clone(),
            bpm: Bpm::new(bpm),
            resume: None,
        },
    )
}

fn update(pattern: &Pattern) -> Box<PatternUpdate> {
    PatternUpdate::build(SR, pattern.clone())
}

fn send(h: &mut EngineHandle, c: Command) {
    assert!(h.commands.push(c).is_ok(), "command queue full");
}

/// Renders `buffers` mono buffers of `BUF` frames, draining events after each.
fn render(
    e: &mut Engine,
    h: &mut EngineHandle,
    buffers: usize,
    events: &mut Vec<BeatEvent>,
) -> Vec<f32> {
    let mut all = Vec::with_capacity(buffers * BUF);
    let mut out = [0.0f32; BUF];
    for _ in 0..buffers {
        e.render(&mut out, Duration::ZERO);
        all.extend_from_slice(&out);
        while let Ok(ev) = h.events.pop() {
            events.push(ev);
        }
    }
    all
}

/// Offline reference: every hit's clip × gain summed at exactly its frame.
fn mix(hits: &[(u64, &[f32], f32)], len: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    for &(frame, clip, gain) in hits {
        for (i, s) in clip.iter().enumerate() {
            let at = frame as usize + i;
            if at < len {
                out[at] += s * gain;
            }
        }
    }
    out
}

fn index_of(p: &Pattern, hit: &pm_core::BeatHit) -> usize {
    p.pulses.iter().position(|x| x.id == hit.pulse).unwrap()
}

fn timeline_hits(p: &Pattern, bpm: i32, to: u64) -> Vec<pm_core::BeatHit> {
    let mut tl = Timeline::new(SR, Bpm::new(bpm), p, 0);
    let mut hits = Vec::new();
    tl.events_in(p, 0, to, &mut hits);
    hits
}

fn first_mismatch(got: &[f32], want: &[f32]) -> Option<(usize, f32, f32)> {
    assert_eq!(got.len(), want.len());
    got.iter()
        .zip(want)
        .enumerate()
        .find(|(_, (g, w))| g.to_bits() != w.to_bits())
        .map(|(i, (g, w))| (i, *g, *w))
}

#[test]
fn render_is_allocation_free() {
    let p = Pattern::default();
    // Grows to 5 pulses and changes the reference cycle (waits for the one),
    let mut grown = p.clone();
    for _ in 0..3 {
        grown.add_pulse().unwrap();
    }
    grown.set_beats(grown.pulses[0].id, 5);
    // then an immediate sound change on top of it.
    let mut bell = grown.clone();
    bell.set_sound(bell.pulses[1].id, SoundType::Bell);
    let mut first = Some(update(&grown));
    let mut second = Some(update(&bell));
    let (mut e, mut h) = engine(&p, 120);
    send(&mut h, Command::Play);
    let mut out = [0.0f32; BUF];
    let mut events = 0usize;
    assert_no_alloc::assert_no_alloc(|| {
        for i in 0..(10 * SR as usize / BUF) {
            match i {
                100 => send(&mut h, Command::SetBpm(Bpm::new(173))),
                150 => send(&mut h, Command::SetPattern(first.take().unwrap())),
                600 => send(&mut h, Command::SetPattern(second.take().unwrap())),
                _ => {}
            }
            e.render(&mut out, Duration::from_millis(5));
            while h.events.pop().is_ok() {
                events += 1;
            }
            let _ = h.position.snapshot();
        }
    });
    assert!(events > 0);
    // The initial pattern (replaced at the switch) and `grown` (replaced by
    // `bell`) came back to be dropped here, off the audio thread.
    let mut garbage = Vec::new();
    while let Ok(g) = h.garbage.pop() {
        garbage.push(g.pattern);
    }
    assert_eq!(garbage, [p, grown]);
}

#[test]
fn clicks_land_on_exact_frames() {
    let p = Pattern::default();
    let (mut e, mut h) = engine(&p, 120);
    e.set_soft_clip(false);
    send(&mut h, Command::Play);
    let total = 3 * SR as usize;
    let got = render(&mut e, &mut h, total.div_ceil(BUF), &mut Vec::new());
    let got = &got[..total];

    let hits = timeline_hits(&p, 120, total as u64);
    let frames: Vec<u64> = hits.iter().map(|h| h.frame).collect();
    assert_eq!(
        &frames[..10],
        [
            0, 0, 24_000, 32_000, 48_000, 64_000, 72_000, 96_000, 96_000, 120_000
        ]
    );
    let bank = SampleBank::build(SR, &p);
    let placed: Vec<_> = hits
        .iter()
        .map(|hit| {
            (
                hit.frame,
                bank.clip(index_of(&p, hit), hit.accent),
                hit.gain,
            )
        })
        .collect();
    let want = mix(&placed, total);
    assert_eq!(first_mismatch(got, &want), None);
    // Clips start at zero amplitude (5 ms linear attack): the first sound is one frame in.
    assert!(got[1] != 0.0 && got[23_999] == 0.0 && got[24_000] == 0.0 && got[24_001] != 0.0);
}

#[test]
fn events_match_timeline() {
    let p = Pattern::default();
    let (mut e, mut h) = engine(&p, 120);
    send(&mut h, Command::Play);
    let total = 30 * SR as usize;
    let buffers = total / BUF;
    let mut events = Vec::new();
    render(&mut e, &mut h, buffers, &mut events);
    let want: Vec<_> = timeline_hits(&p, 120, (buffers * BUF) as u64)
        .iter()
        .map(|h| (h.frame, h.pulse, h.beat))
        .collect();
    let got: Vec<_> = events.iter().map(|e| (e.frame, e.pulse, e.beat)).collect();
    assert_eq!(got.len(), 105);
    assert_eq!(got, want);
    assert_eq!(
        h.dropped_events.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[test]
fn event_time_includes_device_offset() {
    let p = Pattern::default();
    let (mut e, mut h) = engine(&p, 120);
    send(&mut h, Command::Play);
    let mut out = [0.0f32; BUF];
    // Frame 0 and, one buffer later, nothing; then A beat 1 at 24000.
    let offset = Duration::from_millis(100);
    let before = Instant::now();
    e.render(&mut out, offset);
    let after = Instant::now();
    let ev = h.events.pop().unwrap();
    assert_eq!(ev.frame, 0);
    assert!(ev.at >= before + offset && ev.at <= after + offset);
    while h.events.pop().is_ok() {}
    let mut big = vec![0.0f32; 24_000 - BUF + 1];
    let before = Instant::now();
    e.render(&mut big, offset);
    let after = Instant::now();
    let ev = h.events.pop().unwrap();
    assert_eq!(ev.frame, 24_000);
    // 24000 - 512 frames into this buffer.
    let delta = Duration::from_nanos((24_000 - BUF as u64) * 1_000_000_000 / u64::from(SR));
    assert!(ev.at >= before + offset + delta && ev.at <= after + offset + delta);
}

#[test]
fn dense_overlap_never_exceeds_unity() {
    let mut p = Pattern::default();
    while p.pulses.len() < 6 {
        p.add_pulse().unwrap();
    }
    for pulse in &mut p.pulses {
        pulse.beats = 64;
        pulse.intensities = vec![Intensity::Hard; 64];
        pulse.sound = SoundType::Bell;
    }
    let (mut e, mut h) = engine(&p, 300);
    send(&mut h, Command::Play);
    let mut out = [0.0f32; BUF];
    let mut peak = 0.0f32;
    let mut hits = 0usize;
    for _ in 0..(60 * SR as usize / BUF) {
        e.render(&mut out, Duration::ZERO);
        while h.events.pop().is_ok() {
            hits += 1;
        }
        for s in out {
            assert!(!s.is_nan());
            peak = peak.max(s.abs());
        }
    }
    assert!(peak <= 1.0, "peak {peak}");
    // Six Hard bells on one frame sum far above 1: the clipper did work.
    assert!(peak > 0.9, "peak {peak}");
    assert_eq!(hits, 6 * 5 * 60);
}

#[test]
fn all_muted_renders_silence_but_advances() {
    let mut p = Pattern::default();
    let ids: Vec<_> = p.pulses.iter().map(|x| x.id).collect();
    for id in ids {
        p.set_enabled(id, false);
    }
    let (mut e, mut h) = engine(&p, 120);
    send(&mut h, Command::Play);
    let mut events = Vec::new();
    let out = render(&mut e, &mut h, 200, &mut events);
    assert!(out.iter().all(|&s| s == 0.0));
    assert!(events.is_empty());
    let snap = h.position.snapshot();
    assert_eq!(snap.frame, 200 * BUF as u64);
    assert!(snap.playing);
    let tl = Timeline::new(SR, Bpm::new(120), &p, 0);
    assert_eq!(snap.position, tl.position_at(200 * BUF as u64));
    assert_eq!(snap.position.measure, 1);
}

#[test]
fn stop_then_play_restarts_at_one() {
    let p = Pattern::default();
    let (mut e, mut h) = engine(&p, 120);
    e.set_soft_clip(false);
    send(&mut h, Command::Play);
    let mut events = Vec::new();
    let buffers = SR as usize / BUF + 1; // a bit over 1 s: A beat 2 at 48000 still ringing
    render(&mut e, &mut h, buffers, &mut events);
    send(&mut h, Command::Stop);
    events.clear();
    let stopped = render(&mut e, &mut h, 1, &mut events);
    assert!(stopped.iter().all(|&s| s == 0.0), "Stop silences at once");
    assert!(events.is_empty());
    assert!(!h.position.snapshot().playing);

    send(&mut h, Command::Play);
    let start = ((buffers + 1) * BUF) as u64;
    let got = render(&mut e, &mut h, 2 * SR as usize / BUF, &mut events);
    assert_eq!(events[0].frame, start);
    assert_eq!(events[0].beat, 0);
    assert_eq!(events[1].frame, start);
    assert_eq!(events[1].beat, 0);
    let shifted: Vec<_> = events
        .iter()
        .map(|e| (e.frame - start, e.pulse, e.beat))
        .collect();
    let want: Vec<_> = timeline_hits(&p, 120, got.len() as u64)
        .iter()
        .map(|h| (h.frame, h.pulse, h.beat))
        .collect();
    assert_eq!(shifted, want);
    let bank = SampleBank::build(SR, &p);
    let one = mix(
        &[(0, bank.clip(0, true), 1.0), (0, bank.clip(1, true), 1.0)],
        BUF,
    );
    assert_eq!(first_mismatch(&got[..BUF], &one), None);
}

#[test]
fn pattern_switch_uses_new_bank_from_the_one() {
    let old = Pattern::default();
    let mut new = old.clone();
    let a = new.pulses[0].id;
    new.set_beats(a, 3);
    new.set_sound(a, SoundType::Bell);
    new.pulses[0].frequency_hz = 1_200.0;
    let (mut e, mut h) = engine(&old, 120);
    e.set_soft_clip(false);
    send(&mut h, Command::Play);
    let mut head = render(&mut e, &mut h, 1, &mut Vec::new());
    send(&mut h, Command::SetPattern(update(&new)));
    let total = 4 * SR as usize;
    head.extend(render(&mut e, &mut h, total / BUF - 1, &mut Vec::new()));

    let mut tl = Timeline::new(SR, Bpm::new(120), &old, 0);
    let mut before = Vec::new();
    tl.events_in(&old, 0, BUF as u64, &mut before);
    tl.sync_pattern(&new);
    let pm_core::ApplyMode::AtNextOne(switch) = tl.apply_pattern(BUF as u64, &old, &new) else {
        panic!("reference change waits for the one");
    };
    assert_eq!(switch, 96_000);
    tl.events_in(&old, BUF as u64, switch, &mut before);
    let mut after = Vec::new();
    tl.events_in(&new, switch, total as u64, &mut after);
    let (old_bank, new_bank) = (SampleBank::build(SR, &old), SampleBank::build(SR, &new));
    let mut placed: Vec<_> = before
        .iter()
        .map(|hit| {
            (
                hit.frame,
                old_bank.clip(index_of(&old, hit), hit.accent),
                hit.gain,
            )
        })
        .collect();
    placed.extend(after.iter().map(|hit| {
        (
            hit.frame,
            new_bank.clip(index_of(&new, hit), hit.accent),
            hit.gain,
        )
    }));
    let want = mix(&placed, total);
    assert_eq!(first_mismatch(&head, &want), None);
    // The old pattern comes back once it stopped sounding.
    assert_eq!(h.garbage.pop().unwrap().pattern, old);
}

#[test]
fn replaced_patterns_return_through_garbage_even_when_it_is_full() {
    let p = Pattern::default();
    let b = p.pulses[1].id;
    let sounds = [SoundType::Sine, SoundType::Bell, SoundType::Electronic];
    let mut updates: Vec<Option<Box<PatternUpdate>>> = (0..20)
        .map(|i| {
            let mut q = p.clone();
            q.set_sound(b, sounds[i % 3]);
            Some(update(&q))
        })
        .collect();
    let (mut e, mut h) = engine(&p, 120);
    send(&mut h, Command::Play);
    let mut out = [0.0f32; BUF];
    // Nobody empties `garbage` meanwhile: the engine must hold the boxes
    // (or the commands) instead of dropping anything on the audio thread.
    assert_no_alloc::assert_no_alloc(|| {
        for slot in &mut updates {
            send(&mut h, Command::SetPattern(slot.take().unwrap()));
            e.render(&mut out, Duration::ZERO);
            while h.events.pop().is_ok() {}
        }
    });
    let mut returned = Vec::new();
    for _ in 0..200 {
        while let Ok(g) = h.garbage.pop() {
            returned.push(g.pattern);
        }
        e.render(&mut out, Duration::ZERO);
        while h.events.pop().is_ok() {}
    }
    // The initial pattern and the first 19 updates (a box waits while one of
    // its clicks still rings, so the order may differ); the last is live.
    let label = |q: &Pattern| format!("{:?}", q.pulses[1].sound);
    let mut got: Vec<_> = returned.iter().map(label).collect();
    let mut want: Vec<_> = std::iter::once(label(&p))
        .chain((0..19).map(|i| format!("{:?}", sounds[i % 3])))
        .collect();
    got.sort();
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn resume_continues_the_grid_on_a_new_sample_rate() {
    let p = Pattern::default();
    let (mut e, mut h) = engine(&p, 120);
    send(&mut h, Command::Play);
    render(&mut e, &mut h, 100, &mut Vec::new());
    let snap = h.position.snapshot();
    assert_eq!(snap.frame, 51_200);
    let pos = snap.position;
    assert_eq!(
        pos,
        Timeline::new(SR, Bpm::new(120), &p, 0).position_at(51_200)
    );

    let (mut e2, mut h2) = Engine::new(
        44_100,
        2,
        EngineInit {
            pattern: p.clone(),
            bpm: Bpm::new(120),
            resume: Some(pos),
        },
    );
    let mut out = vec![0.0f32; 2 * BUF];
    let mut got = Vec::new();
    for _ in 0..200 {
        e2.render(&mut out, Duration::ZERO);
        while let Ok(ev) = h2.events.pop() {
            got.push((ev.frame, ev.pulse, ev.beat));
        }
    }
    assert!(h2.position.snapshot().playing, "resume starts playing");
    let mut tl = Timeline::resume(44_100, Bpm::new(120), &p, pos, 0);
    let mut want = Vec::new();
    tl.events_in(&p, 0, 200 * BUF as u64, &mut want);
    let want: Vec<_> = want.iter().map(|h| (h.frame, h.pulse, h.beat)).collect();
    assert_eq!(got, want);
    // Old frame 51200: B beat 2 (old 64000) is 12800 old frames = 11760 new
    // frames ahead, A beat 3 (old 72000) 20800 old = 19110 new frames ahead.
    assert_eq!(got[0], (11_760, p.pulses[1].id, 2));
    assert_eq!(got[1], (19_110, p.pulses[0].id, 3));
}

#[test]
fn stereo_gets_the_same_mix_on_every_channel() {
    let p = Pattern::default();
    let (mut e, mut h) = Engine::new(
        SR,
        2,
        EngineInit {
            pattern: p.clone(),
            bpm: Bpm::new(120),
            resume: None,
        },
    );
    let (mut m, mut hm) = engine(&p, 120);
    send(&mut h, Command::Play);
    send(&mut hm, Command::Play);
    let mut st = [1.0f32; 2 * BUF];
    let mut mono = [0.0f32; BUF];
    e.render(&mut st, Duration::ZERO);
    m.render(&mut mono, Duration::ZERO);
    for (i, s) in mono.iter().enumerate() {
        assert_eq!((st[2 * i], st[2 * i + 1]), (*s, *s));
    }
    assert!(mono[1] != 0.0);
}

#[test]
fn ringing_click_survives_a_pattern_edit() {
    let old = Pattern::default();
    let mut new = old.clone();
    new.set_sound(new.pulses[1].id, SoundType::Bell);
    let (mut e, mut h) = engine(&old, 120);
    e.set_soft_clip(false);
    send(&mut h, Command::Play);
    // Both clicks of frame 0 are still ringing when the edit lands at 1024.
    let mut got = render(&mut e, &mut h, 2, &mut Vec::new());
    send(&mut h, Command::SetPattern(update(&new)));
    got.extend(render(&mut e, &mut h, 98, &mut Vec::new()));
    assert!(h.garbage.pop().is_ok(), "old pattern released once silent");

    let hits = timeline_hits(&old, 120, got.len() as u64);
    let (old_bank, new_bank) = (SampleBank::build(SR, &old), SampleBank::build(SR, &new));
    let placed: Vec<_> = hits
        .iter()
        .map(|hit| {
            let bank = if hit.frame < 1024 {
                &old_bank
            } else {
                &new_bank
            };
            (
                hit.frame,
                bank.clip(index_of(&old, hit), hit.accent),
                hit.gain,
            )
        })
        .collect();
    let want = mix(&placed, got.len());
    assert_eq!(first_mismatch(&got, &want), None);
}
