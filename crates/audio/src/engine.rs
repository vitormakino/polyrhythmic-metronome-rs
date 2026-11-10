//! The real-time engine: turns the timeline's hits into samples.
//!
//! [`Engine::render`] runs on the audio thread. It never allocates, frees,
//! locks or does I/O: commands arrive through a lock-free queue, beat events
//! and replaced patterns leave through others, and every buffer it needs is
//! sized in [`Engine::new`]. Click frames come straight from
//! [`Timeline::events_in`], so they are sample-exact and never drift.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use pm_core::session::{Moment, OnFinish, Runner, SessionPlan, SessionProgress};
use pm_core::synth::end_chime;
use pm_core::{
    ApplyMode, BeatHit, Bpm, MAX_PULSES, MusicalPosition, Pattern, SampleBank, Timeline,
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::messages::{
    BeatEvent, Command, PatternUpdate, PositionReader, PositionSnapshot, PositionWriter,
    position_cell,
};
use crate::session::{SessionReader, SessionWriter, session_cell};

/// Clicks sounding at once; when all are busy the oldest is cut.
const VOICES: usize = 64;
/// Frames mixed per step. Hits are fetched per step, so `HIT_CAPACITY` holds
/// one hit per pulse per frame.
const SUB_BLOCK: usize = 64;
const HIT_CAPACITY: usize = MAX_PULSES * SUB_BLOCK;
const COMMAND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 1024;
const GARBAGE_CAPACITY: usize = 8;
/// A `Session` command is only taken with two free slots: one for the plan
/// it replaces, one for the plan a later `Stop` puts in place.
const SESSION_GARBAGE_CAPACITY: usize = 4;
/// `Voice::owner` of the end-of-session chime (pattern ids count up from 0).
const CHIME: u64 = u64::MAX;
/// Replaced patterns waiting for their last click to end or for room in the
/// garbage queue. A pattern command is only taken when two slots are free
/// (the most one command can retire), which leaves one for a pending switch.
const HOLD_SLOTS: usize = 8;
/// Soft-clip knee: samples up to this level pass unchanged (a single Hard
/// click peaks at 0.8); above it they bend smoothly towards 1.0.
const KNEE: f32 = 0.8;

/// What the engine is started with.
#[derive(Clone, Debug)]
pub struct EngineInit {
    pub pattern: Pattern,
    pub bpm: Bpm,
    /// `None`: stopped until [`Command::Play`]. `Some(position)`: playing
    /// from the first frame, which is `position` (from the previous engine's
    /// [`PositionSnapshot`], possibly at another sample rate). This is how a
    /// rebuilt stream continues without losing or repeating a beat; see
    /// [`Timeline::resume`] for what `pattern` and `bpm` must be.
    pub resume: Option<MusicalPosition>,
    /// The session plan the next [`Command::Play`] runs, as if sent with
    /// [`Command::Session`]. With `resume`, the second field continues a
    /// session: a [`SessionProgress`] whose frame counts are as of the first
    /// frame (the old engine's, see [`Transport::resume_init_with`]), and
    /// `bpm` its tempo. With `resume` at [`MusicalPosition::start_of`] and no
    /// progress, the session starts at the first frame, as `Play` would.
    ///
    /// [`Transport::resume_init_with`]: crate::Transport::resume_init_with
    pub session: Option<(Box<SessionPlan>, Option<SessionProgress>)>,
}

/// The UI thread's ends of the engine's queues.
pub struct EngineHandle {
    pub commands: Producer<Command>,
    pub events: Consumer<BeatEvent>,
    /// Replaced patterns, to be dropped here rather than on the audio thread.
    ///
    /// Drain it every UI frame (or let [`AudioService::tick`] do it): the
    /// engine parks a `SetPattern` while its retire slots are full, and every
    /// command queued after it, including `Stop`, waits behind it.
    ///
    /// [`AudioService::tick`]: crate::AudioService::tick
    pub garbage: Consumer<Box<PatternUpdate>>,
    pub position: PositionReader,
    /// Progress of the running (or last) session.
    pub session: SessionReader,
    /// Replaced session plans, to be dropped here (drained by
    /// [`AudioService::tick`] too). A `Session` command waits while it is
    /// nearly full, and so do the commands behind it.
    ///
    /// [`AudioService::tick`]: crate::AudioService::tick
    pub session_garbage: Consumer<Box<SessionPlan>>,
    /// Output underruns, counted by the stream that drives the engine.
    pub underruns: Arc<AtomicU64>,
    /// Beat events dropped because `events` was full.
    pub dropped_events: Arc<AtomicU64>,
}

/// A pattern owned by the engine, tagged so voices can tell which bank they
/// play from.
struct Owned {
    id: u64,
    update: Box<PatternUpdate>,
}

#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    /// `Owned::id` of the bank the clip comes from.
    owner: u64,
    pulse: usize,
    accent: bool,
    gain: f32,
    /// Samples of the clip already played.
    pos: usize,
    /// Frame the click started at (for stealing the oldest).
    started: u64,
}

