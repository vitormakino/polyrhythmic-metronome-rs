//! Behavior of the practice-session runner, driven bar by bar at 48 kHz with
//! bars 2 s apart (4 beats at 120 BPM).

use pm_core::session::{
    Jump, Moment, MuteCycle, OnFinish, PulseMask, PulseState, Ramp, RandomMute, Runner,
    SessionPlan, Span, Stage, Tempo,
};
use pm_core::{Bpm, MAX_PULSES};

const SR: u32 = 48_000;
const BAR: u64 = 2 * SR as u64;

fn stage(length: Span) -> Stage {
    Stage {
        length,
        tempo: None,
        mute: None,
        random_mute: None,
        pulses: [None; MAX_PULSES],
    }
}

fn ramp_stage(length: Span, change: i32, every: Span, limit: Option<u16>, smooth: bool) -> Stage {
    Stage {
        tempo: Some(Tempo {
            jump: None,
            ramp: Some(Ramp {
                change,
                every,
                limit: limit.map(|l| Bpm::new(i32::from(l))),
                smooth,
            }),
        }),
        ..stage(length)
    }
}

fn plan(stages: Vec<Stage>, on_finish: OnFinish) -> SessionPlan {
    SessionPlan {
        name: "t".into(),
        stages,
        on_finish,
        show_muted_visuals: false,
        seed: 7,
    }
}

fn run(plan: &SessionPlan, base: u16, bars: usize) -> Vec<Moment> {
    let mut r = Runner::start(plan, Bpm::new(i32::from(base)), SR, 0);
    (0..bars).map(|i| r.bar(plan, i as u64 * BAR).0).collect()
}

fn bpms(m: &[Moment]) -> Vec<u16> {
    m.iter().map(|m| m.bpm.get()).collect()
}

#[test]
fn jump_by_and_to_set_stage_start() {
    let mut s = stage(Span::Bars(4));
    s.tempo = Some(Tempo {
        jump: Some(Jump::By(10)),
        ramp: None,
    });
    let p = plan(vec![s], OnFinish::Stop);
    assert_eq!(run(&p, 100, 1)[0].bpm.get(), 110);

    let mut s = stage(Span::Bars(4));
    s.tempo = Some(Tempo {
        jump: Some(Jump::To(Bpm::new(60))),
        ramp: None,
    });
    let p = plan(vec![s], OnFinish::Stop);
    assert_eq!(run(&p, 100, 1)[0].bpm.get(), 60);
}

#[test]
fn stepped_ramp_with_max() {
    let p = plan(
        vec![ramp_stage(
            Span::Bars(1000),
            2,
            Span::Bars(4),
            Some(106),
            false,
        )],
        OnFinish::Stop,
    );
    let got = bpms(&run(&p, 100, 17));
    let mut want = Vec::new();
    for b in [100, 102, 104, 106] {
        want.extend([b; 4]);
    }
    want.push(106);
    assert_eq!(got, want);
}

#[test]
fn ramp_without_limit_down() {
    let p = plan(
        vec![ramp_stage(Span::Bars(1000), -5, Span::Bars(1), None, false)],
        OnFinish::Stop,
    );
    assert_eq!(bpms(&run(&p, 100, 4)), [100, 95, 90, 85]);
}

#[test]
fn stepped_ramp_in_seconds() {
    let p = plan(
        vec![ramp_stage(
            Span::Bars(1000),
            10,
            Span::Seconds(4),
            None,
            false,
        )],
        OnFinish::Stop,
    );
    assert_eq!(bpms(&run(&p, 100, 4)), [100, 100, 110, 110]);
}

