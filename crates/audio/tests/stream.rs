//! Rebuilding the stream must not lose the music: a new engine built from
//! what the service recorded continues at the old engine's position, also on
//! a device with another sample rate. The device smoke test is `#[ignore]`d:
//! it plays about 2 s of clicks on the default output.

use std::time::{Duration, Instant};

use pm_audio::{AudioService, Command, Engine, EngineHandle, EngineInit, Transport};
use pm_core::session::{Jump, OnFinish, Ramp, SessionPlan, Span, Stage, Tempo};
use pm_core::{Bpm, MAX_PULSES, MusicalPosition, Pattern, PulseId};

const SR: u32 = 48_000;
const BUF: usize = 512;

type Hit = (u64, PulseId, u32);

fn init(pattern: &Pattern, bpm: i32) -> EngineInit {
    EngineInit {
        pattern: pattern.clone(),
        bpm: Bpm::new(bpm),
        resume: None,
        session: None,
    }
}

/// Records `c` in `t` and queues it for `h`, as `AudioService::send` does.
fn send(t: &mut Transport, h: &mut EngineHandle, c: Command) {
    t.record(&c);
    assert!(h.commands.push(c).is_ok(), "command queue full");
}

/// Renders `buffers` mono buffers of `BUF` frames and returns the hits.
fn render(e: &mut Engine, h: &mut EngineHandle, buffers: usize) -> Vec<Hit> {
    let mut out = [0.0f32; BUF];
    let mut hits = Vec::new();
    for _ in 0..buffers {
        e.render(&mut out, Duration::ZERO);
        while let Ok(ev) = h.events.pop() {
            hits.push((ev.frame, ev.pulse, ev.beat));
        }
    }
    hits
}

/// Engine A playing the default pattern for about 1.3 s, with a tempo change
/// on the way, and the transport that recorded its commands.
fn played_for_1_3_s() -> (Pattern, Transport, Engine, EngineHandle) {
    let p = Pattern::default();
    let first = init(&p, 120);
    let mut t = Transport::new(&first);
    let (mut a, mut ha) = Engine::new(SR, 1, first);
    send(&mut t, &mut ha, Command::Play);
    render(&mut a, &mut ha, 50);
    send(&mut t, &mut ha, Command::SetBpm(Bpm::new(137)));
    // 122 buffers in all: 62464 frames, 1.301 s.
    render(&mut a, &mut ha, 72);
    (p, t, a, ha)
}

#[test]
fn resume_position_survives_rebuild() {
    let (p, t, mut a, mut ha) = played_for_1_3_s();
    let snap = ha.position.snapshot();
    assert_eq!(snap.frame, 62_464);
    assert!(snap.playing);

    let rebuilt = t.resume_init(snap);
    assert_eq!(rebuilt.bpm, Bpm::new(137));
    assert_eq!(rebuilt.resume, Some(snap.position));
    let (mut b, mut hb) = Engine::new(SR, 1, rebuilt);
    let got = render(&mut b, &mut hb, 400);

    // What A would have played over the same frames, relative to the seam.
    let want: Vec<Hit> = render(&mut a, &mut ha, 400)
        .into_iter()
        .map(|(f, pulse, beat)| (f - snap.frame, pulse, beat))
        .collect();
    assert!(want.len() > 12, "A keeps playing: {}", want.len());
    assert_eq!(got, want);

    // B's next "one" is exactly where A's would have been.
    let one = |hits: &[Hit]| {
        hits.iter()
            .find(|&&(_, pulse, beat)| pulse == p.pulses[0].id && beat == 0)
            .copied()
    };
    let next_one = one(&got).expect("B reaches a one");
    assert_eq!(Some(next_one), one(&want));
    assert!(next_one.0 > 0, "the one is ahead of the seam");
}

#[test]
fn resume_across_sample_rate_change() {
    const NEW_SR: u64 = 16_000;
    let (p, t, mut a, mut ha) = played_for_1_3_s();
    let snap = ha.position.snapshot();
    let (mut b, mut hb) = Engine::new(NEW_SR as u32, 2, t.resume_init(snap));
    let mut out = [0.0f32; 2 * BUF];
    let mut got = Vec::new();
    for _ in 0..140 {
        b.render(&mut out, Duration::ZERO);
        while let Ok(ev) = hb.events.pop() {
            got.push((ev.frame, ev.pulse, ev.beat));
        }
    }
    // 140 stereo buffers at 16 kHz are 4.48 s; A covers the same time.
    let want: Vec<Hit> = render(&mut a, &mut ha, 420)
        .into_iter()
        .map(|(f, pulse, beat)| (f - snap.frame, pulse, beat))
        .collect();
    let want: Vec<Hit> = want
        .into_iter()
        .filter(|&(f, _, _)| f * NEW_SR / u64::from(SR) < 140 * BUF as u64)
        .collect();
    assert!(want.len() > 12, "A keeps playing: {}", want.len());
    assert_eq!(got.len(), want.len(), "got {got:?}\nwant {want:?}");
    for (g, w) in got.iter().zip(&want) {
        assert_eq!((g.1, g.2), (w.1, w.2), "same beats in the same order");
        let expected = w.0 * NEW_SR / u64::from(SR);
        assert!(
            g.0.abs_diff(expected) <= 1,
            "hit {g:?} vs expected frame {expected} (old {w:?})"
        );
    }
    let first_one = got
        .iter()
        .find(|&&(_, pulse, beat)| pulse == p.pulses[0].id && beat == 0)
        .expect("B reaches a one");
    assert!(first_one.0 > 0);
}

