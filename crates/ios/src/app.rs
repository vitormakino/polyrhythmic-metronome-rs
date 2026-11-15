//! The iPhone app: [`pm_shell::Shell`] in the full view only, without what
//! is Mac only (mini window button, session strip, config files), with the
//! theme following the system's light or dark mode and the audio session's
//! interruptions stopping playback.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use pm_audio::AudioService;
use pm_shell::{AppShell, Audio, Device, Settings, Shell};
use pm_ui::i18n::Messages;
use pm_ui::state::{UiAction, UiState, WindowMode};
use pm_ui::theme::{self, Theme};
use pm_ui::views::full::full_view;

use crate::audio_session::{Interruption, on_interruption};

/// The built-in theme for the system's appearance: `"light"` in light mode,
/// `"default"` (dark) in dark mode or when the system does not say.
pub fn theme_for(system: Option<egui::Theme>) -> &'static str {
    match system {
        Some(egui::Theme::Light) => "light",
        Some(egui::Theme::Dark) | None => "default",
    }
}

/// Whether a view's `actions` start playback: a `TogglePlay` that left the
/// view `playing` (the view flips the state before asking).
pub fn starts_playing(actions: &[UiAction], playing: bool) -> bool {
    playing && actions.contains(&UiAction::TogglePlay)
}

/// What the audio session gives the app: its interruptions, delivered on
/// the main thread, and a way to ask for activation again before playing.
pub struct SessionLink {
    pub interruptions: Receiver<Interruption>,
    pub activate: Box<dyn Fn()>,
}

pub struct IosApp<A: Audio + Device = AudioService> {
    shell: Shell<A>,
    session: SessionLink,
    /// The built-in theme in use; `None` until the first check.
    theme: Option<&'static str>,
    /// Frames are drawn (`false` from `on_background` to the next frame).
    visible: bool,
}

impl<A: Audio + Device> IosApp<A> {
    /// Starts from the saved `settings` in the full view, saving into
    /// `settings_dir`, showing `messages`.
    pub fn new(
        audio: A,
        settings: &Settings,
        settings_dir: Option<PathBuf>,
        messages: Messages,
        session: SessionLink,
    ) -> Self {
        let mut ui = UiState::default();
        ui.messages = Arc::new(messages);
        ui.show_mini_button = false;
        ui.show_sessions = false;
        // No mini window here, whatever the file says.
        let settings = Settings {
            mode: WindowMode::Full,
            ..settings.clone()
        };
        IosApp {
            shell: Shell::new(audio, ui, &settings, settings_dir),
            session,
            theme: None,
            visible: true,
        }
    }

    /// Switches to the theme for the system's appearance, when it changed.
    fn follow_system_theme(&mut self, ctx: &egui::Context) {
        let name = theme_for(ctx.system_theme());
        if self.theme == Some(name) {
            return;
        }
        self.theme = Some(name);
        if let Some(t) = Theme::builtin(name) {
            theme::apply(ctx, &t);
            self.shell.ctl.ui.theme = Arc::new(t);
        }
    }

    /// Stops on an interruption that begins while playing, as the
    /// transport button would.
    fn interrupted(&mut self, i: Interruption) {
        let ui = &mut self.shell.ctl.ui;
        if let Some(action) = on_interruption(i, ui.playing) {
            ui.playing = !ui.playing;
            self.shell.apply(action);
        }
    }
}

impl<A: Audio + Device> AppShell for IosApp<A> {
    /// Before every frame and, in the background, on the host's wake-ups.
    fn logic(&mut self, ctx: &egui::Context) {
        let interruptions: Vec<Interruption> = self.session.interruptions.try_iter().collect();
        for i in interruptions {
            self.interrupted(i);
        }
        self.follow_system_theme(ctx);
        self.shell.logic(Instant::now(), self.visible);
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.visible = true;
        // This frame's appearance, before anything is painted with the old one.
        self.follow_system_theme(ui.ctx());
        let activate = &self.session.activate;
        // Nothing comes back: the only host action is the hidden mini button's.
        let _ = self.shell.view(ui, Instant::now(), |ui, state, now| {
            let actions = full_view(ui, state, now);
            // Ask for the session before the shell sends Play.
            if starts_playing(&actions, state.playing) {
                activate();
            }
            actions
        });
    }

    /// A suspended app never runs the delayed save: write now.
    fn on_background(&mut self) {
        self.visible = false;
        self.shell.save();
    }

    fn on_exit(&mut self) {
        self.shell.save();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc::{self, Sender};

    use egui::Theme;
    use pm_audio::{BeatEvent, Command, DeviceInfo, OutputLatency};
    use pm_core::session::SessionProgress;

    use super::*;

    #[test]
    fn light_system_picks_light_theme() {
        assert_eq!(theme_for(Some(Theme::Light)), "light");
        assert_eq!(theme_for(Some(Theme::Dark)), "default");
        assert_eq!(theme_for(None), "default");
    }

    #[test]
    fn only_a_toggle_that_leaves_playing_starts_playback() {
        assert!(starts_playing(&[UiAction::TogglePlay], true));
        assert!(!starts_playing(&[UiAction::TogglePlay], false), "a stop");
        assert!(!starts_playing(&[UiAction::PatternChanged], true));
        assert!(!starts_playing(&[], true));
    }

    /// Records the commands sent (shared, the shell owns the fake); one
    /// fixed output.
    #[derive(Default)]
    struct Fake {
        sent: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Audio for Fake {
        fn send(&mut self, command: Command) -> Result<(), Command> {
            self.sent.borrow_mut().push(match command {
                Command::Play => "Play",
                Command::Stop => "Stop",
                _ => "other",
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
            Some(DeviceInfo {
                uid: "Speaker".to_owned(),
                name: "Speaker".to_owned(),
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

        fn drain(&mut self, _f: &mut dyn FnMut(&BeatEvent)) {}

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

    /// An app on the fake output, and the sender of its interruptions.
    fn app(audio: Fake) -> (IosApp<Fake>, Sender<Interruption>) {
        let (tx, rx) = mpsc::channel();
        let session = SessionLink {
            interruptions: rx,
            activate: Box::new(|| {}),
        };
        let app = IosApp::new(
            audio,
            &Settings::default(),
            None,
            Messages::english(),
            session,
        );
        (app, tx)
    }

    #[test]
    fn ios_app_hides_mac_only_parts() {
        let (app, _tx) = app(Fake::default());
        assert!(!app.shell.ctl.ui.show_mini_button);
        assert!(!app.shell.ctl.ui.show_sessions);
        assert_eq!(app.shell.ctl.ui.mode, WindowMode::Full);
    }

    #[test]
    fn interruption_stops_a_playing_app() {
        let audio = Fake::default();
        let sent = Rc::clone(&audio.sent);
        let (mut app, tx) = app(audio);
        let ctx = egui::Context::default();
        app.shell.ctl.ui.playing = true;
        app.shell.apply(UiAction::TogglePlay);
        tx.send(Interruption::Began).unwrap();
        tx.send(Interruption::Ended {
            should_resume: true,
        })
        .unwrap();
        app.logic(&ctx);
        assert!(!app.shell.ctl.ui.playing);
        assert_eq!(sent.borrow().last(), Some(&"Stop"));
    }
}
