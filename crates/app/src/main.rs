//! Native macOS polyrhythmic metronome: the eframe app that joins the audio
//! service and the egui views.
//!
//! Threads: the UI (main) thread owns the [`AudioService`], the views and
//! the settings. A "ui-tick" thread asks for a repaint every 250 ms, so
//! [`App::logic`](eframe::App::logic) keeps running while the window is
//! hidden (eframe then runs only `logic`, no egui pass): the service is
//! ticked (device following, garbage), beat events are drained and the
//! settings saved even in the background.
//!
//! The menu bar item and the global hotkeys are set up on the first `logic`
//! call (macOS wants them on the main thread with the event loop running);
//! their events come through a channel as [`Remote`]s, drained in `logic`,
//! so they work while the window is hidden.

mod controller;
mod hotkeys;
mod remote;
mod settings;
mod tray;
mod windows;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, Sense, ViewportCommand};
use pm_audio::{AudioService, DeviceInfo, EngineInit};

// Debug builds abort on an allocation inside the audio callback (the
// `assert_no_alloc` guard in `pm_audio`'s data callback); release builds use
// the system allocator unchanged.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
use pm_ui::state::{UiAction, UiState, WindowMode};
use pm_ui::theme;
use pm_ui::views::footer::MAX_VISUAL_OFFSET_MS;
use pm_ui::views::{full::full_view, mini::mini_view};

use controller::Controller;
use remote::Remote;
use settings::Settings;
use tray::Tray;
use windows::Windows;

/// How often the audio service is ticked (and the hidden app woken).
const TICK: Duration = Duration::from_millis(250);
/// A tick is due this much before [`TICK`] has passed: the ticker sleeps
/// exactly `TICK`, so without slack a wake-up a hair early skips a tick.
const TICK_SLACK: Duration = Duration::from_millis(50);
/// Settings are written this long after the last change.
const SAVE_DELAY: Duration = Duration::from_secs(1);

struct App {
    audio: AudioService,
    ctl: Controller,
    windows: Windows,
    /// Visual offset (ms) per device UID, as the user set it.
    visual_offsets: BTreeMap<String, u32>,
    /// UID of the output the visual offset was last chosen for.
    device_uid: Option<String>,
    /// Rebuild decisions already reported on stderr.
    rebuilds_reported: u64,
    settings_dir: Option<PathBuf>,
    /// Last settings written; the latest ones and when they last changed.
    saved: Settings,
    pending: (Settings, Instant),
    last_tick: Instant,
    last_frame: Instant,
    /// Menu bar item and hotkeys: built on the first `logic` call, kept
    /// alive here (dropping them removes them).
    started: bool,
    tray: Option<Tray>,
    _hotkeys: Option<global_hotkey::GlobalHotKeyManager>,
    remote_tx: Sender<Remote>,
    remote_rx: Receiver<Remote>,
    /// "Sair" was chosen: let the next close request through.
    quitting: bool,
}

impl App {
    fn new(audio: AudioService, settings: Settings, settings_dir: Option<PathBuf>) -> App {
        let now = Instant::now();
        let (remote_tx, remote_rx) = mpsc::channel();
        let mut ui = UiState::default();
        ui.pattern = settings.pattern.clone();
        ui.bpm = settings.bpm;
        ui.mode = settings.mode;
        let mut app = App {
            audio,
            ctl: Controller::new(ui),
            windows: Windows::new(settings.mode, settings.full_rect, settings.mini_rect),
            visual_offsets: settings.visual_offsets.clone(),
            device_uid: None,
            rebuilds_reported: 0,
            settings_dir,
            saved: settings.clone(),
            pending: (settings, now),
            last_tick: now,
            last_frame: now,
            started: false,
            tray: None,
            _hotkeys: None,
            remote_tx,
            remote_rx,
            quitting: false,
        };
        app.sync_device();
        app
    }