impl Voice {
    const IDLE: Voice = Voice {
        active: false,
        owner: 0,
        pulse: 0,
        accent: false,
        gain: 0.0,
        pos: 0,
        started: 0,
    };
}

/// `Extend` that never grows the vector: hits past its capacity are dropped
/// (unreachable unless a pulse has more than one beat per frame).
struct Bounded<'a>(&'a mut Vec<BeatHit>);

impl Extend<BeatHit> for Bounded<'_> {
    fn extend<I: IntoIterator<Item = BeatHit>>(&mut self, iter: I) {
        for hit in iter {
            if self.0.len() < self.0.capacity() {
                self.0.push(hit);
            }
        }
    }
}

/// A session being played.
struct Active {
    runner: Runner,
    /// Runner frame minus engine frame: a resumed session started before
    /// this engine's frame 0.
    epoch: u64,
    /// What the latest bar or beat asked for.
    moment: Moment,
    /// Engine frame of the latest reference beat handled.
    handled: Option<u64>,
    /// Engine frame the runner's progress describes (`handled`, or frame 0
    /// for a resumed session).
    instant: u64,
    /// No bar handled yet: the next one is the session's first.
    fresh: bool,
}

impl Active {
    fn start(plan: &SessionPlan, bpm: Bpm, sample_rate: u32, at: u64) -> Active {
        let runner = Runner::start(plan, bpm, sample_rate, at);
        Active {
            moment: runner.progress().moment,
            runner,
            epoch: 0,
            handled: None,
            instant: at,
            fresh: true,
        }
    }

    /// Continues `progress`, whose instant is this engine's frame 0.
    fn resume(progress: &SessionProgress, sample_rate: u32) -> Active {
        // `Runner::resume`'s rescale: the session starts at runner frame 0.
        let epoch = (u128::from(progress.total_frames) * u128::from(sample_rate.max(1))
            / u128::from(progress.sample_rate.max(1))) as u64;
        let runner = Runner::resume(progress, sample_rate, epoch);
        Active {
            moment: runner.progress().moment,
            runner,
            epoch,
            handled: None,
            instant: 0,
            fresh: false,
        }
    }
}

