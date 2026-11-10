//! The engine on a real output: a cpal stream on the macOS default output
//! device, rebuilt without losing the musical position when the device
//! changes (headphones, Bluetooth), when the stream fails, or after sleep.
//!
//! [`AudioService`] lives on the UI thread. Commands go through
//! [`AudioService::send`], which records what is playing in a [`Transport`];
//! [`AudioService::tick`] watches the device and rebuilds the stream from
//! that record and the engine's last [`PositionSnapshot`].

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, ErrorKind, OutputCallbackInfo, StreamConfig, StreamInstant};
use pm_core::session::{OnFinish, SessionPlan};
use pm_core::{Bpm, MusicalPosition, Pattern};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::engine::{Engine, EngineHandle, EngineInit};
use crate::messages::{Command, PositionSnapshot};
use crate::session::SessionReader;
use crate::watch::{self, Observation, RebuildCounts, RebuildReason, WatchState};

// The engine is built on the UI thread and moved into the audio callback.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Engine>()
};

/// Buffer size asked for first; the device default is used if refused.
const PREFERRED_BUFFER: u32 = 512;
/// Pause between attempts to open an output while there is none.
const RETRY: Duration = Duration::from_secs(1);
/// Sample rate of the placeholder engine that holds the position while
/// there is no output device.
const IDLE_SAMPLE_RATE: u32 = 48_000;

/// The output the service is playing on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// CoreAudio device UID (stable across runs and reconnections).
    pub uid: String,
    pub name: String,
    pub sample_rate: u32,
    /// What CoreAudio reports for it (zero if the query failed); see
    /// [`query_output_latency`].
    pub latency: OutputLatency,
}

/// Output latency of a device, as CoreAudio reports it.
///
/// cpal 0.18.2 already puts the device's buffer size, `device_and_safety`
/// and nothing else into `OutputCallbackInfo::timestamp().playback` (see
/// `cpal::host::coreaudio::macos::device`, `build_output_stream_raw`: the
/// render callback adds `device_buffer_frames + kAudioDevicePropertyLatency
/// + kAudioDevicePropertySafetyOffset` to the callback time), and the engine
/// stamps beat events with that `playback` time. Only `stream` is missing
/// from the events' times: [`uncompensated`](Self::uncompensated).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputLatency {
    /// `kAudioDevicePropertyLatency` + `kAudioDevicePropertySafetyOffset`
    /// (output scope).
    pub device_and_safety: Duration,
    /// `kAudioStreamPropertyLatency` of the first output stream.
    pub stream: Duration,
}

impl OutputLatency {
    /// Everything CoreAudio reports.
    pub fn total(&self) -> Duration {
        self.device_and_safety + self.stream
    }

    /// The part cpal does not include in `playback`, so beat events come
    /// this much early: the default visual offset.
    pub fn uncompensated(&self) -> Duration {
        self.stream
    }
}

/// A stream could not be opened on an existing output device.
#[derive(Debug)]
pub struct AudioError(pub cpal::Error);

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "audio output: {}", self.0)
    }
}

impl std::error::Error for AudioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Consumer end of the output tap: a copy of what the data callback
/// rendered (first channel of each frame), for tools that analyse the real
/// output, such as `tools/stress`. See [`AudioService::start_with_tap`].
pub struct TapRx {
    /// Rendered samples, starting at the engine's frame 0.
    pub samples: Consumer<f32>,
    dropped: Arc<AtomicU64>,
}

