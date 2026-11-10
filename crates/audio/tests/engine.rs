//! The real-time engine without a device: commands in, samples and beat
//! events out. The owner's requirements: clicks land on exact frames and
//! never drift, and `render` never allocates, locks or blocks.

use std::time::{Duration, Instant};

use pm_audio::{BeatEvent, Command, Engine, EngineHandle, EngineInit, PatternUpdate};
use pm_core::session::{
    Jump, MuteCycle, OnFinish, PulseState, Ramp, RandomMute, SessionPlan, Span, Stage, Tempo,
};
use pm_core::synth::end_chime;
use pm_core::{Bpm, Intensity, MAX_PULSES, Pattern, SampleBank, SoundType, Timeline};

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
            session: None,
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
            session: None,
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
            session: None,
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

// ---------------------------------------------------------------------------
// Practice sessions. Default pattern 4:3: at 120 BPM a bar of pulse 1 is 2 s.

const BAR: u64 = 2 * SR as u64;

fn sstage(length: Span) -> Stage {
    Stage {
        length,
        tempo: None,
        mute: None,
        random_mute: None,
        pulses: [None; MAX_PULSES],
    }
}

fn splan(stages: Vec<Stage>, on_finish: OnFinish) -> SessionPlan {
    SessionPlan {
        name: "test".into(),
        stages,
        on_finish,
        show_muted_visuals: false,
        seed: 7,
    }
}

/// An engine (soft clip off) that runs `plan` from the first frame.
fn session_engine(p: &Pattern, bpm: i32, plan: SessionPlan) -> (Engine, EngineHandle) {
    let (mut e, mut h) = engine(p, bpm);
    e.set_soft_clip(false);
    send(&mut h, Command::Session(Some(Box::new(plan))));
    send(&mut h, Command::Play);
    (e, h)
}

type Ev = (u64, pm_core::PulseId, u32, bool);

fn evs(events: &[BeatEvent]) -> Vec<Ev> {
    events
        .iter()
        .map(|e| (e.frame, e.pulse, e.beat, e.muted))
        .collect()
}

fn hit_evs(hits: &[pm_core::BeatHit]) -> Vec<Ev> {
    hits.iter()
        .map(|h| (h.frame, h.pulse, h.beat, false))
        .collect()
}

#[test]
fn no_session_output_is_identical() {
    let p = Pattern::default();
    let buffers = (10 * SR as usize).div_ceil(BUF);
    let (mut a, mut ha) = engine(&p, 120);
    let (mut b, mut hb) = engine(&p, 120);
    a.set_soft_clip(false);
    b.set_soft_clip(false);
    send(&mut ha, Command::Play);
    send(&mut hb, Command::Session(None));
    send(&mut hb, Command::Play);
    let (mut ea, mut eb) = (Vec::new(), Vec::new());
    let sa = render(&mut a, &mut ha, buffers, &mut ea);
    let sb = render(&mut b, &mut hb, buffers, &mut eb);
    assert_eq!(first_mismatch(&sb, &sa), None);
    assert!(!ea.is_empty());
    assert_eq!(evs(&eb), evs(&ea));
    assert_eq!(hb.session.latest(), None);
}

#[test]
fn stepped_ramp_changes_on_the_one() {
    let p = Pattern::default();
    let mut s = sstage(Span::Bars(8));
    s.tempo = Some(Tempo {
        jump: None,
        ramp: Some(Ramp {
            change: 10,
            every: Span::Bars(4),
            limit: None,
            smooth: false,
        }),
    });
    let (mut e, mut h) = session_engine(&p, 120, splan(vec![s], OnFinish::Continue));
    let end = 16 * SR as u64;
    let mut events = Vec::new();
    render(&mut e, &mut h, (end as usize).div_ceil(BUF), &mut events);
    let got = evs(&events);

    // Bar 4's "one" is where 120 BPM puts it; from there on, 130 BPM.
    let one4 = 4 * BAR;
    let before: Vec<Ev> = got.iter().copied().filter(|e| e.0 < one4).collect();
    assert_eq!(before, hit_evs(&timeline_hits(&p, 120, one4)));
    let mut tl = Timeline::new(SR, Bpm::new(130), &p, one4);
    let mut want = Vec::new();
    tl.events_in(&p, one4, end, &mut want);
    let after: Vec<Ev> = got
        .iter()
        .copied()
        .filter(|e| e.0 >= one4 && e.0 < end)
        .collect();
    assert_eq!(after[0], (one4, p.pulses[0].id, 0, false));
    assert_eq!(after, hit_evs(&want));
    // One 130 BPM beat is 22153.8 frames.
    assert!(matches!(after[2].0 - one4, 22_153 | 22_154), "{after:?}");
}