#[test]
fn smooth_ramp_spreads_whole_steps() {
    let p = plan(
        vec![ramp_stage(Span::Bars(1000), 2, Span::Bars(4), None, true)],
        OnFinish::Stop,
    );
    let mut r = Runner::start(&p, Bpm::new(100), SR, 0);
    let mut seen = Vec::new();
    for bar in 0..=4u64 {
        seen.push(r.bar(&p, bar * BAR).0.bpm.get());
        for beat in 1..4u32 {
            let f = bar * BAR + u64::from(beat) * BAR / 4;
            seen.push(r.beat(&p, f, beat, 4).bpm.get());
        }
    }
    assert!(
        seen.windows(2).all(|w| w[1] >= w[0] && w[1] - w[0] <= 1),
        "{seen:?}"
    );
    assert_eq!(seen[16], 102, "bar 4 beat 0: {seen:?}");
    assert_eq!(seen[0], 100);
}

#[test]
fn mute_cycle_bars() {
    let mut s = stage(Span::Bars(100));
    s.mute = Some(MuteCycle {
        play: Span::Bars(2),
        mute: Span::Bars(1),
    });
    let p = plan(vec![s], OnFinish::Stop);
    let got: Vec<bool> = run(&p, 100, 6).iter().map(|m| m.silent).collect();
    assert_eq!(got, [false, false, true, false, false, true]);
}

#[test]
fn mute_cycle_seconds() {
    let mut s = stage(Span::Bars(100));
    s.mute = Some(MuteCycle {
        play: Span::Seconds(2),
        mute: Span::Seconds(2),
    });
    let p = plan(vec![s], OnFinish::Stop);
    let got: Vec<bool> = run(&p, 100, 4).iter().map(|m| m.silent).collect();
    assert_eq!(got, [false, true, false, true]);
}

#[test]
fn random_percent_grows_to_max() {
    let mut s = stage(Span::Bars(100));
    s.random_mute = Some(RandomMute {
        percent: 20,
        increase: 5,
        every_bars: 4,
        max: 30,
    });
    let p = plan(vec![s], OnFinish::Stop);
    let m = run(&p, 100, 13);
    let got: Vec<u8> = [0, 4, 8, 12].iter().map(|&i| m[i].random_percent).collect();
    assert_eq!(got, [20, 25, 30, 30]);
    // No random mute configured: the percentage is zero.
    let p = plan(vec![stage(Span::Bars(10))], OnFinish::Stop);
    assert_eq!(run(&p, 100, 1)[0].random_percent, 0);
}

#[test]
fn draw_rate_near_target() {
    let p = plan(vec![stage(Span::Bars(10))], OnFinish::Stop);
    let draws = |seed_plan: &SessionPlan| {
        let mut r = Runner::start(seed_plan, Bpm::new(100), SR, 0);
        (0..10_000).map(|_| r.draw(25)).collect::<Vec<_>>()
    };
    let a = draws(&p);
    let hits = a.iter().filter(|&&x| x).count();
    assert!((2_300..=2_700).contains(&hits), "{hits}");
    assert_eq!(a, draws(&p), "same seed, same sequence");
    let mut r = Runner::start(&p, Bpm::new(100), SR, 0);
    assert!((0..100).all(|_| !r.draw(0)));
    assert!((0..100).all(|_| r.draw(100)));
}

#[test]
fn pulse_states_map_to_mask_and_reset_next_stage() {
    let mut s1 = stage(Span::Bars(1));
    s1.pulses[0] = Some(PulseState::On);
    s1.pulses[1] = Some(PulseState::Off);
    s1.pulses[2] = Some(PulseState::Muted);
    let p = plan(vec![s1, stage(Span::Bars(5))], OnFinish::Stop);
    let m = run(&p, 100, 2);
    assert_eq!(
        m[0].pulses,
        PulseMask {
            off: 0b010,
            muted: 0b100,
            forced_on: 0b001
        }
    );
    assert_eq!(m[1].pulses, PulseMask::default());
}

