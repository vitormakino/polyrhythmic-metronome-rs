//! Rebuilding the stream must not lose the music: a new engine built from
//! what the service recorded continues at the old engine's position, also on
//! a device with another sample rate. The device smoke test is `#[ignore]`d:
//! it plays about 2 s of clicks on the default output.

use std::time::{Duration, Instant};

use pm_audio::{AudioService, Command, Engine, EngineHandle, EngineInit, Transport};
use pm_core::{Bpm, Pattern, PulseId};

const SR: u32 = 48_000;
const BUF: usize = 512;

type Hit = (u64, PulseId, u32);

fn init(pattern: &Pattern, bpm: i32) -> EngineInit {
    EngineInit {
        pattern: pattern.clone(),
        bpm: Bpm::new(bpm),
        resume: None,
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