#[test]
fn rebuild_keeps_the_transport_state() {
    let p = Pattern::default();
    let first = init(&p, 120);
    let mut t = Transport::new(&first);
    let (a, mut ha) = Engine::new(SR, 1, first);
    drop(a);

    // Never played: the new engine waits for Play.
    assert_eq!(t.resume_init(ha.position.snapshot()).resume, None);

    // Play sent but not rendered yet: the new engine starts at the one.
    send(&mut t, &mut ha, Command::Play);
    let (mut b, mut hb) = Engine::new(SR, 1, t.resume_init(ha.position.snapshot()));
    let got = render(&mut b, &mut hb, 1);
    assert_eq!(
        got,
        vec![(0, p.pulses[0].id, 0), (0, p.pulses[1].id, 0)],
        "plays from the one"
    );

    // Stopped again: the new engine is stopped, with the last pattern sent.
    let mut edited = p.clone();
    edited.set_beats(edited.pulses[1].id, 5);
    send(&mut t, &mut ha, Command::Stop);
    send(
        &mut t,
        &mut ha,
        Command::SetPattern(pm_audio::PatternUpdate::build(SR, edited.clone())),
    );
    let rebuilt = t.resume_init(ha.position.snapshot());
    assert_eq!(rebuilt.resume, None);
    assert_eq!(rebuilt.pattern, edited);
}

/// A two-stage session: 2 bars at the base tempo, then 8 bars from 150 BPM
/// up 3 BPM a bar.
fn two_stage_plan() -> SessionPlan {
    let stage = |length| Stage {
        length,
        tempo: None,
        mute: None,
        random_mute: None,
        pulses: [None; MAX_PULSES],
    };
    let ramp = Stage {
        tempo: Some(Tempo {
            jump: Some(Jump::To(Bpm::new(150))),
            ramp: Some(Ramp {
                change: 3,
                every: Span::Bars(1),
                limit: None,
                smooth: false,
            }),
        }),
        ..stage(Span::Bars(8))
    };
    SessionPlan {
        name: "two".into(),
        stages: vec![stage(Span::Bars(2)), ramp],
        on_finish: OnFinish::Stop,
        show_muted_visuals: false,
        seed: 1,
    }
}

#[test]
fn rebuild_mid_session_continues_the_stage() {
    const NEW_SR: u64 = 44_100;
    let p = Pattern::default();
    let first = init(&p, 120);
    let mut t = Transport::new(&first);
    let (mut a, mut ha) = Engine::new(SR, 1, first);
    send(
        &mut t,
        &mut ha,
        Command::Session(Some(Box::new(two_stage_plan()))),
    );
    send(&mut t, &mut ha, Command::Play);
    // Stage 2 starts at 4 s (frame 192000); 376320 frames is mid-bar 2 of it.
    let old = render(&mut a, &mut ha, 735);
    let snap = ha.position.snapshot();
    let before = ha.session.latest().expect("the session runs");
    assert_eq!((before.stage, before.bars_in_stage), (2, 2));
    assert_eq!(before.moment.bpm, Bpm::new(156));

    let rebuilt = t.resume_init_with(snap, &ha.session);
    assert_eq!(rebuilt.bpm, Bpm::new(156), "the tempo the session plays");
    let (mut b, mut hb) = Engine::new(NEW_SR as u32, 1, rebuilt);
    let resumed = hb.session.latest().expect("published when built");
    assert_eq!(
        (resumed.stage, resumed.bars_in_stage, resumed.moment.bpm),
        (before.stage, before.bars_in_stage, before.moment.bpm)
    );

    let got = render(&mut b, &mut hb, 600);
    let want: Vec<Hit> = render(&mut a, &mut ha, 650)
        .into_iter()
        .map(|(f, pulse, beat)| (f - snap.frame, pulse, beat))
        .filter(|&(f, _, _)| f * NEW_SR / u64::from(SR) < 590 * BUF as u64)
        .collect();
    let reference = |h: &&Hit| h.1 == p.pulses[0].id;
    let last_old = old.iter().rev().find(reference).unwrap();
    let next_new = got.iter().find(reference).unwrap();
    assert_eq!(next_new.2, (last_old.2 + 1) % 4, "none repeated, none lost");
    let gap = (snap.frame - last_old.0) as f64 / f64::from(SR) + next_new.0 as f64 / NEW_SR as f64;
    assert!(
        (gap - 60.0 / 156.0).abs() < 2.0 / NEW_SR as f64,
        "one beat after the last old one: {gap}"
    );
    // B goes on with the stage exactly as A does, ramp steps included.
    assert!(want.len() > 25, "{}", want.len());
    assert!(got.len() >= want.len(), "{} < {}", got.len(), want.len());
    for (g, w) in got.iter().zip(&want) {
        assert_eq!((g.1, g.2), (w.1, w.2), "same beats in the same order");
        let expected = w.0 * NEW_SR / u64::from(SR);
        assert!(
            g.0.abs_diff(expected) <= 2,
            "hit {g:?} vs {expected} ({w:?})"
        );
    }
    let (pa, pb) = (ha.session.latest().unwrap(), hb.session.latest().unwrap());
    assert_eq!((pb.stage, pb.bars_in_stage), (pa.stage, pa.bars_in_stage));
    assert_eq!(pb.moment.bpm, pa.moment.bpm);
    assert!(pa.moment.bpm > Bpm::new(156), "the ramp moved on");
}

