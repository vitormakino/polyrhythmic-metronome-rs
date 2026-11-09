//! Native macOS polyrhythmic metronome: the eframe app that joins the audio
//! service and the egui views.
//!
//! Threads: the UI (main) thread owns the [`AudioService`], the views and
//! the settings. A "ui-tick" thread asks for a repaint every 250 ms, so
//! [`App::logic`](eframe::App::logic) keeps running while the window is
//! hidden (eframe then runs only `logic`, no egui pass): the service is
//! ticked (device following, garbage), beat events are drained and the
//! settings saved even in the background.

mod controller;
mod settings;
mod windows;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use eframe::egui::{self, Sense, ViewportCommand};
use pm_audio::{AudioService, DeviceInfo, EngineInit};
use pm_ui::state::{UiAction, UiState, WindowMode};
use pm_ui::theme;
use pm_ui::views::footer::MAX_VISUAL_OFFSET_MS;
use pm_ui::views::{full::full_view, mini::mini_view};

use controller::Controller;
use settings::Settings;
use windows::Windows;

/// How often the audio service is ticked (and the hidden app woken).
const TICK: Duration = Duration::from_millis(250);
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
    settings_dir: Option<PathBuf>,
    /// Last settings written; the latest ones and when they last changed.
    saved: Settings,
    pending: (Settings, Instant),
    last_tick: Instant,
    last_frame: Instant,
}

impl App {
    fn new(audio: AudioService, settings: Settings, settings_dir: Option<PathBuf>) -> App {
        let now = Instant::now();
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
            settings_dir,
            saved: settings.clone(),
            pending: (settings, now),
            last_tick: now,
            last_frame: now,
        };
        app.sync_device();
        app
    }

    /// Device name for the views and, when the output changed, its visual
    /// offset: the one saved for it, else what CoreAudio reports.
    fn sync_device(&mut self) {
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
        if now - self.last_tick >= TICK {
            self.last_tick = now;
            self.audio.tick();
            self.sync_device();
        }
        let handle = self.audio.handle();
        // A parked SetPattern holds back every later command (Task 7).
        while handle.garbage.pop().is_ok() {}
        while let Ok(event) = handle.events.pop() {
            self.ctl.ui.on_beat(&event);
        }
        self.ctl.ui.underruns = handle.underruns.load(Ordering::Relaxed);
        self.ctl.flush(&mut self.audio);

        if ctx.input(|i| i.viewport().close_requested()) {
            for command in Windows::hide() {
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
/// latency CoreAudio reports for it (at most the slider's maximum).
fn visual_offset_for(device: &DeviceInfo, saved: &BTreeMap<String, u32>) -> Duration {
    match saved.get(&device.uid) {
        Some(&ms) => Duration::from_millis(u64::from(ms)),
        None => device
            .reported_latency
            .min(Duration::from_millis(u64::from(MAX_VISUAL_OFFSET_MS))),
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
    use super::*;

    #[test]
    fn visual_offset_is_saved_per_device_else_reported_latency() {
        let device = |uid: &str, latency_ms: u64| DeviceInfo {
            uid: uid.to_owned(),
            name: uid.to_owned(),
            sample_rate: 48_000,
            reported_latency: Duration::from_millis(latency_ms),
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