impl TapRx {
    /// Frames the callback could not copy because the ring was full. Not
    /// zero means `samples` has gaps and cannot be aligned to engine frames.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Producer end, in the data callback: preallocated, never allocates.
struct TapTx {
    samples: Producer<f32>,
    dropped: Arc<AtomicU64>,
}

impl TapTx {
    fn push(&mut self, data: &[f32], channels: usize) {
        let frames = data.len() / channels;
        match self.samples.write_chunk_uninit(frames) {
            Ok(chunk) => {
                chunk.fill_from_iter(data.chunks_exact(channels).map(|frame| frame[0]));
            }
            Err(_) => {
                self.dropped.fetch_add(frames as u64, Ordering::Relaxed);
            }
        }
    }
}

fn new_tap(capacity: usize) -> (TapTx, TapRx) {
    let (samples, consumer) = RingBuffer::new(capacity);
    let dropped = Arc::new(AtomicU64::new(0));
    (
        TapTx {
            samples,
            dropped: Arc::clone(&dropped),
        },
        TapRx {
            samples: consumer,
            dropped,
        },
    )
}

/// The first error a stream's error callback reported, as one atomic (the
/// callback may run on a CoreAudio thread).
#[derive(Default)]
struct ErrorSlot(AtomicU8);

/// Error kinds by code - 1 (0 is "no error"). Unknown kinds map to `Other`.
const KINDS: [ErrorKind; 14] = [
    ErrorKind::DeviceBusy,
    ErrorKind::DeviceChanged,
    ErrorKind::DeviceNotAvailable,
    ErrorKind::HostUnavailable,
    ErrorKind::InvalidInput,
    ErrorKind::PermissionDenied,
    ErrorKind::RealtimeDenied,
    ErrorKind::ResourceExhausted,
    ErrorKind::StreamInvalidated,
    ErrorKind::UnsupportedConfig,
    ErrorKind::UnsupportedOperation,
    ErrorKind::Xrun,
    ErrorKind::BackendError,
    ErrorKind::Other,
];

impl ErrorSlot {
    /// Keeps `kind` unless an earlier error is already kept.
    fn set(&self, kind: ErrorKind) {
        let index = KINDS
            .iter()
            .position(|&k| k == kind)
            .unwrap_or(KINDS.len() - 1);
        let _ = self
            .0
            .compare_exchange(0, index as u8 + 1, Ordering::Relaxed, Ordering::Relaxed);
    }

    fn get(&self) -> Option<ErrorKind> {
        match self.0.load(Ordering::Relaxed) {
            0 => None,
            code => KINDS.get(usize::from(code) - 1).copied(),
        }
    }
}

/// What the UI asked the engine to play: the latest BPM, pattern, session
/// plan and play/stop sent. A rebuilt engine starts from it.
#[derive(Clone, Debug)]
pub struct Transport {
    pattern: Pattern,
    bpm: Bpm,
    playing: bool,
    /// The plan the next `Play` runs; while `running`, the one running.
    plan: Option<SessionPlan>,
    /// A session runs since the last `Play`.
    running: bool,
    /// A plan sent while `running`, as the engine holds it until `Stop`.
    queued: Option<Option<SessionPlan>>,
}

impl Transport {
    /// The state an engine built from `init` starts in.
    pub fn new(init: &EngineInit) -> Transport {
        let running = init.session.as_ref().is_some_and(|(_, progress)| {
            init.resume
                .is_some_and(|pos| progress.is_some() || pos.ticks_per_measure == 0)
        });
        Transport {
            pattern: init.pattern.clone(),
            bpm: init.bpm,
            playing: init.resume.is_some(),
            plan: init.session.as_ref().map(|(plan, _)| (**plan).clone()),
            running,
            queued: None,
        }
    }

    /// Notes `command`, sent to the engine. A session plan is cloned here,
    /// on the UI thread.
    pub fn record(&mut self, command: &Command) {
        match command {
            Command::Play => {
                if !self.playing {
                    self.playing = true;
                    self.running = self.plan.is_some();
                }
            }
            Command::Stop => {
                if self.playing {
                    self.playing = false;
                    if std::mem::take(&mut self.running)
                        && let Some(plan) = self.queued.take()
                    {
                        self.plan = plan;
                    }
                }
            }
            Command::SetBpm(bpm) => self.bpm = *bpm,
            Command::SetPattern(update) => self.pattern = update.pattern.clone(),
            Command::Session(plan) => {
                let plan = plan.as_deref().cloned();
                if self.running {
                    self.queued = Some(plan);
                } else {
                    self.plan = plan;
                }
            }
        }
    }

    /// How to build the engine that replaces one whose last published
    /// position is `snapshot`: the last BPM and pattern sent and, if playing,
    /// `resume` at the old engine's position. A `Play` the old engine had
    /// not taken yet starts the new one at the "one".
    ///
    /// The old engine must not render after `snapshot` (drop its stream
    /// first), or the frames it renders in between are played twice.
    pub fn resume_init(&self, snapshot: PositionSnapshot) -> EngineInit {
        let resume = match (self.playing, snapshot.playing) {
            (false, _) => None,
            (true, true) => Some(snapshot.position),
            // `Play` not taken yet: start at the one, as `Play` does.
            (true, false) => Some(MusicalPosition::start_of(0)),
        };
        EngineInit {
            pattern: self.pattern.clone(),
            bpm: self.bpm,
            resume,
            session: self.plan.clone().map(|plan| (Box::new(plan), None)),
        }
    }