/// Real-time renderer. Create it with [`Engine::new`], move it to the audio
/// thread and call [`render`](Engine::render) for every output buffer.
pub struct Engine {
    sample_rate: u32,
    channels: usize,
    timeline: Timeline,
    /// Pattern rendered now.
    current: Owned,
    /// Pattern taking over at the timeline's pending switch.
    next: Option<Owned>,
    retired: [Option<Owned>; HOLD_SLOTS],
    next_id: u64,
    voices: [Voice; VOICES],
    hits: Vec<BeatHit>,
    mix: [f32; SUB_BLOCK],
    /// First frame of the next buffer.
    frame: u64,
    playing: bool,
    /// Position reported while stopped.
    stopped_at: MusicalPosition,
    soft_clip: bool,
    commands: Consumer<Command>,
    events: Producer<BeatEvent>,
    garbage: Producer<Box<PatternUpdate>>,
    position: PositionWriter,
    dropped_events: Arc<AtomicU64>,
    /// Tempo the timeline plays now.
    bpm: Bpm,
    /// Plan the next `Play` runs; while `active`, the one it runs.
    plan: Option<Box<SessionPlan>>,
    /// A `Session` sent while `active`: replaces `plan` when it ends.
    queued: Option<Option<Box<SessionPlan>>>,
    active: Option<Active>,
    /// `Play` commands taken, no-ops included (the Transport counts the
    /// ones it sent).
    plays: u64,
    chime: Vec<f32>,
    session_garbage: Producer<Box<SessionPlan>>,
    session_out: SessionWriter,
}

impl Engine {
    /// Builds an engine for an output of `sample_rate` Hz with `channels`
    /// interleaved channels. Allocates everything `render` will use.
    pub fn new(sample_rate: u32, channels: u16, init: EngineInit) -> (Engine, EngineHandle) {
        let (command_tx, command_rx) = RingBuffer::new(COMMAND_CAPACITY);
        let (event_tx, event_rx) = RingBuffer::new(EVENT_CAPACITY);
        let (garbage_tx, garbage_rx) = RingBuffer::new(GARBAGE_CAPACITY);
        let (writer, reader) = position_cell();
        let (session_tx, session_rx) = session_cell();
        let (session_garbage_tx, session_garbage_rx) = RingBuffer::new(SESSION_GARBAGE_CAPACITY);
        let dropped_events = Arc::new(AtomicU64::new(0));
        let timeline = match init.resume {
            Some(pos) => Timeline::resume(sample_rate, init.bpm, &init.pattern, pos, 0),
            None => Timeline::new(sample_rate, init.bpm, &init.pattern, 0),
        };
        let (plan, active) = match init.session {
            None => (None, None),
            Some((plan, progress)) => {
                let active = match (init.resume, progress) {
                    (Some(_), Some(progress)) => Some(Active::resume(&progress, sample_rate)),
                    (Some(pos), None) if pos.ticks_per_measure == 0 => {
                        Some(Active::start(&plan, init.bpm, sample_rate, 0))
                    }
                    _ => None,
                };
                (Some(plan), active)
            }
        };
        let engine = Engine {
            sample_rate,
            channels: usize::from(channels.max(1)),
            stopped_at: timeline.position_at(0),
            timeline,
            current: Owned {
                id: 0,
                update: PatternUpdate::build(sample_rate, init.pattern),
            },
            next: None,
            retired: Default::default(),
            next_id: 1,
            voices: [Voice::IDLE; VOICES],
            hits: Vec::with_capacity(HIT_CAPACITY),
            mix: [0.0; SUB_BLOCK],
            frame: 0,
            playing: init.resume.is_some(),
            soft_clip: true,
            commands: command_rx,
            events: event_tx,
            garbage: garbage_tx,
            position: writer,
            dropped_events: Arc::clone(&dropped_events),
            bpm: init.bpm,
            plan,
            queued: None,
            active,
            plays: 0,
            chime: end_chime(sample_rate),
            session_garbage: session_garbage_tx,
            session_out: session_tx,
        };
        engine.publish();
        engine.publish_session();
        let handle = EngineHandle {
            commands: command_tx,
            events: event_rx,
            garbage: garbage_rx,
            position: reader,
            session: session_rx,
            session_garbage: session_garbage_rx,
            underruns: Arc::new(AtomicU64::new(0)),
            dropped_events,
        };
        (engine, handle)
    }