#[test]
fn seconds_stage_ends_on_next_one() {
    let mut s1 = stage(Span::Seconds(3));
    s1.tempo = Some(Tempo {
        jump: Some(Jump::By(7)),
        ramp: None,
    });
    let p = plan(vec![stage(Span::Seconds(3)), s1.clone()], OnFinish::Stop);
    let mut r = Runner::start(&p, Bpm::new(100), SR, 0);
    let _ = r.bar(&p, 0);
    let _ = r.bar(&p, BAR);
    assert_eq!(r.progress().stage, 1, "2 s < 3 s: still stage 1");
    let (m, _) = r.bar(&p, 2 * BAR);
    let pr = r.progress();
    assert_eq!((pr.stage, pr.bars_in_stage, pr.frames_in_stage), (2, 0, 0));
    assert_eq!(m.bpm.get(), 107);
}

#[test]
fn user_nudge_shifts_ramp_but_limit_holds() {
    let p = plan(
        vec![ramp_stage(
            Span::Bars(1000),
            2,
            Span::Bars(1),
            Some(110),
            false,
        )],
        OnFinish::Stop,
    );
    let mut r = Runner::start(&p, Bpm::new(100), SR, 0);
    let mut got = Vec::new();
    for i in 0..6u64 {
        got.push(r.bar(&p, i * BAR).0.bpm.get());
        if i == 2 {
            r.user_bpm(Bpm::new(105));
        }
    }
    assert_eq!(got, [100, 102, 104, 107, 109, 110]);
    assert_eq!(r.progress().max_bpm.get(), 110);
}

#[test]
fn user_pulse_wins_until_stage_end() {
    let mut s1 = stage(Span::Bars(3));
    s1.pulses[1] = Some(PulseState::Off);
    let mut s2 = stage(Span::Bars(3));
    s2.pulses[1] = Some(PulseState::Off);
    let p = plan(vec![s1, s2], OnFinish::Stop);
    let mut r = Runner::start(&p, Bpm::new(100), SR, 0);
    assert_eq!(r.bar(&p, 0).0.pulses.off, 0b10);
    r.user_pulse(1);
    assert_eq!(r.bar(&p, BAR).0.pulses.off, 0);
    assert_eq!(r.bar(&p, 2 * BAR).0.pulses.off, 0);
    assert_eq!(
        r.bar(&p, 3 * BAR).0.pulses.off,
        0b10,
        "next stage applies it again"
    );
}

fn finish_plan(on_finish: OnFinish) -> SessionPlan {
    let mut s = ramp_stage(Span::Bars(2), 5, Span::Bars(1), None, false);
    s.mute = Some(MuteCycle {
        play: Span::Bars(1),
        mute: Span::Bars(1),
    });
    plan(vec![s], on_finish)
}

#[test]
fn on_finish_stop_continue_repeat() {
    // Stop.
    let p = finish_plan(OnFinish::Stop);
    let mut r = Runner::start(&p, Bpm::new(100), SR, 0);
    assert_eq!(r.bar(&p, 0).1, None);
    assert_eq!(r.bar(&p, BAR).1, None);
    assert_eq!(r.bar(&p, 2 * BAR).1, Some(true));
    assert!(r.progress().finished);

    // Continue: reported once, ramp frozen, mutes keep cycling.
    let p = finish_plan(OnFinish::Continue);
    let mut r = Runner::start(&p, Bpm::new(100), SR, 0);
    let mut out = Vec::new();
    for i in 0..6u64 {
        out.push(r.bar(&p, i * BAR));
    }
    let ends: Vec<Option<bool>> = out.iter().map(|o| o.1).collect();
    assert_eq!(ends, [None, None, Some(false), None, None, None]);
    assert_eq!(
        bpms(&out.iter().map(|o| o.0).collect::<Vec<_>>()),
        [100, 105, 105, 105, 105, 105]
    );
    let silent: Vec<bool> = out.iter().map(|o| o.0.silent).collect();
    assert_eq!(silent, [false, true, false, true, false, true]);
    assert!(r.progress().finished);

    // Repeat: back to stage 1 at the base tempo.
    let p = finish_plan(OnFinish::Repeat);
    let m = run(&p, 100, 6);
    assert_eq!(bpms(&m), [100, 105, 100, 105, 100, 105]);
}

