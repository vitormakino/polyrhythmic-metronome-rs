//! Practice session files in `sessions/` of the config folder, plus the
//! Draft 4 JSON Schema that gives editors (Taplo) autocomplete for them.
//!
//! The app never writes a session file. `parse` turns one into a
//! [`SessionPlan`]; every rule it enforces is an error naming the line.

use pm_core::session::{
    Jump, MuteCycle, OnFinish, PulseState, Ramp, RandomMute, SessionPlan, Span, Stage, Tempo,
};
use pm_core::{Bpm, MAX_PULSES};
use serde::Deserialize;
use toml::Spanned;

const DRAFT_4: &str = "http://json-schema.org/draft-04/schema#";
const MAX_STAGES: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionFile {
    name: Option<String>,
    on_finish: Option<OnFinishFile>,
    show_muted_visuals: Option<bool>,
    stage: Option<Vec<Spanned<StageFile>>>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum OnFinishFile {
    Stop,
    Continue,
    Repeat,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StageFile {
    bars: Option<Spanned<i64>>,
    seconds: Option<Spanned<i64>>,
    tempo: Option<Spanned<TempoFile>>,
    mute: Option<Spanned<MuteFile>>,
    random_mute: Option<Spanned<RandomMuteFile>>,
    pulses: Option<Vec<Spanned<PulseFile>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TempoFile {
    jump: Option<Spanned<i64>>,
    set: Option<Spanned<i64>>,
    change: Option<Spanned<i64>>,
    every_bars: Option<Spanned<i64>>,
    every_seconds: Option<Spanned<i64>>,
    max: Option<Spanned<i64>>,
    min: Option<Spanned<i64>>,
    smooth: Option<Spanned<bool>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MuteFile {
    play_bars: Option<Spanned<i64>>,
    mute_bars: Option<Spanned<i64>>,
    play_seconds: Option<Spanned<i64>>,
    mute_seconds: Option<Spanned<i64>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RandomMuteFile {
    percent: Option<Spanned<i64>>,
    increase: Option<Spanned<i64>>,
    every_bars: Option<Spanned<i64>>,
    max: Option<Spanned<i64>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PulseFile {
    pulse: Option<Spanned<i64>>,
    state: Option<PulseStateFile>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum PulseStateFile {
    Off,
    Muted,
    On,
}

/// Source text with the means to name a line for an error.
struct Source<'a>(&'a str);

impl Source<'_> {
    /// 1-based line at a byte offset.
    fn line(&self, offset: usize) -> usize {
        let end = offset.min(self.0.len());
        self.0.as_bytes()[..end]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1
    }

    fn err<T>(
        &self,
        span: std::ops::Range<usize>,
        msg: impl std::fmt::Display,
    ) -> Result<T, String> {
        Err(format!("line {}: {msg}", self.line(span.start)))
    }

    /// An integer within `lo..=hi`.
    fn int(&self, v: &Spanned<i64>, key: &str, lo: i64, hi: i64) -> Result<i64, String> {
        let n = *v.get_ref();
        if (lo..=hi).contains(&n) {
            Ok(n)
        } else {
            self.err(v.span(), format!("{key} must be {lo} to {hi}, got {n}"))
        }
    }

    fn count(&self, v: &Spanned<i64>, key: &str, hi: i64) -> Result<u32, String> {
        self.int(v, key, 1, hi).map(|n| n as u32)
    }
}

/// FNV-1a, 64 bits: a hash that does not change between runs or machines
/// (unlike `std`'s randomized hasher), so a session replays the same draws.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn bpm(src: &Source, v: &Spanned<i64>, key: &str) -> Result<Bpm, String> {
    src.int(v, key, i64::from(Bpm::MIN), i64::from(Bpm::MAX))
        .map(|n| Bpm::new(n as i32))
}

fn stage_length(src: &Source, stage: &Spanned<StageFile>, n: usize) -> Result<Span, String> {
    let s = stage.get_ref();
    match (&s.bars, &s.seconds) {
        (Some(_), Some(sec)) => src.err(
            sec.span(),
            format!("stage {n}: use either bars or seconds, not both"),
        ),
        (None, None) => src.err(stage.span(), format!("stage {n}: needs bars or seconds")),
        (Some(b), None) => Ok(Span::Bars(src.count(b, "bars", 10_000)?)),
        (None, Some(sec)) => Ok(Span::Seconds(src.count(sec, "seconds", 86_400)?)),
    }
}

fn tempo(src: &Source, t: &Spanned<TempoFile>) -> Result<Option<Tempo>, String> {
    let f = t.get_ref();
    let at = t.span();
    if let (Some(_), Some(set)) = (&f.jump, &f.set) {
        return src.err(set.span(), "tempo: jump and set cannot be used together");
    }
    let jump = match (&f.jump, &f.set) {
        (Some(j), _) => {
            let n = src.int(j, "jump", i64::from(i32::MIN), i64::from(i32::MAX))?;
            Some(Jump::By(n as i32))
        }
        (_, Some(s)) => Some(Jump::To(bpm(src, s, "set")?)),
        _ => None,
    };
    let every = match (&f.every_bars, &f.every_seconds) {
        (Some(_), Some(s)) => {
            return src.err(
                s.span(),
                "tempo: use either every_bars or every_seconds, not both",
            );
        }
        (Some(b), None) => Some(Span::Bars(src.count(b, "every_bars", 10_000)?)),
        (None, Some(s)) => Some(Span::Seconds(src.count(s, "every_seconds", 86_400)?)),
        (None, None) => None,
    };
    let Some(change) = &f.change else {
        for (key, v) in [
            ("every_bars", f.every_bars.as_ref()),
            ("every_seconds", f.every_seconds.as_ref()),
            ("max", f.max.as_ref()),
            ("min", f.min.as_ref()),
        ] {
            if let Some(v) = v {
                return src.err(v.span(), format!("tempo: {key} needs change"));
            }
        }
        if let Some(v) = &f.smooth {
            return src.err(v.span(), "tempo: smooth needs change");
        }
        return Ok(jump.map(|j| Tempo {
            jump: Some(j),
            ramp: None,
        }));
    };
    let c = *change.get_ref();
    if c == 0 || i32::try_from(c).is_err() {
        return src.err(
            change.span(),
            format!("tempo: change must be a non-zero integer, got {c}"),
        );
    }
    let Some(every) = every else {
        return src.err(at, "tempo: change needs every_bars or every_seconds");
    };
    let limit = match (&f.max, &f.min) {
        (Some(m), _) if c < 0 => {
            return src.err(m.span(), "tempo: max only goes with a positive change");
        }
        (_, Some(m)) if c > 0 => {
            return src.err(m.span(), "tempo: min only goes with a negative change");
        }
        (Some(m), _) => Some(bpm(src, m, "max")?),
        (_, Some(m)) => Some(bpm(src, m, "min")?),
        _ => None,
    };
    Ok(Some(Tempo {
        jump,
        ramp: Some(Ramp {
            change: c as i32,
            every,
            limit,
            smooth: f.smooth.as_ref().is_some_and(|s| *s.get_ref()),
        }),
    }))
}

fn mute(src: &Source, m: &Spanned<MuteFile>) -> Result<MuteCycle, String> {
    let f = m.get_ref();
    let pair = |play: &Option<Spanned<i64>>, mute: &Option<Spanned<i64>>, names: [&str; 2]| match (
        play, mute,
    ) {
        (Some(p), Some(q)) => Ok((
            src.count(p, names[0], 1_000_000)?,
            src.count(q, names[1], 1_000_000)?,
        )),
        _ => src.err(
            m.span(),
            format!("mute: {} and {} go together", names[0], names[1]),
        ),
    };
    let bars = f.play_bars.is_some() || f.mute_bars.is_some();
    let seconds = f.play_seconds.is_some() || f.mute_seconds.is_some();
    match (bars, seconds) {
        (true, true) => src.err(
            m.span(),
            "mute: use bars or seconds on both sides, not mixed",
        ),
        (false, false) => src.err(
            m.span(),
            "mute: needs play_bars and mute_bars, or play_seconds and mute_seconds",
        ),
        (true, false) => {
            let (p, q) = pair(&f.play_bars, &f.mute_bars, ["play_bars", "mute_bars"])?;
            Ok(MuteCycle {
                play: Span::Bars(p),
                mute: Span::Bars(q),
            })
        }
        (false, true) => {
            let (p, q) = pair(
                &f.play_seconds,
                &f.mute_seconds,
                ["play_seconds", "mute_seconds"],
            )?;
            Ok(MuteCycle {
                play: Span::Seconds(p),
                mute: Span::Seconds(q),
            })
        }
    }
}

fn random_mute(src: &Source, r: &Spanned<RandomMuteFile>) -> Result<RandomMute, String> {
    let f = r.get_ref();
    let Some(percent) = &f.percent else {
        return src.err(r.span(), "random_mute: percent is required");
    };
    let percent = src.int(percent, "percent", 0, 100)? as u8;
    let increase = match &f.increase {
        Some(i) => src.int(i, "increase", 0, 100)? as u8,
        None => 0,
    };
    let every_bars = match &f.every_bars {
        Some(e) => src.count(e, "every_bars", 10_000)?,
        None if increase > 0 => {
            return src.err(r.span(), "random_mute: increase needs every_bars");
        }
        None => 1,
    };
    let max = match &f.max {
        Some(m) => {
            let n = src.int(m, "max", 0, 100)? as u8;
            if n < percent {
                return src.err(
                    m.span(),
                    format!("random_mute: max ({n}) must be at least percent ({percent})"),
                );
            }
            n
        }
        None => 100,
    };
    Ok(RandomMute {
        percent,
        increase,
        every_bars,
        max,
    })
}

fn pulses(
    src: &Source,
    list: &[Spanned<PulseFile>],
) -> Result<[Option<PulseState>; MAX_PULSES], String> {
    let mut out = [None; MAX_PULSES];
    for entry in list {
        let f = entry.get_ref();
        let (Some(pulse), Some(state)) = (&f.pulse, f.state) else {
            return src.err(entry.span(), "pulses: each entry needs pulse and state");
        };
        let position = src.int(pulse, "pulse", 1, MAX_PULSES as i64)? as usize - 1;
        if out[position].is_some() {
            return src.err(
                pulse.span(),
                format!("pulses: duplicate pulse {}", position + 1),
            );
        }
        out[position] = Some(match state {
            PulseStateFile::Off => PulseState::Off,
            PulseStateFile::Muted => PulseState::Muted,
            PulseStateFile::On => PulseState::On,
        });
    }
    Ok(out)
}

fn stage(src: &Source, s: &Spanned<StageFile>, n: usize) -> Result<Stage, String> {
    let f = s.get_ref();
    let length = stage_length(src, s, n)?;
    Ok(Stage {
        length,
        tempo: match &f.tempo {
            Some(t) => tempo(src, t)?,
            None => None,
        },
        mute: f.mute.as_ref().map(|m| mute(src, m)).transpose()?,
        random_mute: f
            .random_mute
            .as_ref()
            .map(|r| random_mute(src, r))
            .transpose()?,
        pulses: match &f.pulses {
            Some(list) => pulses(src, list)?,
            None => [None; MAX_PULSES],
        },
    })
}

/// Parses a session file. `file_stem` gives the default name and the seed of
/// the random-mute draws. `Ok` carries warnings (none today: unknown keys are
/// errors); `Err` carries a message with the line.
pub fn parse(text: &str, file_stem: &str) -> Result<(SessionPlan, Vec<String>), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let src = Source(text);
    let file: SessionFile = toml::from_str(text).map_err(|e| {
        let line = e.span().map_or(1, |s| src.line(s.start));
        // The message does not always name the key; quote the line itself.
        let quoted = text.lines().nth(line - 1).unwrap_or("").trim();
        format!("line {line}: {} (`{quoted}`)", e.message())
    })?;
    let stages = file.stage.unwrap_or_default();
    if stages.is_empty() {
        return Err("line 1: a session needs at least one [[stage]]".to_owned());
    }
    if stages.len() > MAX_STAGES {
        return src.err(
            stages[MAX_STAGES].span(),
            format!("a session has at most {MAX_STAGES} stages"),
        );
    }
    let stages = stages
        .iter()
        .enumerate()
        .map(|(i, s)| stage(&src, s, i + 1))
        .collect::<Result<Vec<_>, _>>()?;
    let name = file
        .name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| file_stem.to_owned());
    let plan = SessionPlan {
        name,
        stages,
        on_finish: match file.on_finish {
            None | Some(OnFinishFile::Stop) => OnFinish::Stop,
            Some(OnFinishFile::Continue) => OnFinish::Continue,
            Some(OnFinishFile::Repeat) => OnFinish::Repeat,
        },
        show_muted_visuals: file.show_muted_visuals.unwrap_or(false),
        seed: fnv1a(file_stem),
    };
    Ok((plan, Vec::new()))
}

fn int_schema(description: &str, min: Option<i64>, max: Option<i64>) -> serde_json::Value {
    let mut v = serde_json::json!({ "type": "integer", "description": description });
    if let Some(min) = min {
        v["minimum"] = min.into();
    }
    if let Some(max) = max {
        v["maximum"] = max.into();
    }
    v
}

/// JSON Schema (Draft 4) for a session file.
pub fn session_schema() -> serde_json::Value {
    let bpm = (i64::from(Bpm::MIN), i64::from(Bpm::MAX));
    let tempo = serde_json::json!({
        "type": "object",
        "description": "Tempo change in this stage: a one-off jump or set, a ramp, or both.",
        "additionalProperties": false,
        "properties": {
            "jump": int_schema("Add this many BPM when the stage starts (negative lowers it); not with set.", None, None),
            "set": int_schema("Set this tempo in BPM when the stage starts; not with jump.", Some(bpm.0), Some(bpm.1)),
            "change": int_schema("Ramp: BPM added each time the interval passes (not 0, negative lowers it). Needs every_bars or every_seconds.", None, None),
            "every_bars": int_schema("Ramp interval in bars; not with every_seconds.", Some(1), Some(10_000)),
            "every_seconds": int_schema("Ramp interval in seconds; not with every_bars.", Some(1), Some(86_400)),
            "max": int_schema("Ramp ceiling in BPM; only with a positive change.", Some(bpm.0), Some(bpm.1)),
            "min": int_schema("Ramp floor in BPM; only with a negative change.", Some(bpm.0), Some(bpm.1)),
            "smooth": { "type": "boolean", "description": "Move in whole-BPM steps on every pulse-1 beat instead of one step per interval; needs change. Default false." },
        }
    });
    let mute = serde_json::json!({
        "type": "object",
        "description": "Play, then mute, over and over. Use the bars pair or the seconds pair, not both.",
        "additionalProperties": false,
        "properties": {
            "play_bars": int_schema("Bars that play before each mute; goes with mute_bars.", Some(1), Some(1_000_000)),
            "mute_bars": int_schema("Bars that are muted after each play; goes with play_bars.", Some(1), Some(1_000_000)),
            "play_seconds": int_schema("Seconds that play before each mute; goes with mute_seconds.", Some(1), Some(1_000_000)),
            "mute_seconds": int_schema("Seconds that are muted after each play; goes with play_seconds.", Some(1), Some(1_000_000)),
        }
    });
    let random_mute = serde_json::json!({
        "type": "object",
        "description": "Mutes a share of the bars at random.",
        "additionalProperties": false,
        "required": ["percent"],
        "properties": {
            "percent": int_schema("Chance, in percent, that a bar is muted.", Some(0), Some(100)),
            "increase": int_schema("Percent added every every_bars bars (needs every_bars when above 0). Default 0.", Some(0), Some(100)),
            "every_bars": int_schema("Bars between increases.", Some(1), Some(10_000)),
            "max": int_schema("Highest chance the increases reach; at least percent. Default 100.", Some(0), Some(100)),
        }
    });
    let pulse = serde_json::json!({
        "type": "object",
        "description": "One pulse override.",
        "additionalProperties": false,
        "required": ["pulse", "state"],
        "properties": {
            "pulse": int_schema("Pulse position, counting from 1; each pulse at most once per stage.", Some(1), Some(MAX_PULSES as i64)),
            "state": { "enum": ["off", "muted", "on"], "description": "off: silent and dark; muted: silent but shown; on: always plays." },
        }
    });
    let stage = serde_json::json!({
        "type": "object",
        "description": "One stage of the session. With no options it just keeps playing for its length.",
        "additionalProperties": false,
        "properties": {
            "bars": int_schema("Stage length in bars; use this or seconds.", Some(1), Some(10_000)),
            "seconds": int_schema("Stage length in seconds; use this or bars.", Some(1), Some(86_400)),
            "tempo": tempo,
            "mute": mute,
            "random_mute": random_mute,
            "pulses": { "type": "array", "description": "Per-pulse overrides for this stage.", "items": pulse },
        }
    });
    serde_json::json!({
        "$schema": DRAFT_4,
        "title": "Polyrhythmic Metronome practice session",
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "name": { "type": "string", "description": "Name shown in the session list; default is the file name without .toml." },
            "on_finish": { "enum": ["stop", "continue", "repeat"], "description": "What happens after the last stage: stop (default), continue at the last tempo, or repeat from the first stage." },
            "show_muted_visuals": { "type": "boolean", "description": "Keep animating the pulses during muted stretches. Default false: the screen holds still." },
            "stage": { "type": "array", "description": "The stages, played in order.", "minItems": 1, "maxItems": MAX_STAGES, "items": stage },
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pm_core::Bpm;
    use pm_core::session::{
        Jump, MuteCycle, OnFinish, PulseState, Ramp, RandomMute, Span, Stage, Tempo,
    };
    use std::path::Path;

    const SPEC_EXAMPLE: &str = r#"#:schema ../schema/session.schema.json
name = "Warm-up 3:4"
on_finish = "stop"
show_muted_visuals = false

[[stage]]
bars = 8

[[stage]]
bars = 32
tempo = { change = 2, every_bars = 4, max = 140 }

[[stage]]
bars = 16
mute = { play_bars = 4, mute_bars = 4 }

[[stage]]
seconds = 60
pulses = [ { pulse = 2, state = "off" } ]

[[stage]]
bars = 16
random_mute = { percent = 20, increase = 5, every_bars = 4, max = 60 }
"#;

    fn plain(length: Span) -> Stage {
        Stage {
            length,
            tempo: None,
            mute: None,
            random_mute: None,
            pulses: [None; pm_core::MAX_PULSES],
        }
    }

    #[test]
    fn parses_the_spec_example() {
        let (plan, warnings) = parse(SPEC_EXAMPLE, "warmup").unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut s2 = plain(Span::Bars(32));
        s2.tempo = Some(Tempo {
            jump: None,
            ramp: Some(Ramp {
                change: 2,
                every: Span::Bars(4),
                limit: Some(Bpm::new(140)),
                smooth: false,
            }),
        });
        let mut s3 = plain(Span::Bars(16));
        s3.mute = Some(MuteCycle {
            play: Span::Bars(4),
            mute: Span::Bars(4),
        });
        let mut s4 = plain(Span::Seconds(60));
        s4.pulses[1] = Some(PulseState::Off);
        let mut s5 = plain(Span::Bars(16));
        s5.random_mute = Some(RandomMute {
            percent: 20,
            increase: 5,
            every_bars: 4,
            max: 60,
        });
        assert_eq!(
            plan,
            SessionPlan {
                name: "Warm-up 3:4".into(),
                stages: vec![plain(Span::Bars(8)), s2, s3, s4, s5],
                on_finish: OnFinish::Stop,
                show_muted_visuals: false,
                seed: plan.seed,
            }
        );
    }

    #[test]
    fn defaults_jump_set_and_seed() {
        let (a, _) = parse("[[stage]]\nbars = 1\n", "alpha").unwrap();
        assert_eq!(a.name, "alpha");
        assert_eq!(a.on_finish, OnFinish::Stop);
        assert!(!a.show_muted_visuals);
        let (b, _) = parse("[[stage]]\nbars = 1\n", "alpha").unwrap();
        let (c, _) = parse("[[stage]]\nbars = 1\n", "beta").unwrap();
        assert_eq!(a.seed, b.seed);
        assert_ne!(a.seed, c.seed);
        assert_eq!(a.seed, 0x8ac6_25bb_85ed_202b, "FNV-1a 64 of the stem");
        let (p, _) = parse(
            "on_finish = \"repeat\"\n[[stage]]\nseconds = 5\ntempo = { jump = -10 }\n[[stage]]\nbars = 2\ntempo = { set = 90, change = -5, every_seconds = 10, min = 60, smooth = true }\n",
            "x",
        )
        .unwrap();
        assert_eq!(p.on_finish, OnFinish::Repeat);
        assert_eq!(
            p.stages[0].tempo,
            Some(Tempo {
                jump: Some(Jump::By(-10)),
                ramp: None
            })
        );
        assert_eq!(
            p.stages[1].tempo,
            Some(Tempo {
                jump: Some(Jump::To(Bpm::new(90))),
                ramp: Some(Ramp {
                    change: -5,
                    every: Span::Seconds(10),
                    limit: Some(Bpm::new(60)),
                    smooth: true
                })
            })
        );
    }

    #[test]
    fn rejects_bad_values() {
        let cases: &[(&str, usize, &str)] = &[
            ("[[stage]]\nbars = 1\nseconds = 5\n", 3, "bars"),
            ("name = \"x\"\n[[stage]]\n", 2, "bars"),
            (
                "[[stage]]\nbars = 1\nrandom_mute = { percent = 101 }\n",
                3,
                "percent",
            ),
            (
                "[[stage]]\nbars = 1\ntempo = { change = -2, every_bars = 1, max = 100 }\n",
                3,
                "max",
            ),
            (
                "[[stage]]\nbars = 1\ntempo = { jump = 1, set = 90 }\n",
                3,
                "jump",
            ),
            (
                "[[stage]]\nbars = 1\npulses = [ { pulse = 17, state = \"off\" } ]\n",
                3,
                "pulse",
            ),
            (
                "[[stage]]\nbars = 1\npulses = [\n { pulse = 2, state = \"off\" },\n { pulse = 2, state = \"on\" },\n]\n",
                5,
                "duplicate",
            ),
            ("[[stage]]\nbars = 1\ntempo = { chnage = 2 }\n", 3, "chnage"),
            ("name = \"x\"\n", 1, "stage"),
            ("[[stage]]\nbars = 0\n", 2, "bars"),
            (
                "[[stage]]\nbars = 1\nmute = { play_bars = 1, mute_seconds = 1 }\n",
                3,
                "mute",
            ),
            (
                "[[stage]]\nbars = 1\ntempo = { change = 0, every_bars = 1 }\n",
                3,
                "change",
            ),
            ("[[stage]]\nbars = 1\ntempo = { set = 400 }\n", 3, "set"),
            (
                "[[stage]]\nbars = 1\ntempo = { smooth = true }\n",
                3,
                "smooth",
            ),
            (
                "[[stage]]\nbars = 1\nrandom_mute = { percent = 50, increase = 5 }\n",
                3,
                "every_bars",
            ),
            (
                "[[stage]]\nbars = 1\nrandom_mute = { percent = 50, max = 40 }\n",
                3,
                "max",
            ),
            ("on_finish = \"loop\"\n[[stage]]\nbars = 1\n", 1, "loop"),
            ("[[stage]]\nbars = 1\n[[stage]]\nbars = \"x\"\n", 4, "bars"),
        ];
        for (text, line, word) in cases {
            let err = parse(text, "s").unwrap_err();
            assert!(
                err.contains(&format!("line {line}")) && err.contains(word),
                "{text:?}: {err}"
            );
        }
    }

    #[test]
    fn rejects_too_many_stages() {
        let text = "[[stage]]\nbars = 1\n".repeat(257);
        assert!(parse(&text, "s").unwrap_err().contains("256"));
        assert!(parse(&"[[stage]]\nbars = 1\n".repeat(256), "s").is_ok());
    }

    fn check_or_update(rel: &str, value: &serde_json::Value) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel);
        let want = crate::config::pretty(value);
        if std::env::var_os("UPDATE_SCHEMAS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &want).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            want,
            "{rel} is stale; run with UPDATE_SCHEMAS=1"
        );
    }

    fn every_property_has_a_description(node: &serde_json::Value, path: &str) {
        if let Some(props) = node.get("properties").and_then(|p| p.as_object()) {
            for (key, child) in props {
                let here = format!("{path}.{key}");
                assert!(child.get("description").is_some(), "{here}: no description");
                every_property_has_a_description(child, &here);
            }
        }
        if let Some(items) = node.get("items") {
            every_property_has_a_description(items, &format!("{path}[]"));
        }
    }

    #[test]
    fn schema_is_draft4_and_committed() {
        let s = session_schema();
        let text = s.to_string();
        for banned in ["\"$defs\"", "\"$ref\"", "\"const\"", "\"definitions\""] {
            assert!(!text.contains(banned), "{banned}");
        }
        assert_eq!(s["$schema"], "http://json-schema.org/draft-04/schema#");
        assert_eq!(s["additionalProperties"], false);
        every_property_has_a_description(&s, "");
        check_or_update("schema/session.schema.json", &s);
    }
}