    /// Device name and open error for the views and, when the output
    /// changed, its visual offset: the one saved for it, else the latency
    /// the beat times miss. Reports each stream rebuild on stderr.
    fn sync_device(&mut self) {
        let decided = self.audio.rebuild_reasons().total();
        if decided != self.rebuilds_reported {
            self.rebuilds_reported = decided;
            if let Some(reason) = self.audio.last_rebuild_reason() {
                eprintln!("polyrhythmic-metronome: audio stream rebuilt: {reason}");
            }
            if let Some(e) = self.audio.last_error() {
                eprintln!("polyrhythmic-metronome: {e}");
            }
        }
        self.ctl.ui.audio_error = self.audio.last_error().map(ToString::to_string);
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
            mode: self.windows.mode(),
            full_rect: self.windows.full_rect,
            mini_rect: self.windows.mini_rect,
            visual_offsets: self.visual_offsets.clone(),
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

    fn save(&mut self) {
        let current = self.settings();
        if let Some(dir) = &self.settings_dir
            && let Err(e) = current.save(dir)
        {
            eprintln!("polyrhythmic-metronome: could not save settings: {e}");
        }
        self.pending.0 = current.clone();
        self.saved = current;
    }

    /// Builds the menu bar item and registers the hotkeys. Either may fail
    /// (reported on stderr); the app runs on without it.
    fn start_remotes(&mut self, ctx: &egui::Context) {
        tray::forward(self.remote_tx.clone(), ctx.clone());
        hotkeys::forward(self.remote_tx.clone(), ctx.clone());
        match Tray::build(self.ctl.ui.playing) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => eprintln!("polyrhythmic-metronome: no menu bar item: {e}"),
        }
        match hotkeys::register_hotkeys() {
            Ok(manager) => self._hotkeys = Some(manager),
            Err(e) => eprintln!("polyrhythmic-metronome: no global hotkeys: {e}"),
        }
    }

    /// A request from the menu bar item or a hotkey: window ones here, the
    /// others through the views' actions.
    fn remote(&mut self, ctx: &egui::Context, remote: Remote, now: Instant) {
        if remote == Remote::Quit {
            self.save();
            self.quitting = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
            ctx.request_repaint();
        } else if let Some(mode) = remote::target_mode(remote, self.windows.mode()) {
            self.show(ctx, mode, now);
        } else {
            for action in remote::view_actions(&mut self.ctl.ui, remote) {
                self.handle(ctx, action, now);
            }
        }
    }

    /// Shows the window in `mode`. The full window also takes focus; the
    /// mini one floats on top without taking it from the app in use.
    fn show(&mut self, ctx: &egui::Context, mode: WindowMode, now: Instant) {
        if self.windows.hidden() {
            // Nothing lit from before it was hidden flashes at once.
            self.ctl.ui.clear_beats();
        }
        for command in self.windows.show(mode == WindowMode::Full) {
            ctx.send_viewport_cmd(command);
        }
        if mode != self.ctl.ui.mode {
            self.ctl.ui.mode = mode;
            self.handle(ctx, UiAction::ToggleMini, now);
        }
        ctx.request_repaint();
    }

    fn handle(&mut self, ctx: &egui::Context, action: UiAction, now: Instant) {
        match action {
            UiAction::ToggleMini => {
                // The view already flipped `ui.mode`.
                for command in self.windows.switch(self.ctl.ui.mode, now) {
                    ctx.send_viewport_cmd(command);
                }
            }
            UiAction::SetVisualOffset(offset) => {
                if let Some(uid) = &self.device_uid {
                    let ms = u32::try_from(offset.as_millis()).unwrap_or(u32::MAX);
                    self.visual_offsets
                        .insert(uid.clone(), ms.min(MAX_VISUAL_OFFSET_MS));
                }
            }
            other => self.ctl.apply(&mut self.audio, other),
        }
    }
}