    /// [`resume_init`](Self::resume_init) that also continues the session
    /// the old engine runs, from the latest progress in `session` (its
    /// `EngineHandle::session`): same stage, bar and tempo, and the time
    /// since that progress was taken still counts. A session that finished
    /// with [`OnFinish::Stop`] stays stopped.
    ///
    /// While the session runs, a plan sent for the next `Play` is not in the
    /// init: send [`queued_session`](Self::queued_session) to the new engine.
    pub fn resume_init_with(
        &self,
        snapshot: PositionSnapshot,
        session: &SessionReader,
    ) -> EngineInit {
        let latest = session.latest_at();
        let mut init = self.resume_init(snapshot);
        if !self.running {
            return init;
        }
        let finished_stop = !snapshot.playing
            && latest.is_some_and(|(_, p)| p.finished)
            && self
                .plan
                .as_ref()
                .is_some_and(|p| p.on_finish == OnFinish::Stop);
        if finished_stop {
            // The engine stopped by itself and took the queued plan.
            let plan = self.queued.clone().unwrap_or_else(|| self.plan.clone());
            init.resume = None;
            init.session = plan.map(|plan| (Box::new(plan), None));
            return init;
        }
        if let (Some(_), true, Some((frame, mut progress)), Some((_, slot))) =
            (init.resume, snapshot.playing, latest, init.session.as_mut())
        {
            // Date the progress at the seam, the new engine's frame 0.
            let since = snapshot.frame.saturating_sub(frame);
            progress.frames_in_stage = progress.frames_in_stage.saturating_add(since);
            progress.total_frames = progress.total_frames.saturating_add(since);
            init.bpm = progress.moment.bpm;
            *slot = Some(progress);
        }
        init
    }

    /// The `Session` command a rebuilt engine needs while a session runs: a
    /// plan sent during the run, waiting for the next `Play`.
    pub fn queued_session(&self) -> Option<Command> {
        let plan = self.queued.as_ref().filter(|_| self.running)?;
        Some(Command::Session(plan.clone().map(Box::new)))
    }
}

/// Runs the engine on the default output device and keeps it there.
pub struct AudioService {
    host: cpal::Host,
    transport: Transport,
    handle: EngineHandle,
    /// `resume` of the init the current handle's engine was built with: its
    /// exact position while it has rendered nothing.
    origin: Option<MusicalPosition>,
    live: Option<Live>,
    /// Ring size (samples) of the output tap, if one was asked for.
    tap_capacity: Option<usize>,
    rebuilds: u64,
    /// Why the stream was last replaced or dropped, and how often for each
    /// reason.
    last_reason: Option<RebuildReason>,
    reasons: RebuildCounts,
    /// Why the last attempt to open a stream failed, until one succeeds.
    last_error: Option<AudioError>,
    next_attempt: Instant,
    /// Monotonic and wall clock at the last tick, to notice sleep.
    clocks: (Instant, SystemTime),
}

/// An open stream and what watches it.
struct Live {
    stream: cpal::Stream,
    /// Tells the data callback to render nothing more (set before a rebuild).
    halted: Arc<AtomicBool>,
    info: DeviceInfo,
    tap: Option<TapRx>,
    /// Set by the error callback: rebuild.
    error: Arc<ErrorSlot>,
    /// Data callbacks so far.
    callbacks: Arc<AtomicU64>,
    /// What `tick` remembers about this stream.
    watch: WatchState,
}

impl AudioService {
    /// Opens the default output and starts `init` on it. Without an output
    /// device this still succeeds, with no stream: [`tick`](Self::tick)
    /// keeps trying and [`device_info`](Self::device_info) is `None`. Fails
    /// only if a present device refuses every stream configuration.
    pub fn start(init: EngineInit) -> Result<AudioService, AudioError> {
        Self::start_inner(init, None)
    }