    /// Turns the output soft clipper on (the default) or off. Off is for
    /// tests that compare samples exactly; the mix can then exceed 1.0.
    pub fn set_soft_clip(&mut self, on: bool) {
        self.soft_clip = on;
    }

    /// Fills `out` (interleaved, `channels` per frame) with the next frames,
    /// the same mono mix on every channel. `device_time_offset` is the time
    /// from now until the first frame of `out` reaches the DAC; it dates the
    /// beat events. Commands sent so far apply at the first frame of `out`.
    pub fn render(&mut self, out: &mut [f32], device_time_offset: Duration) {
        let buffer_time = Instant::now() + device_time_offset;
        out.fill(0.0);
        let start = self.frame;
        if self.playing {
            self.promote_due(start);
        }
        self.release_retired();
        self.drain_commands(start);
        let frames = out.len() / self.channels;
        let mut done = 0;
        while done < frames {
            let at = start + done as u64;
            let mut len = (frames - done).min(SUB_BLOCK);
            if self.playing {
                self.promote_due(at);
                // May stop playing (a finished session).
                self.session_beat(at);
            }
            if self.playing {
                // The old pattern renders up to the switch, the new one from it.
                if let (Some(_), Some(switch)) = (&self.next, self.timeline.pending_switch())
                    && switch < at + len as u64
                {
                    len = (switch - at) as usize;
                }
                // A session acts on reference beats: each starts a sub-block.
                if self.active.is_some() {
                    let (beat, _) = self.timeline.next_reference_beat(at + 1);
                    len = len.min((beat - at) as usize);
                }
            }
            self.mix[..len].fill(0.0);
            self.mix_voices(len);
            if self.playing {
                self.schedule(at, len, start, buffer_time);
            }
            let block = &mut out[done * self.channels..(done + len) * self.channels];
            for (frame, &x) in block.chunks_exact_mut(self.channels).zip(&self.mix[..len]) {
                frame.fill(if self.soft_clip { soft_clip(x) } else { x });
            }
            done += len;
        }
        self.frame = start + frames as u64;
        self.release_retired();
        self.publish();
    }

    fn drain_commands(&mut self, at: u64) {
        while let Ok(command) = self.commands.peek() {
            let full = match command {
                Command::SetPattern(_) => self.free_slots() < 2,
                Command::Session(_) => self.session_garbage.slots() < 2,
                _ => false,
            };
            if full {
                // No room to retire what it replaces: wait for the UI to
                // empty `garbage`. Later commands wait too, keeping order.
                break;
            }
            let Ok(command) = self.commands.pop() else {
                break;
            };
            match command {
                Command::Play => self.play(at),
                Command::Stop => self.stop(at),
                Command::SetBpm(bpm) => self.set_bpm(at, bpm),
                Command::SetPattern(update) => self.set_pattern(at, update),
                Command::Session(plan) => self.select_session(plan),
                Command::UserPulse(position) => self.user_pulse(position),
            }
        }
    }

    fn play(&mut self, at: u64) {
        self.plays += 1;
        if !self.playing {
            self.timeline.restart(&self.current.update.pattern, at);
            self.playing = true;
            // A new run: the previous run's progress goes before the Play
            // count does, so no reader pairs it with this Play.
            self.session_out.publish(None);
            if let Some(plan) = self.plan.as_deref() {
                self.active = Some(Active::start(plan, self.bpm, self.sample_rate, at));
            }
        }
        self.session_out.publish_plays(self.plays);
    }

    fn stop(&mut self, at: u64) {
        if !self.playing {
            return;
        }
        self.halt(at);
        self.voices = [Voice::IDLE; VOICES];
    }

    /// Stops scheduling clicks from `at` on; what rings goes on ringing.
    fn halt(&mut self, at: u64) {
        self.stopped_at = self.timeline.position_at(at);
        self.playing = false;
        // No need to wait for the "one" any more.
        if let Some(next) = self.next.take() {
            let old = std::mem::replace(&mut self.current, next);
            self.retire(old);
        }
        if self.active.take().is_some()
            && let Some(plan) = self.queued.take()
        {
            self.replace_plan(plan);
        }
    }