impl eframe::App for App {
    /// Runs before every frame and, while the window is hidden, on every
    /// wake-up of the tick thread.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        if !self.started {
            self.started = true;
            self.start_remotes(ctx);
        }
        if tick_due(&mut self.last_tick, now) {
            self.audio.tick();
            self.sync_device();
        }
        let handle = self.audio.handle();
        // A parked SetPattern holds back every later command (Task 7).
        while handle.garbage.pop().is_ok() {}
        while let Ok(event) = handle.events.pop() {
            // Nothing advances while hidden: keep no backlog.
            if !self.windows.hidden() {
                self.ctl.ui.on_beat(&event);
            }
        }
        self.ctl.ui.underruns = handle.underruns.load(Ordering::Relaxed);
        let remotes: Vec<Remote> = self.remote_rx.try_iter().collect();
        for remote in remotes {
            self.remote(ctx, remote, now);
        }
        self.ctl.flush(&mut self.audio);
        if let Some(tray) = &mut self.tray {
            tray.set_playing(self.ctl.ui.playing);
        }

        if !self.quitting && ctx.input(|i| i.viewport().close_requested()) {
            for command in self.windows.hide() {
                ctx.send_viewport_cmd(command);
            }
        }
        self.save_when_settled(now);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.ctl.ui.advance(now, dt);

        // The window has no title bar: drag it by any empty spot. Widgets
        // drawn after this sit on top of it and keep their own drags.
        let window = ui.max_rect();
        let background = ui.interact(window, ui.make_persistent_id("window-drag"), Sense::drag());
        if background.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }

        let actions = match self.windows.mode() {
            WindowMode::Full => full_view(ui, &mut self.ctl.ui, now),
            WindowMode::Mini => mini_view(ui, &mut self.ctl.ui, now),
        };
        let ctx = ui.ctx().clone();
        for action in actions {
            self.handle(&ctx, action, now);
        }
        let info = ctx.input(|i| i.viewport().clone());
        self.windows.track(&info, now);

        if self.ctl.ui.playing || self.ctl.ui.animating() {
            ctx.request_repaint();
        }
    }

    /// ⌘Q (or "Sair"): write the settings now.
    fn on_exit(&mut self) {
        self.save();
    }

    /// Transparent, so the mini window's rounded card shows its corners.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }
}

/// The visual offset for `device`: the one the user set for it, else the
/// part of its latency that the beat events' times do not include yet (at
/// most the slider's maximum). cpal's `playback` time, which stamps the
/// events, already counts the buffer, device latency and safety offset;
/// only the stream latency is left (see [`pm_audio::OutputLatency`]).
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

/// Wakes the app every [`TICK`] even while its window is hidden.
fn spawn_ticker(ctx: egui::Context) {
    let spawned = std::thread::Builder::new()
        .name("ui-tick".to_owned())
        .spawn(move || {
            loop {
                std::thread::sleep(TICK);
                ctx.request_repaint();
            }
        });
    if let Err(e) = spawned {
        eprintln!("polyrhythmic-metronome: no tick thread, a hidden window stops ticking: {e}");
    }
}

fn main() -> eframe::Result {
    let settings_dir = settings::default_dir();
    let settings = settings_dir
        .as_deref()
        .map(Settings::load)
        .unwrap_or_default();
    let audio = match AudioService::start(EngineInit {
        pattern: settings.pattern.clone(),
        bpm: settings.bpm,
        resume: None,
    }) {
        Ok(audio) => audio,
        Err(e) => {
            eprintln!("polyrhythmic-metronome: {e}");
            std::process::exit(1);
        }
    };
    let viewport = Windows::new(settings.mode, settings.full_rect, settings.mini_rect).viewport();
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Polyrhythmic Metronome",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            spawn_ticker(cc.egui_ctx.clone());
            Ok(Box::new(App::new(audio, settings, settings_dir)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use pm_audio::OutputLatency;

    use super::*;

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