    /// Like [`start`](Self::start), and the data callback also copies the
    /// first channel of everything it renders into a preallocated ring of
    /// `tap_samples` samples, read through [`tap`](Self::tap). Meant for
    /// measurement tools: the ring is allocated here, the callback only
    /// writes into it, and a full ring counts the lost frames instead of
    /// blocking. A rebuilt stream gets a fresh ring (see
    /// [`rebuilds`](Self::rebuilds)).
    pub fn start_with_tap(
        init: EngineInit,
        tap_samples: usize,
    ) -> Result<AudioService, AudioError> {
        Self::start_inner(init, Some(tap_samples))
    }

    fn start_inner(
        init: EngineInit,
        tap_capacity: Option<usize>,
    ) -> Result<AudioService, AudioError> {
        let host = cpal::default_host();
        let transport = Transport::new(&init);
        let now = Instant::now();
        let mut last_error = None;
        let (live, handle) = match open(&host, &init, tap_capacity) {
            Ok(Some((live, handle))) => (Some(live), handle),
            Ok(None) => (None, placeholder(&init)),
            Err(OpenError::Transient(e)) => {
                last_error = Some(AudioError(e));
                (None, placeholder(&init))
            }
            Err(OpenError::Fatal(e)) => return Err(AudioError(e)),
        };
        Ok(AudioService {
            host,
            transport,
            handle,
            origin: init.resume,
            live,
            tap_capacity,
            rebuilds: 0,
            last_reason: None,
            reasons: RebuildCounts::default(),
            last_error,
            next_attempt: now + RETRY,
            clocks: (now, SystemTime::now()),
        })
    }

    /// Sends `command` to the engine and records it for rebuilds. All
    /// commands must go through here, not through `handle().commands`.
    /// Gives the command back if the queue is full. Without an output it is
    /// only recorded.
    pub fn send(&mut self, command: Command) -> Result<(), Command> {
        if self.live.is_some() && self.handle.commands.slots() == 0 {
            return Err(command);
        }
        self.transport.record(&command);
        if self.live.is_some() {
            // We are the only producer: there is room.
            let pushed = self.handle.commands.push(command);
            debug_assert!(pushed.is_ok());
        }
        Ok(())
    }

    /// The engine's events, garbage, position and counters. Replaced by a
    /// new handle on every rebuild (counters carry over).
    pub fn handle(&mut self) -> &mut EngineHandle {
        &mut self.handle
    }

    /// The output tap asked for with [`start_with_tap`](Self::start_with_tap)
    /// (`None` without one, or while there is no output).
    pub fn tap(&mut self) -> Option<&mut TapRx> {
        self.live.as_mut().and_then(|live| live.tap.as_mut())
    }

    /// Streams opened to replace a previous one so far (each replaces the tap).
    pub fn rebuilds(&self) -> u64 {
        self.rebuilds
    }

    /// Why `tick` last replaced or dropped the stream (or tried to open one
    /// after there was none), whether or not the new stream opened.
    pub fn last_rebuild_reason(&self) -> Option<&RebuildReason> {
        self.last_reason.as_ref()
    }

    /// How often `tick` decided to rebuild, by reason. Its
    /// [`total`](RebuildCounts::total) can exceed [`rebuilds`](Self::rebuilds):
    /// it also counts attempts that found no device or failed to open.
    pub fn rebuild_reasons(&self) -> RebuildCounts {
        self.reasons
    }

    /// Why the last attempt to open a stream failed; `None` once one opens.
    pub fn last_error(&self) -> Option<&AudioError> {
        self.last_error.as_ref()
    }

    /// The output in use, or `None` when there is none ("sem saída de
    /// áudio").
    pub fn device_info(&self) -> Option<DeviceInfo> {
        self.live.as_ref().map(|live| live.info.clone())
    }