    fn set_bpm(&mut self, at: u64, bpm: Bpm) {
        self.timeline.set_bpm(at, bpm);
        self.bpm = bpm;
        if let Some(active) = &mut self.active {
            active.runner.user_bpm(bpm);
            active.moment = active.runner.progress().moment;
            self.publish_session();
        }
    }

    fn user_pulse(&mut self, position: usize) {
        if let Some(active) = &mut self.active {
            active.runner.user_pulse(position);
            active.moment = active.runner.progress().moment;
            self.publish_session();
        }
    }

    fn select_session(&mut self, plan: Option<Box<SessionPlan>>) {
        if self.active.is_some() {
            if let Some(Some(old)) = self.queued.replace(plan) {
                self.drop_plan(old);
            }
        } else {
            self.replace_plan(plan);
        }
    }

    fn replace_plan(&mut self, plan: Option<Box<SessionPlan>>) {
        if let Some(old) = std::mem::replace(&mut self.plan, plan) {
            self.drop_plan(old);
        }
    }

    /// Hands `plan` to the UI thread to be freed there.
    fn drop_plan(&mut self, plan: Box<SessionPlan>) {
        if let Err(PushError::Full(plan)) = self.session_garbage.push(plan) {
            // Unreachable: `Session` waits for two free slots. Leaking beats
            // freeing on the audio thread.
            debug_assert!(false, "no room to retire a session plan");
            std::mem::forget(plan);
        }
    }

    /// At a reference beat not handled yet: the runner's bar (beat 0) or
    /// smooth-ramp beat, the tempo it asks for, the chime at the end.
    fn session_beat(&mut self, at: u64) {
        let (Some(active), Some(plan)) = (&mut self.active, self.plan.as_deref()) else {
            return;
        };
        if active.handled == Some(at) {
            return;
        }
        let (frame, beat) = self.timeline.next_reference_beat(at);
        if frame != at {
            return;
        }
        active.handled = Some(at);
        active.instant = at;
        let runner_frame = at + active.epoch;
        let mut outcome = None;
        let mut chime = false;
        if beat == 0 {
            let before = active.runner.progress();
            let (moment, finished) = active.runner.bar(plan, runner_frame);
            let after = active.runner.progress();
            let repeated = plan.on_finish == OnFinish::Repeat
                && !active.fresh
                && before.stage == before.stages
                && (after.stage, after.bars_in_stage) == (1, 0);
            chime = finished.is_some() || repeated;
            outcome = finished;
            active.fresh = false;
            active.moment = moment;
        } else {
            let stage = active.runner.progress().stage as usize;
            let smooth = stage
                .checked_sub(1)
                .and_then(|i| plan.stages.get(i))
                .and_then(|s| s.tempo?.ramp)
                .is_some_and(|r| r.smooth);
            if smooth {
                let beats = self.current.update.pattern.reference_beats();
                active.moment = active.runner.beat(plan, runner_frame, beat, beats);
            }
        }
        let bpm = active.moment.bpm;
        if bpm != self.bpm {
            self.timeline.set_bpm(at, bpm);
            self.bpm = bpm;
        }
        self.publish_session();
        if chime {
            let slot = self.free_voice();
            self.voices[slot] = Voice {
                active: true,
                owner: CHIME,
                pulse: 0,
                accent: false,
                gain: 1.0,
                pos: 0,
                started: at,
            };
        }
        if outcome == Some(true) {
            self.halt(at);
        }
    }

    /// Publishes the running session's progress. Before its first bar there
    /// is nothing to publish yet.
    fn publish_session(&self) {
        if let Some(active) = self.active.as_ref().filter(|a| !a.fresh) {
            self.session_out
                .publish(Some((active.instant, &active.runner.progress())));
        }
    }

