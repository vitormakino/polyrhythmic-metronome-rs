//! The platform-free frame loop: [`Shell`] and the [`AppShell`] trait hosts drive.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use pm_audio::{AudioService, BeatEvent, DeviceInfo};
use pm_core::session::SessionProgress;
use pm_ui::state::{UiAction, UiState, WindowMode};
use pm_ui::views::footer::MAX_VISUAL_OFFSET_MS;

use crate::controller::{Audio, Controller};
use crate::settings::Settings;

/// What a host drives, once per frame and on its wake-ups.
pub trait AppShell {
    /// Runs before every frame and, while the app is not on screen, on the
    /// host's periodic wake-ups (no `ui` call then).
    fn logic(&mut self, ctx: &egui::Context);
    /// Draws one frame.
    fn ui(&mut self, ui: &mut egui::Ui);
    /// The app is about to quit.
    fn on_exit(&mut self) {}
    /// The app left the screen; it may be suspended soon after.
    fn on_background(&mut self) {}
    /// What the frame is cleared to before `ui` draws.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

/// The window fields of the [`Settings`], supplied by a host that has
/// windows (see [`Shell::set_window`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowSettings {
    pub mode: WindowMode,
    /// `[x, y, width, height]` of the full window (outer position, inner size), in points.
    pub full_rect: Option<[f32; 4]>,
    /// Same for the mini window.
    pub mini_rect: Option<[f32; 4]>,
}

/// What [`Shell::logic`] reads from the output besides sending commands:
/// [`AudioService`], or a fake in tests.
pub trait Device {
    /// Device following and garbage (about 4 times a second).
    fn tick(&mut self);
    /// The output in use, `None` when there is none.
    fn device_info(&self) -> Option<DeviceInfo>;
    /// Why the last attempt to open a stream failed.
    fn last_error(&self) -> Option<String>;
    /// Why the stream was rebuilt, when a rebuild was decided since
    /// `reported` (the count of decisions already reported, moved on here).
    fn rebuild_report(&self, reported: &mut u64) -> Option<String>;
    /// Drops the engine's garbage and passes every beat event to `f`.
    fn drain(&mut self, f: &mut dyn FnMut(&BeatEvent));
    /// Output underruns so far.
    fn underruns(&mut self) -> u64;
    /// Whether the engine plays.
    fn engine_playing(&mut self) -> bool;
    /// The current run's session progress (see `AudioService::session_progress`).
    fn session_progress(&self) -> Option<SessionProgress>;
}

impl Device for AudioService {
    fn tick(&mut self) {
        AudioService::tick(self);
    }

    fn device_info(&self) -> Option<DeviceInfo> {
        AudioService::device_info(self)
    }

    fn last_error(&self) -> Option<String> {
        AudioService::last_error(self).map(ToString::to_string)
    }

    fn rebuild_report(&self, reported: &mut u64) -> Option<String> {
        let decided = self.rebuild_reasons().total();
        if decided == *reported {
            return None;
        }
        *reported = decided;
        // Every decision records its reason.
        self.last_rebuild_reason().map(ToString::to_string)
    }

    fn drain(&mut self, f: &mut dyn FnMut(&BeatEvent)) {
        let handle = self.handle();
        // A parked SetPattern or Session holds back every later command.
        while handle.garbage.pop().is_ok() {}
        while handle.session_garbage.pop().is_ok() {}
        while let Ok(event) = handle.events.pop() {
            f(&event);
        }
    }

    fn underruns(&mut self) -> u64 {
        self.handle().underruns.load(Ordering::Relaxed)
    }

    fn engine_playing(&mut self) -> bool {
        self.handle().position.snapshot().playing
    }

    fn session_progress(&self) -> Option<SessionProgress> {
        AudioService::session_progress(self)
    }
}

/// How often the audio service is ticked (and a hidden app should be woken).
pub const TICK: Duration = Duration::from_millis(250);
/// A tick is due this much before [`TICK`] has passed: a host waking the app
/// every `TICK` may be a hair early, and without slack would skip a tick.
const TICK_SLACK: Duration = Duration::from_millis(50);
/// Settings are written this long after the last change.
const SAVE_DELAY: Duration = Duration::from_secs(1);