#[test]
fn repeat_keeps_user_delta_in_the_base() {
    let p = finish_plan(OnFinish::Repeat);
    let mut r = Runner::start(&p, Bpm::new(100), SR, 0);
    r.bar(&p, 0);
    r.bar(&p, BAR);
    r.user_bpm(Bpm::new(115)); // +10 over the ramp's 105
    assert_eq!(r.bar(&p, 2 * BAR).0.bpm.get(), 110);
}

#[test]
fn resume_roundtrip() {
    let mut s2 = stage(Span::Bars(10));
    s2.tempo = Some(Tempo {
        jump: Some(Jump::By(5)),
        ramp: None,
    });
    let p = plan(
        vec![
            ramp_stage(Span::Seconds(3), 1, Span::Bars(1), None, false),
            s2,
        ],
        OnFinish::Stop,
    );
    let mut r = Runner::start(&p, Bpm::new(100), SR, 0);
    r.bar(&p, 0);
    r.bar(&p, BAR);
    let before = r.progress();
    assert_eq!(
        (before.stage, before.bars_in_stage, before.moment.bpm.get()),
        (1, 1, 101)
    );

    let f = 1_000_000;
    let mut r2 = Runner::resume(&before, 44_100, f);
    let after = r2.progress();
    assert_eq!((after.stage, after.bars_in_stage), (1, 1));
    assert_eq!(after.moment.bpm.get(), 101);
    assert_eq!(after.frames_in_stage, 88_200, "2 s at the new rate");

    // 4 s into the stage: Seconds(3) is over, at either rate.
    let (m, _) = r2.bar(&p, f + 88_200);
    assert_eq!((r2.progress().stage, m.bpm.get()), (2, 106));
    let (m1, _) = r.bar(&p, 2 * BAR);
    assert_eq!(m1.bpm.get(), 106);
}

#[test]
fn runner_is_copy_and_small() {
    fn is_copy<T: Copy>() {}
    is_copy::<Runner>();
    assert!(std::mem::size_of::<Runner>() < 512);
}

#[test]
fn user_bpm_below_a_reached_limit_holds_then_ramp_continues() {
    let p = plan(
        vec![ramp_stage(
            Span::Bars(1000),
            2,
            Span::Bars(4),
            Some(106),
            false,
        )],
        OnFinish::Stop,
    );
    let mut r = Runner::start(&p, Bpm::new(100), SR, 0);
    for i in 0..=40u64 {
        r.bar(&p, i * BAR);
    }
    assert_eq!(r.progress().moment.bpm.get(), 106);
    r.user_bpm(Bpm::new(100));
    let got: Vec<u16> = (41..=44u64)
        .map(|i| r.bar(&p, i * BAR).0.bpm.get())
        .collect();
    assert_eq!(
        got,
        [100, 100, 100, 102],
        "holds, then steps from the user's tempo"
    );
    let last = (45..=70u64).map(|i| r.bar(&p, i * BAR).0.bpm.get()).last();
    assert_eq!(last, Some(106), "the limit still clamps");
}

#[test]
fn user_bpm_below_a_reached_limit_holds_for_smooth_ramps() {
    let p = plan(
        vec![ramp_stage(
            Span::Bars(1000),
            2,
            Span::Bars(4),
            Some(106),
            true,
        )],
        OnFinish::Stop,
    );
    let mut r = Runner::start(&p, Bpm::new(100), SR, 0);
    for i in 0..=40u64 {
        r.bar(&p, i * BAR);
    }
    assert_eq!(r.progress().moment.bpm.get(), 106);
    r.user_bpm(Bpm::new(100));
    for beat in 1..4u32 {
        let f = 40 * BAR + u64::from(beat) * BAR / 4;
        assert_eq!(r.beat(&p, f, beat, 4).bpm.get(), 100);
    }
    assert_eq!(r.bar(&p, 41 * BAR).0.bpm.get(), 100);
    assert_eq!(r.bar(&p, 42 * BAR).0.bpm.get(), 101, "next whole step");
}