    /// Call about 4 times a second from the UI thread. Drops replaced
    /// patterns from `garbage` (the UI should drain it every frame too: a
    /// parked `SetPattern` delays every later command, including `Stop`),
    /// and rebuilds the stream when the default output changed, the stream
    /// failed or stalled, or the machine slept (see the `watch` module for
    /// how each is confirmed first); with no output it retries.
    pub fn tick(&mut self) {
        while self.handle.garbage.pop().is_ok() {}
        while self.handle.session_garbage.pop().is_ok() {}
        let now = Instant::now();
        let wall = SystemTime::now();
        let wall_jumped = wall
            .duration_since(self.clocks.1)
            .is_ok_and(|w| w > now.duration_since(self.clocks.0) + watch::STALL);
        self.clocks = (now, wall);

        let default_uid = self
            .host
            .default_output_device()
            .and_then(|d| d.id().ok())
            .map(|id| id.id().to_owned());
        let reason = match &mut self.live {
            Some(live) => watch::rebuild_reason(
                &mut live.watch,
                Observation {
                    now,
                    wall_jumped,
                    callbacks: live.callbacks.load(Ordering::Relaxed),
                    error: live.error.get(),
                    uid: &live.info.uid,
                    default_uid,
                },
            ),
            None => default_uid.filter(|_| now >= self.next_attempt).map(|to| {
                RebuildReason::DeviceChanged {
                    from: None,
                    to: Some(to),
                }
            }),
        };
        if let Some(reason) = reason {
            self.reasons.count(&reason);
            self.last_reason = Some(reason);
            self.rebuild(now);
        }
    }