/// The app without its platform: the views' state and the engine commands
/// ([`Controller`]), the audio service, the visual offset per output device
/// and the delayed settings saving. A host's [`AppShell`] holds one and
/// calls [`logic`](Self::logic) and [`view`](Self::view).
pub struct Shell<A: Audio = AudioService> {
    pub ctl: Controller,
    audio: A,
    /// Visual offset (ms) per device UID, as the user set it.
    visual_offsets: BTreeMap<String, u32>,
    /// UID of the output the visual offset was last chosen for.
    device_uid: Option<String>,
    /// Rebuild decisions already reported on stderr.
    rebuilds_reported: u64,
    window: WindowSettings,
    settings_dir: Option<PathBuf>,
    /// Last settings written; the latest ones and when they last changed.
    saved: Settings,
    pending: (Settings, Instant),
    last_tick: Instant,
    last_frame: Instant,
    /// Whether the app was on screen at the last `logic` or `view`.
    visible: bool,
}

impl<A: Audio + Device> Shell<A> {
    /// Starts from the saved `settings` (pattern, tempo, mode, session,
    /// topology, window, offsets) over `ui` (language, theme, platform switches);
    /// saves into `settings_dir`, if any.
    pub fn new(
        audio: A,
        mut ui: UiState,
        settings: &Settings,
        settings_dir: Option<PathBuf>,
    ) -> Self {
        let now = Instant::now();
        ui.pattern = settings.pattern.clone();
        ui.bpm = settings.bpm;
        ui.mode = settings.mode;
        ui.topology = settings.topology;
        // Kept only if the loaded sessions have it (`Controller::set_sessions`).
        ui.session = settings.session.clone();
        let mut shell = Shell {
            ctl: Controller::new(ui),
            audio,
            visual_offsets: settings.visual_offsets.clone(),
            device_uid: None,
            rebuilds_reported: 0,
            window: WindowSettings {
                mode: settings.mode,
                full_rect: settings.full_rect,
                mini_rect: settings.mini_rect,
            },
            settings_dir,
            saved: settings.clone(),
            pending: (settings.clone(), now),
            last_tick: now,
            last_frame: now,
            visible: true,
        };
        shell.sync_device();
        shell
    }

    /// Runs before every frame and on every wake-up while the app is not on
    /// screen (`visible` false): ticks the service when due, drains the beat
    /// events (lighting them only when `visible`: nothing advances while
    /// hidden, so no backlog is kept), follows the session, sends what
    /// waits and saves the settings once they settle. Back on screen,
    /// nothing lit from before the app was hidden flashes at once.
    pub fn logic(&mut self, now: Instant, visible: bool) {
        self.set_visible(visible);
        if tick_due(&mut self.last_tick, now) {
            self.audio.tick();
            self.sync_device();
        }
        let ui = &mut self.ctl.ui;
        self.audio.drain(&mut |event| {
            if visible {
                ui.on_beat(event);
            }
        });
        self.ctl.ui.underruns = self.audio.underruns();
        let engine_playing = self.audio.engine_playing();
        // The current run's progress only: a fresh Play never sees the last
        // run's `finished`.
        let progress = self.audio.session_progress();
        self.ctl
            .sync_session(&mut self.audio, progress, engine_playing);
        self.ctl.flush(&mut self.audio);
        self.save_when_settled(now);
    }

    /// One frame: advances the animations, runs `draw` (a view) and handles
    /// its actions in order, the shell's first. Returns, in their order, the
    /// ones that concern the host (today [`UiAction::ToggleMini`]), for it to
    /// handle after all of the shell's. A frame means the app is on screen
    /// (see [`logic`](Self::logic)). Asks for the next frame while playing
    /// or animating.
    pub fn view(
        &mut self,
        ui: &mut egui::Ui,
        now: Instant,
        draw: impl FnOnce(&mut egui::Ui, &mut UiState, Instant) -> Vec<UiAction>,
    ) -> Vec<UiAction> {
        self.set_visible(true);
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.ctl.ui.advance(now, dt);
        let actions = draw(ui, &mut self.ctl.ui, now);
        let rest = actions
            .into_iter()
            .filter_map(|action| self.apply(action))
            .collect();
        if self.ctl.ui.playing || self.ctl.ui.animating() {
            ui.ctx().request_repaint();
        }
        rest
    }

    /// The output, for what a host does with it beyond the frame loop (the
    /// iPhone app pauses it while stopped).
    pub fn audio_mut(&mut self) -> &mut A {
        &mut self.audio
    }