    fn set_pattern(&mut self, at: u64, update: Box<PatternUpdate>) {
        let new = Owned {
            id: self.next_id,
            update,
        };
        self.next_id += 1;
        if new.update.pattern.pulses.len() > MAX_PULSES {
            // Growing the timeline past its reserve would allocate here.
            self.retire(new);
            return;
        }
        if let Some(active) = &mut self.active {
            // The user wins on every pulse they turned on or off.
            let old = &self.next.as_ref().unwrap_or(&self.current).update.pattern;
            let changed = old.pulses.iter().zip(&new.update.pattern.pulses);
            for (i, (o, n)) in changed.enumerate() {
                if o.enabled != n.enabled {
                    active.runner.user_pulse(i);
                }
            }
            active.moment = active.runner.progress().moment;
            self.publish_session();
        }
        // Within MAX_PULSES this only uses reserved room.
        self.timeline.sync_pattern(&new.update.pattern);
        let mode = if self.playing {
            self.timeline
                .apply_pattern(at, &self.current.update.pattern, &new.update.pattern)
        } else {
            // Play restarts the timeline from the current pattern anyway.
            ApplyMode::Now
        };
        match mode {
            ApplyMode::Now => {
                let old = std::mem::replace(&mut self.current, new);
                self.retire(old);
                if let Some(pending) = self.next.take() {
                    self.retire(pending);
                }
            }
            ApplyMode::AtNextOne(_) => {
                if let Some(pending) = self.next.replace(new) {
                    self.retire(pending);
                }
            }
        }
    }

    /// Makes `next` current once the switch frame is reached.
    fn promote_due(&mut self, at: u64) {
        if self.timeline.pending_switch().is_none_or(|f| f <= at)
            && let Some(next) = self.next.take()
        {
            let old = std::mem::replace(&mut self.current, next);
            self.retire(old);
        }
    }

    fn free_slots(&self) -> usize {
        self.retired.iter().filter(|s| s.is_none()).count()
    }

    fn retire(&mut self, owned: Owned) {
        if let Some(slot) = self.retired.iter_mut().find(|s| s.is_none()) {
            *slot = Some(owned);
        } else {
            // Unreachable: commands wait for two free slots. Leaking beats
            // freeing on the audio thread.
            debug_assert!(false, "no slot to retire a pattern");
            std::mem::forget(owned);
        }
    }

    /// Sends retired patterns no voice plays from any more to `garbage`;
    /// keeps them if it is full.
    fn release_retired(&mut self) {
        for slot in &mut self.retired {
            let Some(owned) = slot.take() else { continue };
            if self.voices.iter().any(|v| v.active && v.owner == owned.id) {
                *slot = Some(owned);
                continue;
            }
            if let Err(PushError::Full(update)) = self.garbage.push(owned.update) {
                *slot = Some(Owned {
                    id: owned.id,
                    update,
                });
            }
        }
    }

    /// Continues every sounding click over `mix[..len]`.
    fn mix_voices(&mut self, len: usize) {
        let Engine {
            voices,
            mix,
            current,
            retired,
            chime,
            ..
        } = self;
        for voice in voices.iter_mut().filter(|v| v.active) {
            let clip = clip_of(current, retired, chime, voice);
            play(voice, clip, &mut mix[..len]);
        }
    }