    fn rebuild(&mut self, now: Instant) {
        // Stop the old engine before reading where it is, or the frames it
        // renders after the snapshot are played again by the new one.
        // Dropping a cpal 0.18.2 CoreAudio `Stream` is not enough: it holds
        // an `Arc<Mutex<StreamInner>>` and its `Drop` only signals the
        // monitor; the AudioUnit is stopped and disposed when the last strong
        // `Arc` goes. The default-output monitor thread upgrades its weak ref
        // and holds a strong one while it handles a device change, exactly
        // when we rebuild. `pause` locks the inner unit and calls the
        // synchronous `AudioOutputUnitStop`; `halted` also silences the
        // callback should the stop fail or a callback still slip through.
        if let Some(live) = self.live.take() {
            live.halted.store(true, Ordering::Release);
            let _ = live.stream.pause();
            drop(live);
        }
        let mut snapshot = self.handle.position.snapshot();
        if snapshot.frame == 0
            && let Some(origin) = self.origin
        {
            snapshot.position = origin;
        }
        let init = self
            .transport
            .resume_init_with(snapshot, &self.handle.session);
        self.next_attempt = now + RETRY;
        let (live, mut handle) = match open(&self.host, &init, self.tap_capacity) {
            Ok(Some(opened)) => opened,
            // Keep the old handle: its position stays the last one played.
            Ok(None) => return,
            Err(OpenError::Transient(e) | OpenError::Fatal(e)) => {
                self.last_error = Some(AudioError(e));
                return;
            }
        };
        self.last_error = None;
        if let Some(command) = self.transport.queued_session() {
            // A fresh queue: there is room.
            let pushed = handle.commands.push(command);
            debug_assert!(pushed.is_ok());
        }
        let old = std::mem::replace(&mut self.handle, handle);
        for (new, old) in [
            (&self.handle.underruns, &old.underruns),
            (&self.handle.dropped_events, &old.dropped_events),
        ] {
            new.fetch_add(old.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        self.origin = init.resume;
        self.rebuilds += 1;
        self.live = Some(live);
    }
}

/// A handle whose engine never runs: holds `init`'s position until an
/// output appears.
fn placeholder(init: &EngineInit) -> EngineHandle {
    Engine::new(IDLE_SAMPLE_RATE, 2, init.clone()).1
}

enum OpenError {
    /// Worth retrying (device busy, gone, invalidated...).
    Transient(cpal::Error),
    Fatal(cpal::Error),
}

impl From<cpal::Error> for OpenError {
    fn from(e: cpal::Error) -> OpenError {
        match e.kind() {
            ErrorKind::UnsupportedConfig
            | ErrorKind::UnsupportedOperation
            | ErrorKind::PermissionDenied
            | ErrorKind::InvalidInput
            | ErrorKind::HostUnavailable => OpenError::Fatal(e),
            _ => OpenError::Transient(e),
        }
    }
}

/// Opens a stream for an engine built from `init` on the default output, at
/// the device's own sample rate. `Ok(None)`: there is no output device.
fn open(
    host: &cpal::Host,
    init: &EngineInit,
    tap_capacity: Option<usize>,
) -> Result<Option<(Live, EngineHandle)>, OpenError> {
    let Some(device) = host.default_output_device() else {
        return Ok(None);
    };
    let uid = device.id()?.id().to_owned();
    let name = device
        .description()
        .map_or_else(|_| uid.clone(), |d| d.name().to_owned());
    let supported = device.default_output_config()?;
    let sample_rate = supported.sample_rate();
    let channels = supported.channels();
    let fixed_fits = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => (*min..=*max).contains(&PREFERRED_BUFFER),
        cpal::SupportedBufferSize::Unknown => true,
    };
    let mut sizes = vec![BufferSize::Default];
    if fixed_fits {
        sizes.insert(0, BufferSize::Fixed(PREFERRED_BUFFER));
    }
    let error = Arc::new(ErrorSlot::default());
    let callbacks = Arc::new(AtomicU64::new(0));
    let halted = Arc::new(AtomicBool::new(false));
    let mut last_error = None;
    for buffer_size in sizes {
        let config = StreamConfig {
            channels,
            sample_rate,
            buffer_size,
        };
        let (engine, handle) = Engine::new(sample_rate, channels, init.clone());
        let (tap_tx, tap_rx) = match tap_capacity.map(new_tap) {
            Some((tx, rx)) => (Some(tx), Some(rx)),
            None => (None, None),
        };
        let mut callback = Callback {
            engine,
            channels: usize::from(channels.max(1)),
            sample_rate: u64::from(sample_rate.max(1)),
            last: None,
            underruns: Arc::clone(&handle.underruns),
            callbacks: Arc::clone(&callbacks),
            halted: Arc::clone(&halted),
            tap: tap_tx,
        };
        let on_error = {
            let underruns = Arc::clone(&handle.underruns);
            let error = Arc::clone(&error);
            move |e: cpal::Error| match e.kind() {
                ErrorKind::Xrun => {
                    underruns.fetch_add(1, Ordering::Relaxed);
                }
                // The default-output unit rerouted by itself; `tick` sees the
                // new device and rebuilds at its sample rate.
                ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {}
                kind => error.set(kind),
            }
        };
        match device.build_output_stream::<f32, _, _>(
            config,
            move |data, info| callback.run(data, info),
            on_error,
            None,
        ) {
            Ok(stream) => {
                stream.play()?;
                let info = DeviceInfo {
                    latency: query_output_latency(&uid).unwrap_or_default(),
                    uid,
                    name,
                    sample_rate,
                };
                let live = Live {
                    stream,
                    halted,
                    info,
                    tap: tap_rx,
                    error,
                    callbacks,
                    watch: WatchState::new(Instant::now()),
                };
                return Ok(Some((live, handle)));
            }
            Err(e) if e.kind() == ErrorKind::UnsupportedConfig => last_error = Some(e),
            Err(e) => return Err(e.into()),
        }
    }
    Err(last_error
        .unwrap_or_else(|| ErrorKind::UnsupportedConfig.into())
        .into())
}

/// State of the data callback, on the audio thread.
struct Callback {
    engine: Engine,
    channels: usize,
    sample_rate: u64,
    /// Time and length (frames) of the previous callback.
    last: Option<(StreamInstant, u64)>,
    underruns: Arc<AtomicU64>,
    callbacks: Arc<AtomicU64>,
    /// Set when the service replaces this stream: output silence, render
    /// nothing, so the engine's position stays where the snapshot read it.
    halted: Arc<AtomicBool>,
    /// Copy of the output for measurement tools; `None` in normal use.
    tap: Option<TapTx>,
}

impl Callback {
    /// Renders `data`. Only atomics and the engine: no allocation or lock;
    /// debug builds abort on an allocation (with `AllocDisabler` as the
    /// global allocator, as the app and `tools/stress` install it).
    fn run(&mut self, data: &mut [f32], info: &OutputCallbackInfo) {
        #[cfg(debug_assertions)]
        assert_no_alloc::assert_no_alloc(|| self.render(data, info));
        #[cfg(not(debug_assertions))]
        self.render(data, info);
    }