#[test]
fn rebuild_keeps_the_running_plan_and_the_one_queued() {
    let p = Pattern::default();
    let first = init(&p, 120);
    let mut t = Transport::new(&first);
    let (mut a, mut ha) = Engine::new(SR, 1, first);
    let mut next = two_stage_plan();
    next.stages.truncate(1);
    send(
        &mut t,
        &mut ha,
        Command::Session(Some(Box::new(two_stage_plan()))),
    );
    send(&mut t, &mut ha, Command::Play);
    render(&mut a, &mut ha, 10);
    // Sent while the session runs: it waits for the next Play.
    send(&mut t, &mut ha, Command::Session(Some(Box::new(next))));
    render(&mut a, &mut ha, 10);
    assert_eq!(ha.session.latest().unwrap().stages, 2);

    let rebuilt = t.resume_init_with(ha.position.snapshot(), &ha.session);
    let (plan, progress) = rebuilt.session.as_ref().unwrap();
    assert_eq!((plan.stages.len(), progress.unwrap().stage), (2, 1));
    let (mut b, mut hb) = Engine::new(SR, 1, rebuilt);
    let queued = t.queued_session().expect("the queued plan");
    assert!(hb.commands.push(queued).is_ok());
    render(&mut b, &mut hb, 10);
    assert_eq!(
        hb.session.latest().unwrap().stages,
        2,
        "still the running plan"
    );
    send(&mut t, &mut hb, Command::Stop);
    send(&mut t, &mut hb, Command::Play);
    render(&mut b, &mut hb, 1);
    assert_eq!(hb.session.latest().unwrap().stages, 1, "the queued plan");
    assert_eq!(hb.session_garbage.pop().unwrap().stages.len(), 2);
    assert!(t.queued_session().is_none());
}

#[test]
fn rebuild_after_a_session_stopped_itself_stays_stopped() {
    let p = Pattern::default();
    let first = init(&p, 120);
    let mut t = Transport::new(&first);
    let (mut a, mut ha) = Engine::new(SR, 1, first);
    let mut short = two_stage_plan();
    short.stages.truncate(1);
    send(&mut t, &mut ha, Command::Session(Some(Box::new(short))));
    send(&mut t, &mut ha, Command::Play);
    // One stage of 2 bars: it finishes at 4 s.
    render(&mut a, &mut ha, 400);
    let snap = ha.position.snapshot();
    assert!(!snap.playing && ha.session.latest().unwrap().finished);

    let rebuilt = t.resume_init_with(snap, &ha.session);
    assert_eq!(rebuilt.resume, None, "the UI's Play is not replayed");
    let (plan, progress) = rebuilt.session.unwrap();
    assert_eq!((plan.stages.len(), progress), (1, None));
}