    /// Starts the clicks of `[at, at + len)` and reports them as events.
    /// A running session drops the hits of its `off` pulses and mutes others.
    fn schedule(&mut self, at: u64, len: usize, buffer_start: u64, buffer_time: Instant) {
        self.hits.clear();
        let pattern = &self.current.update.pattern;
        let moment = self.active.as_ref().map(|a| a.moment);
        let forced_on = moment.map_or(0, |m| m.pulses.forced_on);
        self.timeline.events_in_forced(
            pattern,
            at,
            at + len as u64,
            forced_on,
            &mut Bounded(&mut self.hits),
        );
        // One random draw per instant: hits at the same frame share it.
        let mut draw: Option<(u64, bool)> = None;
        for i in 0..self.hits.len() {
            let hit = self.hits[i];
            let pulse = pattern
                .pulses
                .iter()
                .position(|p| p.id == hit.pulse)
                .unwrap_or(usize::MAX);
            let mut muted = false;
            if let (Some(m), Some(active)) = (moment, &mut self.active) {
                let bit = if pulse < 16 { 1u16 << pulse } else { 0 };
                if m.pulses.off & bit != 0 {
                    continue;
                }
                muted = m.silent || m.pulses.muted & bit != 0;
                if m.random_percent > 0 {
                    let drawn = match draw {
                        Some((frame, drawn)) if frame == hit.frame => drawn,
                        _ => active.runner.draw(m.random_percent),
                    };
                    draw = Some((hit.frame, drawn));
                    muted |= drawn;
                }
            }
            let since = (hit.frame - buffer_start) * 1_000_000_000 / u64::from(self.sample_rate);
            let event = BeatEvent {
                pulse: hit.pulse,
                beat: hit.beat,
                frame: hit.frame,
                at: buffer_time + Duration::from_nanos(since),
                muted,
            };
            if self.events.push(event).is_err() {
                self.dropped_events.fetch_add(1, Ordering::Relaxed);
            }
            if muted {
                continue;
            }
            let slot = self.free_voice();
            self.voices[slot] = Voice {
                active: true,
                owner: self.current.id,
                pulse,
                accent: hit.accent,
                gain: hit.gain,
                pos: 0,
                started: hit.frame,
            };
            let clip = clip_of(
                &self.current,
                &self.retired,
                &self.chime,
                &self.voices[slot],
            );
            let offset = (hit.frame - at) as usize;
            play(&mut self.voices[slot], clip, &mut self.mix[offset..len]);
        }
    }

    /// An idle voice, or the oldest one (cut).
    fn free_voice(&self) -> usize {
        if let Some(i) = self.voices.iter().position(|v| !v.active) {
            return i;
        }
        (0..VOICES)
            .min_by_key(|&i| self.voices[i].started)
            .unwrap_or(0)
    }

    fn publish(&self) {
        let position = if self.playing {
            self.timeline.position_at(self.frame)
        } else {
            self.stopped_at
        };
        self.position.publish(PositionSnapshot {
            frame: self.frame,
            position,
            playing: self.playing,
        });
    }
}

/// The clip `voice` plays, from the bank it was started with (or the chime).
fn clip_of<'a>(
    current: &'a Owned,
    retired: &'a [Option<Owned>],
    chime: &'a [f32],
    voice: &Voice,
) -> &'a [f32] {
    if voice.owner == CHIME {
        return chime;
    }
    let bank: Option<&SampleBank> = if current.id == voice.owner {
        Some(&current.update.bank)
    } else {
        retired
            .iter()
            .flatten()
            .find(|o| o.id == voice.owner)
            .map(|o| &o.update.bank)
    };
    bank.map_or(&[], |b| b.clip(voice.pulse, voice.accent))
}

/// Adds the voice's next samples × gain into `mix`; ends it with its clip.
fn play(voice: &mut Voice, clip: &[f32], mix: &mut [f32]) {
    let rest = clip.get(voice.pos..).unwrap_or(&[]);
    let n = rest.len().min(mix.len());
    for (m, s) in mix[..n].iter_mut().zip(&rest[..n]) {
        *m += s * voice.gain;
    }
    voice.pos += n;
    if voice.pos >= clip.len() {
        voice.active = false;
    }
}

/// The output soft clipper (public so tools can reproduce the engine's
/// output exactly). Unity below the knee, then a tanh bend: continuous, slope 1 at the knee,
/// never above 1.0.
pub fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let bent = KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh();
    bent.min(1.0).copysign(x)
}