#[test]
fn silent_bar_has_events_marked_muted_and_no_sound() {
    let p = Pattern::default();
    let mut s = sstage(Span::Bars(100));
    s.mute = Some(MuteCycle {
        play: Span::Bars(1),
        mute: Span::Bars(1),
    });
    let (mut e, mut h) = session_engine(&p, 120, splan(vec![s], OnFinish::Stop));
    let mut events = Vec::new();
    let samples = render(
        &mut e,
        &mut h,
        (3 * BAR as usize).div_ceil(BUF),
        &mut events,
    );
    let got = evs(&events);
    let bar = |k: u64| -> Vec<Ev> {
        got.iter()
            .copied()
            .filter(|e| e.0 >= k * BAR && e.0 < (k + 1) * BAR)
            .collect()
    };
    let want = hit_evs(&timeline_hits(&p, 120, 3 * BAR));
    let in_range = got.iter().filter(|e| e.0 < 3 * BAR).count();
    assert_eq!(in_range, want.len(), "muted beats still come out as events");
    assert!(!bar(0).is_empty() && bar(0).iter().all(|e| !e.3));
    assert!(!bar(1).is_empty() && bar(1).iter().all(|e| e.3));
    assert!(!bar(2).is_empty() && bar(2).iter().all(|e| !e.3));
    let quiet = (BAR + 3 * u64::from(SR) / 10) as usize..2 * BAR as usize;
    assert!(samples[quiet].iter().all(|&x| x == 0.0), "bar 1 is silent");
    assert!(samples[2 * BAR as usize + 1] != 0.0, "bar 2 sounds again");
    // The last buffer reaches the "one" of bar 3.
    let progress = h.session.latest().expect("a session ran");
    assert_eq!((progress.stage, progress.bars_in_stage), (1, 3));
    assert!(progress.moment.silent, "bar 3 is muted again");
}

#[test]
fn off_pulse_has_no_events() {
    let p = Pattern::default();
    let mut s = sstage(Span::Bars(100));
    s.pulses[1] = Some(PulseState::Off);
    let (mut e, mut h) = session_engine(&p, 120, splan(vec![s], OnFinish::Stop));
    let mut events = Vec::new();
    let got = render(&mut e, &mut h, 400, &mut events);
    assert!(events.iter().all(|ev| ev.pulse != p.pulses[1].id));
    assert!(events.iter().any(|ev| ev.pulse == p.pulses[0].id));
    // It sounds exactly like the pattern with that pulse disabled.
    let mut off = p.clone();
    off.set_enabled(off.pulses[1].id, false);
    let (mut r, mut hr) = engine(&off, 120);
    r.set_soft_clip(false);
    send(&mut hr, Command::Play);
    let mut ref_events = Vec::new();
    let want = render(&mut r, &mut hr, 400, &mut ref_events);
    assert_eq!(first_mismatch(&got, &want), None);
    assert_eq!(evs(&events), evs(&ref_events));
}

#[test]
fn pulse_one_off_still_counts_bars() {
    let p = Pattern::default();
    let mut first = sstage(Span::Bars(2));
    first.pulses[0] = Some(PulseState::Off);
    let mut second = sstage(Span::Bars(4));
    second.tempo = Some(Tempo {
        jump: Some(Jump::To(Bpm::new(60))),
        ramp: None,
    });
    let (mut e, mut h) = session_engine(&p, 120, splan(vec![first, second], OnFinish::Stop));
    let mut events = Vec::new();
    render(
        &mut e,
        &mut h,
        (3 * BAR as usize).div_ceil(BUF),
        &mut events,
    );
    let ones: Vec<u64> = events
        .iter()
        .filter(|ev| ev.pulse == p.pulses[0].id)
        .map(|ev| ev.frame)
        .collect();
    // Silent for two bars, then stage 2 at the third "one", at 60 BPM.
    assert_eq!(ones[..2], [2 * BAR, 2 * BAR + u64::from(SR)]);
    let pulse2: Vec<u64> = events
        .iter()
        .filter(|ev| ev.pulse == p.pulses[1].id && ev.frame < 2 * BAR)
        .map(|ev| ev.frame)
        .collect();
    assert_eq!(pulse2, [0, 32_000, 64_000, 96_000, 128_000, 160_000]);
    let progress = h.session.latest().unwrap();
    assert_eq!((progress.stage, progress.moment.bpm.get()), (2, 60));
}