#[test]
fn rebuild_keeps_a_play_sent_after_a_session_stopped_itself() {
    let p = Pattern::default();
    let first = init(&p, 120);
    let mut t = Transport::new(&first);
    let (mut a, mut ha) = Engine::new(SR, 1, first);
    let mut short = two_stage_plan();
    short.stages.truncate(1);
    send(&mut t, &mut ha, Command::Session(Some(Box::new(short))));
    send(&mut t, &mut ha, Command::Play);
    render(&mut a, &mut ha, 400);
    assert!(ha.session.latest().unwrap().finished);
    // The UI sees `finished`, sends Stop, then plays the session again with
    // another plan; the stream is rebuilt before the engine takes the Play.
    send(&mut t, &mut ha, Command::Stop);
    send(
        &mut t,
        &mut ha,
        Command::Session(Some(Box::new(two_stage_plan()))),
    );
    send(&mut t, &mut ha, Command::Play);
    let snap = ha.position.snapshot();
    assert!(
        !snap.playing && ha.session.latest().unwrap().finished,
        "stale"
    );

    let rebuilt = t.resume_init_with(snap, &ha.session);
    assert_eq!(
        rebuilt.resume,
        Some(MusicalPosition::start_of(0)),
        "Play kept"
    );
    let (plan, progress) = rebuilt.session.as_ref().unwrap();
    assert_eq!((plan.stages.len(), *progress), (2, None));
    t.engine_replaced();
    let (mut b, mut hb) = Engine::new(SR, 1, rebuilt);
    let got = render(&mut b, &mut hb, 1);
    assert_eq!(got[..2], [(0, p.pulses[0].id, 0), (0, p.pulses[1].id, 0)]);
    let progress = hb.session.latest().expect("the new run");
    assert_eq!(
        (progress.stage, progress.stages, progress.finished),
        (1, 2, false)
    );

    // Once the new engine has taken every Play, a finish is a finish again.
    let mut short = two_stage_plan();
    short.stages.truncate(1);
    send(&mut t, &mut hb, Command::Stop);
    send(&mut t, &mut hb, Command::Session(Some(Box::new(short))));
    send(&mut t, &mut hb, Command::Play);
    render(&mut b, &mut hb, 400);
    let snap = hb.position.snapshot();
    assert!(!snap.playing && hb.session.latest().unwrap().finished);
    assert_eq!(t.resume_init_with(snap, &hb.session).resume, None);
}

#[test]
fn progress_of_a_run_not_started_yet_is_hidden() {
    let p = Pattern::default();
    let first = init(&p, 120);
    let mut t = Transport::new(&first);
    let (mut a, mut ha) = Engine::new(SR, 1, first);
    let short = || {
        let mut plan = two_stage_plan();
        plan.stages.truncate(1);
        Box::new(plan)
    };
    assert_eq!(t.session_progress(&ha.session), None, "nothing ran yet");
    send(&mut t, &mut ha, Command::Session(Some(short())));
    send(&mut t, &mut ha, Command::Play);
    assert_eq!(t.session_progress(&ha.session), None, "Play not taken");
    render(&mut a, &mut ha, 400);
    let done = t.session_progress(&ha.session).expect("the finished run");
    assert!(done.finished);

    // Played again: until the engine takes the Play, the finished progress
    // in the cell is the previous run's.
    send(&mut t, &mut ha, Command::Stop);
    send(&mut t, &mut ha, Command::Session(Some(short())));
    send(&mut t, &mut ha, Command::Play);
    assert!(ha.session.latest().unwrap().finished, "stale in the cell");
    assert_eq!(t.session_progress(&ha.session), None);
    render(&mut a, &mut ha, 1);
    let fresh = t.session_progress(&ha.session).expect("the new run");
    assert!(!fresh.finished);

    // A rebuilt engine counts its own Plays.
    let snap = ha.position.snapshot();
    let rebuilt = t.resume_init_with(snap, &ha.session);
    t.engine_replaced();
    let (mut b, mut hb) = Engine::new(SR, 1, rebuilt);
    render(&mut b, &mut hb, 1);
    assert!(t.session_progress(&hb.session).is_some());
    send(&mut t, &mut hb, Command::Stop);
    send(&mut t, &mut hb, Command::Play);
    assert_eq!(t.session_progress(&hb.session), None);
}

#[test]
#[ignore = "plays about 2 s of clicks on the default output device"]
fn real_device_smoke() {
    let mut service = AudioService::start(init(&Pattern::default(), 120)).expect("audio service");
    let info = service.device_info().expect("an output device");
    println!(
        "device {:?} ({}) at {} Hz, latency {:?}",
        info.name, info.uid, info.sample_rate, info.latency
    );
    assert!(service.send(Command::Play).is_ok());
    let started = Instant::now();
    let mut events = 0;
    while started.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(250));
        service.tick();
        while service.handle().events.pop().is_ok() {
            events += 1;
        }
    }
    let snap = service.handle().position.snapshot();
    let underruns = service
        .handle()
        .underruns
        .load(std::sync::atomic::Ordering::Relaxed);
    println!(
        "frames {}, events {events}, underruns {underruns}",
        snap.frame
    );
    assert!(snap.playing);
    assert!(
        snap.frame > u64::from(info.sample_rate) * 3 / 2,
        "the stream ran: {} frames",
        snap.frame
    );
    assert!(events >= 6, "clicks were scheduled: {events}");
    assert_eq!(underruns, 0);
}