    /// Handles a view's action (from a view or a host's own controls):
    /// `SetVisualOffset` here, the engine's through the [`Controller`].
    /// Gives back the ones that concern the host (`ToggleMini`).
    pub fn apply(&mut self, action: UiAction) -> Option<UiAction> {
        match action {
            UiAction::ToggleMini => return Some(action),
            UiAction::SetVisualOffset(offset) => {
                if let Some(uid) = &self.device_uid {
                    let ms = u32::try_from(offset.as_millis()).unwrap_or(u32::MAX);
                    self.visual_offsets
                        .insert(uid.clone(), ms.min(MAX_VISUAL_OFFSET_MS));
                }
            }
            other => self.ctl.apply(&mut self.audio, other),
        }
        None
    }

    /// Clears the beats lit before the app was hidden when it comes back on
    /// screen.
    fn set_visible(&mut self, visible: bool) {
        if visible && !self.visible {
            self.ctl.ui.clear_beats();
        }
        self.visible = visible;
    }

    /// The host's windows as they are now, saved with the settings.
    pub fn set_window(&mut self, window: WindowSettings) {
        self.window = window;
    }

    /// Writes the settings now (on exit, or before the app may be suspended).
    pub fn save(&mut self) {
        let current = self.settings();
        if let Some(dir) = &self.settings_dir
            && let Err(e) = current.save(dir)
        {
            eprintln!("polyrhythmic-metronome: could not save settings: {e}");
        }
        self.pending.0 = current.clone();
        self.saved = current;
    }

    /// Device name and open error for the views and, when the output
    /// changed, its visual offset: the one saved for it, else the latency
    /// the beat times miss. Reports each stream rebuild on stderr.
    fn sync_device(&mut self) {
        if let Some(reason) = self.audio.rebuild_report(&mut self.rebuilds_reported) {
            eprintln!("polyrhythmic-metronome: audio stream rebuilt: {reason}");
            if let Some(e) = self.audio.last_error() {
                eprintln!("polyrhythmic-metronome: {e}");
            }
        }
        self.ctl.ui.audio_error = self.audio.last_error();
        let info = self.audio.device_info();
        self.ctl.ui.device_name = info.as_ref().map(|d| d.name.clone());
        let uid = info.as_ref().map(|d| d.uid.clone());
        if uid == self.device_uid {
            return;
        }
        self.device_uid = uid;
        if let Some(info) = info {
            self.ctl.ui.visual_offset = visual_offset_for(&info, &self.visual_offsets);
        }
    }

    fn settings(&self) -> Settings {
        Settings {
            pattern: self.ctl.ui.pattern.clone(),
            bpm: self.ctl.ui.bpm,
            mode: self.window.mode,
            full_rect: self.window.full_rect,
            mini_rect: self.window.mini_rect,
            visual_offsets: self.visual_offsets.clone(),
            session: self.ctl.ui.session.clone(),
            topology: self.ctl.ui.topology,
            // Set and saved by the host at startup, before the app runs.
            examples_created: self.saved.examples_created,
        }
    }

    /// Saves once the settings have not changed for [`SAVE_DELAY`].
    fn save_when_settled(&mut self, now: Instant) {
        let current = self.settings();
        if current != self.pending.0 {
            self.pending = (current, now);
        } else if self.pending.0 != self.saved && now - self.pending.1 >= SAVE_DELAY {
            self.save();
        }
    }
}

/// The visual offset for `device`: the one the user set for it, else the
/// part of its latency that the beat events' times do not include yet (at
/// most the slider's maximum). cpal's `playback` time, which stamps the
/// events, already counts the buffer and the device's latency; only
/// [`OutputLatency::stream`](pm_audio::OutputLatency::stream) is left (zero
/// on iOS; see [`pm_audio::OutputLatency`]).
fn visual_offset_for(device: &DeviceInfo, saved: &BTreeMap<String, u32>) -> Duration {
    match saved.get(&device.uid) {
        Some(&ms) => Duration::from_millis(u64::from(ms)),
        None => device
            .latency
            .uncompensated()
            .min(Duration::from_millis(u64::from(MAX_VISUAL_OFFSET_MS))),
    }
}