#[test]
fn random_mute_is_per_instant() {
    let p = Pattern::default();
    let random = |percent| {
        let mut s = sstage(Span::Bars(100));
        s.random_mute = Some(RandomMute {
            percent,
            increase: 0,
            every_bars: 1,
            max: 100,
        });
        splan(vec![s], OnFinish::Stop)
    };
    let (mut e, mut h) = session_engine(&p, 120, random(100));
    let mut events = Vec::new();
    let got = render(&mut e, &mut h, 400, &mut events);
    assert!(!events.is_empty() && events.iter().all(|ev| ev.muted));
    assert!(got.iter().all(|&x| x == 0.0));

    let (mut e, mut h) = session_engine(&p, 120, random(50));
    let mut events = Vec::new();
    render(
        &mut e,
        &mut h,
        (20 * SR as usize).div_ceil(BUF),
        &mut events,
    );
    let mut shared = 0;
    for group in events.chunk_by(|a, b| a.frame == b.frame) {
        if group.len() > 1 {
            shared += 1;
            assert!(
                group.iter().all(|ev| ev.muted == group[0].muted),
                "{group:?}"
            );
        }
    }
    assert!(shared >= 10, "the 4:3 ones are shared instants: {shared}");
    let muted = events.iter().filter(|ev| ev.muted).count();
    assert!(
        muted > 0 && muted < events.len(),
        "{muted} of {}",
        events.len()
    );
}

#[test]
fn finish_stop_rings_chime_then_stops() {
    let p = Pattern::default();
    let (mut e, mut h) =
        session_engine(&p, 120, splan(vec![sstage(Span::Bars(2))], OnFinish::Stop));
    let mut events = Vec::new();
    let got = render(
        &mut e,
        &mut h,
        (3 * BAR as usize).div_ceil(BUF),
        &mut events,
    );
    let finish = 2 * BAR;
    assert!(!h.position.snapshot().playing);
    let progress = h.session.latest().unwrap();
    assert!(progress.finished);
    assert!(events.iter().all(|ev| ev.frame < finish));
    assert_eq!(evs(&events), hit_evs(&timeline_hits(&p, 120, finish)));
    // Bar 1's clicks are over by then: what sounds is the chime, alone.
    let chime = end_chime(SR);
    let at = finish as usize;
    assert_eq!(first_mismatch(&got[at..at + chime.len()], &chime), None);
    assert!(got[at + 1] != 0.0);
    assert!(got[at + chime.len()..].iter().all(|&x| x == 0.0));
}

#[test]
fn session_render_never_allocates() {
    let mut p = Pattern::default();
    let extra = p.add_pulse().unwrap();
    p.set_enabled(extra, false);
    let mut ramp = sstage(Span::Bars(2));
    ramp.tempo = Some(Tempo {
        jump: None,
        ramp: Some(Ramp {
            change: 2,
            every: Span::Bars(1),
            limit: Some(Bpm::new(140)),
            smooth: true,
        }),
    });
    let mut mute = sstage(Span::Seconds(4));
    mute.mute = Some(MuteCycle {
        play: Span::Bars(1),
        mute: Span::Bars(1),
    });
    mute.random_mute = Some(RandomMute {
        percent: 30,
        increase: 10,
        every_bars: 1,
        max: 80,
    });
    let mut pulses = sstage(Span::Bars(2));
    pulses.tempo = Some(Tempo {
        jump: Some(Jump::By(-10)),
        ramp: None,
    });
    pulses.pulses[0] = Some(PulseState::Muted);
    pulses.pulses[1] = Some(PulseState::Off);
    pulses.pulses[2] = Some(PulseState::On);
    let plan = splan(vec![ramp, mute, pulses], OnFinish::Repeat);
    let mut toggled = p.clone();
    toggled.set_enabled(toggled.pulses[1].id, false);
    let mut edit = Some(update(&toggled));
    let mut next_plan = Some(Box::new(splan(vec![sstage(Span::Bars(1))], OnFinish::Stop)));

    let (mut e, mut h) = session_engine(&p, 120, plan);
    let mut out = [0.0f32; BUF];
    let (mut events, mut muted) = (0usize, 0usize);
    let mut stages = [false; 4];
    assert_no_alloc::assert_no_alloc(|| {
        for i in 0..(20 * SR as usize / BUF) {
            match i {
                100 => send(&mut h, Command::SetBpm(Bpm::new(125))),
                300 => send(&mut h, Command::SetPattern(edit.take().unwrap())),
                400 => send(&mut h, Command::Session(next_plan.take())),
                _ => {}
            }
            e.render(&mut out, Duration::from_millis(5));
            while let Ok(ev) = h.events.pop() {
                events += 1;
                muted += usize::from(ev.muted);
            }
            if let Some(progress) = h.session.latest() {
                stages[progress.stage as usize] = true;
            }
            let _ = h.position.snapshot();
        }
        send(&mut h, Command::Stop);
        send(&mut h, Command::Play);
        e.render(&mut out, Duration::ZERO);
    });
    assert!(events > 0 && muted > 0);
    assert_eq!(stages, [false, true, true, true], "every stage ran");
    // The first plan, replaced by the one queued mid-run, came back to be
    // dropped here, as did the pattern the edit replaced.
    assert_eq!(h.session_garbage.pop().unwrap().stages.len(), 3);
    assert!(h.garbage.pop().is_ok());
    assert_eq!(h.session.latest().unwrap().stages, 1);
}