    fn render(&mut self, data: &mut [f32], info: &OutputCallbackInfo) {
        if self.halted.load(Ordering::Acquire) {
            data.fill(0.0);
            return;
        }
        let ts = info.timestamp();
        let frames = (data.len() / self.channels) as u64;
        // Counts late callbacks only. A stall is the service's business: it
        // watches `callbacks`, which does not depend on CoreAudio's host
        // time (a bogus `mHostTime` would fake a gap here).
        if let Some((prev, prev_frames)) = self.last {
            let gap = ts.callback.duration_since(prev);
            let expected = Duration::from_nanos(prev_frames * 1_000_000_000 / self.sample_rate);
            if gap > expected * 3 / 2 {
                self.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.last = Some((ts.callback, frames));
        self.engine
            .render(data, ts.playback.duration_since(ts.callback));
        if let Some(tap) = &mut self.tap {
            tap.push(data, self.channels);
        }
        self.callbacks.fetch_add(1, Ordering::Relaxed);
    }
}

/// Output latency of the device with CoreAudio UID `device_uid`:
/// `kAudioDevicePropertyLatency` + `kAudioDevicePropertySafetyOffset` (output
/// scope) and `kAudioStreamPropertyLatency` of its first output stream, in
/// frames at the device's nominal sample rate. `None` if the device is not
/// found or a query fails. The only `unsafe` code in the workspace.
pub fn query_output_latency(device_uid: &str) -> Option<OutputLatency> {
    use std::ffi::c_void;
    use std::ptr::{self, NonNull};

    use objc2_core_audio::{
        AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
        AudioObjectPropertyScope, AudioObjectPropertySelector, kAudioDevicePropertyLatency,
        kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
        kAudioDevicePropertyStreams, kAudioHardwarePropertyTranslateUIDToDevice,
        kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject, kAudioObjectUnknown,
        kAudioStreamPropertyLatency,
    };
    use objc2_core_foundation::CFString;

    /// Reads a property into `out`; returns the bytes written.
    fn get(
        object: AudioObjectID,
        selector: AudioObjectPropertySelector,
        scope: AudioObjectPropertyScope,
        qualifier: Option<&CFString>,
        out: NonNull<c_void>,
        capacity: u32,
    ) -> Option<u32> {
        let address = AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: scope,
            mElement: kAudioObjectPropertyElementMain,
        };
        let qualifier_ref: *const CFString = qualifier.map_or(ptr::null(), ptr::from_ref);
        let (qualifier_size, qualifier_data) = if qualifier.is_some() {
            (
                size_of::<*const CFString>() as u32,
                ptr::from_ref(&qualifier_ref).cast::<c_void>(),
            )
        } else {
            (0, ptr::null())
        };
        let mut size = capacity;
        // SAFETY: `address` and `size` live across the call, `out` points to
        // `capacity` writable bytes (the caller's local), and the qualifier
        // is null or a pointer to a live CFStringRef, as
        // kAudioHardwarePropertyTranslateUIDToDevice expects.
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&address),
                qualifier_size,
                qualifier_data,
                NonNull::from(&mut size),
                out,
            )
        };
        (status == 0).then_some(size)
    }
    let u32_of = |object, selector, scope, qualifier| {
        let mut v: u32 = 0;
        let n = get(
            object,
            selector,
            scope,
            qualifier,
            NonNull::from(&mut v).cast(),
            size_of::<u32>() as u32,
        )?;
        (n == size_of::<u32>() as u32).then_some(v)
    };

