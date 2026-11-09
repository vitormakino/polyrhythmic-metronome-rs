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

use pm_core::{
    ApplyMode, BeatHit, Bpm, MAX_PULSES, MusicalPosition, Pattern, SampleBank, Timeline,
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::messages::{
    BeatEvent, Command, PatternUpdate, PositionReader, PositionSnapshot, PositionWriter,
    position_cell,
};

/// Clicks sounding at once; when all are busy the oldest is cut.
const VOICES: usize = 64;
/// Frames mixed per step. Hits are fetched per step, so `HIT_CAPACITY` holds
/// one hit per pulse per frame.
const SUB_BLOCK: usize = 64;
const HIT_CAPACITY: usize = MAX_PULSES * SUB_BLOCK;
const COMMAND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 1024;
const GARBAGE_CAPACITY: usize = 8;
/// Replaced patterns waiting for their last click to end or for room in the
/// garbage queue. A pattern command is only taken when two slots are free
/// (the most one command can retire), which leaves one for a pending switch.
const HOLD_SLOTS: usize = 8;
/// Soft-clip knee: samples up to this level pass unchanged (a single Hard
/// click peaks at 0.8); above it they bend smoothly towards 1.0.
const KNEE: f32 = 0.8;

/// What the engine is started with.
pub struct EngineInit {
    pub pattern: Pattern,
    pub bpm: Bpm,
    /// `None`: stopped until [`Command::Play`]. `Some(position)`: playing
    /// from the first frame, which is `position` (from the previous engine's
    /// [`PositionSnapshot`], possibly at another sample rate). This is how a
    /// rebuilt stream continues without losing or repeating a beat; see
    /// [`Timeline::resume`] for what `pattern` and `bpm` must be.
    pub resume: Option<MusicalPosition>,
}

/// The UI thread's ends of the engine's queues.
pub struct EngineHandle {
    pub commands: Producer<Command>,
    pub events: Consumer<BeatEvent>,
    /// Replaced patterns, to be dropped here rather than on the audio thread.
    pub garbage: Consumer<Box<PatternUpdate>>,
    pub position: PositionReader,
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
}

impl Engine {
    /// Builds an engine for an output of `sample_rate` Hz with `channels`
    /// interleaved channels. Allocates everything `render` will use.
    pub fn new(sample_rate: u32, channels: u16, init: EngineInit) -> (Engine, EngineHandle) {
        let (command_tx, command_rx) = RingBuffer::new(COMMAND_CAPACITY);
        let (event_tx, event_rx) = RingBuffer::new(EVENT_CAPACITY);
        let (garbage_tx, garbage_rx) = RingBuffer::new(GARBAGE_CAPACITY);
        let (writer, reader) = position_cell();
        let dropped_events = Arc::new(AtomicU64::new(0));
        let timeline = match init.resume {
            Some(pos) => Timeline::resume(sample_rate, init.bpm, &init.pattern, pos, 0),
            None => Timeline::new(sample_rate, init.bpm, &init.pattern, 0),
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
        };
        engine.publish();
        let handle = EngineHandle {
            commands: command_tx,
            events: event_rx,
            garbage: garbage_rx,
            position: reader,
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
                // The old pattern renders up to the switch, the new one from it.
                if let (Some(_), Some(switch)) = (&self.next, self.timeline.pending_switch())
                    && switch < at + len as u64
                {
                    len = (switch - at) as usize;
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
            if matches!(command, Command::SetPattern(_)) && self.free_slots() < 2 {
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
                Command::SetBpm(bpm) => self.timeline.set_bpm(at, bpm),
                Command::SetPattern(update) => self.set_pattern(at, update),
            }
        }
    }

    fn play(&mut self, at: u64) {
        if !self.playing {
            self.timeline.restart(&self.current.update.pattern, at);
            self.playing = true;
        }
    }

    fn stop(&mut self, at: u64) {
        if !self.playing {
            return;
        }
        self.stopped_at = self.timeline.position_at(at);
        self.playing = false;
        self.voices = [Voice::IDLE; VOICES];
        // No need to wait for the "one" any more.
        if let Some(next) = self.next.take() {
            let old = std::mem::replace(&mut self.current, next);
            self.retire(old);
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
            ..
        } = self;
        for voice in voices.iter_mut().filter(|v| v.active) {
            let clip = clip_of(current, retired, voice);
            play(voice, clip, &mut mix[..len]);
        }
    }

    /// Starts the clicks of `[at, at + len)` and reports them as events.
    fn schedule(&mut self, at: u64, len: usize, buffer_start: u64, buffer_time: Instant) {
        self.hits.clear();
        let pattern = &self.current.update.pattern;
        self.timeline
            .events_in(pattern, at, at + len as u64, &mut Bounded(&mut self.hits));
        for i in 0..self.hits.len() {
            let hit = self.hits[i];
            let pulse = pattern
                .pulses
                .iter()
                .position(|p| p.id == hit.pulse)
                .unwrap_or(usize::MAX);
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
            let clip = clip_of(&self.current, &self.retired, &self.voices[slot]);
            let offset = (hit.frame - at) as usize;
            play(&mut self.voices[slot], clip, &mut self.mix[offset..len]);
            let since = (hit.frame - buffer_start) * 1_000_000_000 / u64::from(self.sample_rate);
            let event = BeatEvent {
                pulse: hit.pulse,
                beat: hit.beat,
                frame: hit.frame,
                at: buffer_time + Duration::from_nanos(since),
            };
            if self.events.push(event).is_err() {
                self.dropped_events.fetch_add(1, Ordering::Relaxed);
            }
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

/// The clip `voice` plays, from the bank it was started with.
fn clip_of<'a>(current: &'a Owned, retired: &'a [Option<Owned>], voice: &Voice) -> &'a [f32] {
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

/// Unity below the knee, then a tanh bend: continuous, slope 1 at the knee,
/// never above 1.0.
fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let bent = KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh();
    bent.min(1.0).copysign(x)
}