/// Whether the audio service is due a tick at `now`; if so `last` moves to `now`.
fn tick_due(last: &mut Instant, now: Instant) -> bool {
    if now.saturating_duration_since(*last) + TICK_SLACK >= TICK {
        *last = now;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use pm_audio::{Command, OutputLatency};
    use pm_ui::views::topology::TopologyKind;

    use super::*;

    /// Records the commands sent and hands out the queued beat events; the
    /// output is `uid` (none while `None`).
    #[derive(Default)]
    struct Fake {
        sent: Vec<String>,
        events: VecDeque<BeatEvent>,
        uid: Option<String>,
    }

    impl Audio for Fake {
        fn send(&mut self, command: Command) -> Result<(), Command> {
            self.sent.push(match &command {
                Command::Play => "Play".to_owned(),
                Command::Stop => "Stop".to_owned(),
                Command::SetBpm(b) => format!("Bpm {}", b.get()),
                Command::UserPulse(i) => format!("UserPulse {i}"),
                Command::Session(plan) => format!(
                    "Session {}",
                    plan.as_ref().map_or("None", |p| p.name.as_str())
                ),
                Command::SetPattern(u) => format!("Pattern {}", u.pattern.pulses.len()),
            });
            Ok(())
        }

        fn sample_rate(&self) -> u32 {
            48_000
        }
    }

    impl Device for Fake {
        fn tick(&mut self) {}

        fn device_info(&self) -> Option<DeviceInfo> {
            self.uid.as_ref().map(|uid| DeviceInfo {
                uid: uid.clone(),
                name: uid.clone(),
                sample_rate: 48_000,
                latency: OutputLatency::default(),
            })
        }

        fn last_error(&self) -> Option<String> {
            None
        }

        fn rebuild_report(&self, _reported: &mut u64) -> Option<String> {
            None
        }

        fn drain(&mut self, f: &mut dyn FnMut(&BeatEvent)) {
            while let Some(event) = self.events.pop_front() {
                f(&event);
            }
        }

        fn underruns(&mut self) -> u64 {
            0
        }

        fn engine_playing(&mut self) -> bool {
            false
        }

        fn session_progress(&self) -> Option<SessionProgress> {
            None
        }
    }

    fn shell(audio: Fake) -> Shell<Fake> {
        Shell::new(audio, UiState::default(), &Settings::default(), None)
    }

    /// A playing shell with two beats of pulse 0 queued, both due by `now`.
    fn with_two_beats(now: Instant) -> Shell<Fake> {
        let mut s = shell(Fake::default());
        s.ctl.ui.playing = true;
        let pulse = s.ctl.ui.pattern.pulses[0].id;
        for (beat, ago) in [(1, 20), (2, 10)] {
            s.audio.events.push_back(BeatEvent {
                pulse,
                beat,
                frame: u64::from(beat) * 100,
                at: now - Duration::from_millis(ago),
                muted: false,
            });
        }
        s
    }

    #[test]
    fn hidden_logic_drains_beats_without_lighting_them() {
        let now = Instant::now();
        let mut s = with_two_beats(now);
        let pulse = s.ctl.ui.pattern.pulses[0].id;
        s.logic(now, false);
        assert!(s.audio.events.is_empty(), "no backlog kept");
        s.ctl.ui.advance(now, 0.016);
        assert_eq!(s.ctl.ui.active_beat(pulse), None);
        assert_eq!(s.ctl.ui.pending_beats(), 0);
    }

    #[test]
    fn visible_logic_lights_beats() {
        let now = Instant::now();
        let mut s = with_two_beats(now);
        let pulse = s.ctl.ui.pattern.pulses[0].id;
        s.logic(now, true);
        assert!(s.audio.events.is_empty());
        s.ctl.ui.advance(now, 0.016);
        assert_eq!(s.ctl.ui.active_beat(pulse), Some(2));
    }

    #[test]
    fn beats_lit_before_hiding_are_cleared_on_return() {
        let now = Instant::now();
        let mut s = with_two_beats(now);
        let pulse = s.ctl.ui.pattern.pulses[0].id;
        s.logic(now, true);
        s.ctl.ui.advance(now, 0.016);
        assert_eq!(s.ctl.ui.active_beat(pulse), Some(2), "lit");
        s.logic(now, false);
        s.logic(now, true);
        s.ctl.ui.advance(now, 0.016);
        assert_eq!(s.ctl.ui.active_beat(pulse), None);
        assert_eq!(s.ctl.ui.pending_beats(), 0);
        assert_eq!(s.ctl.ui.flash(pulse), 0.0);
    }

    #[test]
    fn a_frame_after_hidden_logic_clears_the_beats() {
        // A host may draw before its next `logic` says it is visible (the
        // Mac shows its window from `logic`, after the shell's).
        let now = Instant::now();
        let mut s = with_two_beats(now);
        let pulse = s.ctl.ui.pattern.pulses[0].id;
        s.logic(now, true);
        s.ctl.ui.advance(now, 0.016);
        s.logic(now, false);
        let mut lit = None;
        view_once(&mut s, |_, ui, _| {
            lit = ui.active_beat(pulse);
            Vec::new()
        });
        assert_eq!(lit, None);
    }

    /// Runs `view` once in a bare egui pass with `draw` as the view.
    fn view_once(
        s: &mut Shell<Fake>,
        draw: impl FnOnce(&mut egui::Ui, &mut UiState, Instant) -> Vec<UiAction>,
    ) -> Vec<UiAction> {
        let ctx = egui::Context::default();
        let mut draw = Some(draw);
        let mut out = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(draw) = draw.take() {
                out = s.view(ui, Instant::now(), draw);
            }
        });
        // No renderer here to hand the font atlas to.
        output.textures_delta.clear();
        out
    }

    #[test]
    fn view_forwards_play_and_returns_toggle_mini() {
        let mut s = shell(Fake::default());
        let returned = view_once(&mut s, |_, ui, _| {
            // As the views do: flip the state, then ask.
            ui.playing = true;
            ui.mode = WindowMode::Mini;
            vec![UiAction::TogglePlay, UiAction::ToggleMini]
        });
        assert_eq!(s.audio.sent, ["Session None", "Play"]);
        assert_eq!(returned, [UiAction::ToggleMini]);
        assert!(s.ctl.ui.playing);
    }

    #[test]
    fn visual_offset_is_saved_per_device() {
        let start = Instant::now();
        let mut s = shell(Fake {
            uid: Some("a".to_owned()),
            ..Fake::default()
        });
        let offset = Duration::from_millis(30);
        let returned = view_once(&mut s, |_, ui, _| {
            ui.visual_offset = offset;
            vec![UiAction::SetVisualOffset(offset)]
        });
        assert!(returned.is_empty(), "handled by the shell");
        // Each logic call a tick later follows the output.
        s.audio.uid = Some("b".to_owned());
        s.logic(start + TICK, true);
        assert_eq!(s.ctl.ui.device_name.as_deref(), Some("b"));
        assert_eq!(s.ctl.ui.visual_offset, Duration::ZERO, "b's own latency");
        s.audio.uid = Some("a".to_owned());
        s.logic(start + 2 * TICK, true);
        assert_eq!(s.ctl.ui.visual_offset, offset);
    }

    #[test]
    fn topology_comes_from_the_settings_and_goes_back_to_them() {
        let settings = Settings {
            topology: TopologyKind::Arcs,
            ..Settings::default()
        };
        let mut s = Shell::new(Fake::default(), UiState::default(), &settings, None);
        assert_eq!(s.ctl.ui.topology, TopologyKind::Arcs);
        assert_eq!(s.settings().topology, TopologyKind::Arcs);
        s.ctl.ui.topology = TopologyKind::Hidden;
        assert_eq!(s.settings().topology, TopologyKind::Hidden);
    }

    #[test]
    fn hidden_ticker_wake_ups_each_tick() {
        let start = Instant::now();
        // The ticker sleeps TICK (250 ms) and wakes up a bit early or late.
        for every_ms in [249, 250, 253, 270] {
            let mut last = start;
            let ticks = (1..=40)
                .filter(|&k| tick_due(&mut last, start + Duration::from_millis(every_ms * k)))
                .count();
            assert_eq!(ticks, 40, "woken every {every_ms} ms");
        }
    }

    #[test]
    fn frames_do_not_tick_more_than_about_four_times_a_second() {
        let start = Instant::now();
        let mut last = start;
        let ticks = (1..=60)
            .filter(|&k| tick_due(&mut last, start + Duration::from_millis(1000 * k / 60)))
            .count();
        assert!((4..=5).contains(&ticks), "{ticks} ticks in 1 s at 60 fps");
    }

    #[test]
    fn visual_offset_is_saved_per_device_else_the_uncompensated_latency() {
        // Device latency + safety offset are already in the beat times: a
        // default counting them again would show beats late.
        let device = |uid: &str, stream_ms: u64| DeviceInfo {
            uid: uid.to_owned(),
            name: uid.to_owned(),
            sample_rate: 48_000,
            latency: OutputLatency {
                device_and_safety: Duration::from_millis(40),
                stream: Duration::from_millis(stream_ms),
            },
        };
        let saved = BTreeMap::from([("usb".to_owned(), 7)]);
        assert_eq!(
            visual_offset_for(&device("usb", 30), &saved),
            Duration::from_millis(7)
        );
        assert_eq!(
            visual_offset_for(&device("bt", 180), &saved),
            Duration::from_millis(180)
        );
        assert_eq!(
            visual_offset_for(&device("odd", 9000), &saved),
            Duration::from_millis(500)
        );
    }
}