    let uid = CFString::from_str(device_uid);
    let device = u32_of(
        kAudioObjectSystemObject as AudioObjectID,
        kAudioHardwarePropertyTranslateUIDToDevice,
        kAudioObjectPropertyScopeGlobal,
        Some(&uid),
    )
    .filter(|&id| id != kAudioObjectUnknown)?;
    let output = kAudioObjectPropertyScopeOutput;
    let device_latency = u32_of(device, kAudioDevicePropertyLatency, output, None)?;
    let safety_offset = u32_of(device, kAudioDevicePropertySafetyOffset, output, None)?;
    let mut streams = [0 as AudioObjectID; 16];
    let bytes = get(
        device,
        kAudioDevicePropertyStreams,
        output,
        None,
        NonNull::from(&mut streams).cast(),
        size_of_val(&streams) as u32,
    )?;
    let stream_latency = if bytes >= size_of::<AudioObjectID>() as u32 {
        u32_of(
            streams[0],
            kAudioStreamPropertyLatency,
            kAudioObjectPropertyScopeGlobal,
            None,
        )?
    } else {
        0
    };
    let mut rate: f64 = 0.0;
    let n = get(
        device,
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
        None,
        NonNull::from(&mut rate).cast(),
        size_of::<f64>() as u32,
    )?;
    if n != size_of::<f64>() as u32 || !rate.is_finite() || rate <= 0.0 {
        return None;
    }
    let seconds = |frames: f64| Duration::from_secs_f64(frames / rate);
    Some(OutputLatency {
        device_and_safety: seconds(f64::from(device_latency) + f64::from(safety_offset)),
        stream: seconds(f64::from(stream_latency)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::OutputStreamTimestamp;

    #[cfg(debug_assertions)]
    #[global_allocator]
    static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

    fn at(ms: u64) -> OutputCallbackInfo {
        let callback = StreamInstant::from_nanos(ms * 1_000_000);
        OutputCallbackInfo::new(OutputStreamTimestamp {
            callback,
            playback: StreamInstant::from_nanos(ms * 1_000_000 + 5_000_000),
        })
    }

    /// 480 stereo frames at 48 kHz are 10 ms: a 14 ms gap is fine, a 16 ms
    /// one an underrun, and so is a gap over a second (stalls are the
    /// service's `callbacks` watchdog, not this). All without allocating.
    #[test]
    fn callback_counts_gaps_and_never_allocates() {
        let init = EngineInit {
            pattern: Pattern::default(),
            bpm: Bpm::new(120),
            resume: Some(MusicalPosition::start_of(0)),
            session: None,
        };
        let (engine, mut handle) = Engine::new(48_000, 2, init);
        // Room for exactly 6 buffers of 480 frames.
        let (tap_tx, mut tap_rx) = new_tap(2_880);
        let mut cb = Callback {
            engine,
            channels: 2,
            sample_rate: 48_000,
            last: None,
            underruns: Arc::clone(&handle.underruns),
            callbacks: Arc::new(AtomicU64::new(0)),
            halted: Arc::new(AtomicBool::new(false)),
            tap: Some(tap_tx),
        };
        let mut out = [0.0f32; 960];
        let underruns = || handle.underruns.load(Ordering::Relaxed);
        assert_no_alloc::assert_no_alloc(|| {
            for ms in [0, 10, 20, 34] {
                cb.run(&mut out, &at(ms));
            }
        });
        assert_eq!(underruns(), 0);
        assert_no_alloc::assert_no_alloc(|| cb.run(&mut out, &at(50)));
        assert_eq!(underruns(), 1);
        assert_no_alloc::assert_no_alloc(|| cb.run(&mut out, &at(1_100)));
        assert_eq!(underruns(), 2, "a long gap is just late");
        assert_eq!(cb.callbacks.load(Ordering::Relaxed), 6);
        let events = std::iter::from_fn(|| handle.events.pop().ok()).count();
        assert!(events > 0, "the engine rendered clicks");

        // The tap holds the first channel of each of the 6 rendered buffers,
        // and nothing was dropped yet.
        assert_eq!(tap_rx.dropped(), 0);
        assert_eq!(tap_rx.samples.slots(), 2_880);
        // The ring is full: the next buffer is counted, not written, and the
        // callback still neither allocates nor blocks.
        assert_no_alloc::assert_no_alloc(|| cb.run(&mut out, &at(1_110)));
        assert_eq!(tap_rx.dropped(), 480);
        assert_eq!(tap_rx.samples.slots(), 2_880);
        let tapped: Vec<f32> = std::iter::from_fn(|| tap_rx.samples.pop().ok()).collect();
        assert!(
            tapped.iter().any(|&x| x != 0.0),
            "the tap carries the clicks"
        );

        // Halted for a rebuild: silence, and the position does not move.
        let frame = handle.position.snapshot().frame;
        cb.halted.store(true, Ordering::Release);
        out.fill(1.0);
        assert_no_alloc::assert_no_alloc(|| cb.run(&mut out, &at(1_120)));
        assert!(out.iter().all(|&x| x == 0.0));
        assert_eq!(handle.position.snapshot().frame, frame);
        assert_eq!(cb.callbacks.load(Ordering::Relaxed), 7);
    }

    #[test]
    fn error_slot_keeps_the_first_kind() {
        let slot = ErrorSlot::default();
        assert_eq!(slot.get(), None);
        for kind in KINDS {
            let one = ErrorSlot::default();
            one.set(kind);
            assert_eq!(one.get(), Some(kind));
        }
        slot.set(ErrorKind::StreamInvalidated);
        slot.set(ErrorKind::DeviceNotAvailable);
        assert_eq!(slot.get(), Some(ErrorKind::StreamInvalidated));
    }
}
