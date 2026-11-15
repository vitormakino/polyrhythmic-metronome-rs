//! The iPhone app: [`pm_shell::Shell`] in the full view only, without what
//! is Mac only (mini window button, session strip, config files), with the
//! theme following the system's light or dark mode, the audio held only
//! while playing ([`OutputGate`]) and the audio session's interruptions
//! stopping playback.

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

use crate::audio_session::{Interruption, SessionEvent, on_interruption};
use crate::output_gate::{Effect, OutputGate};

/// The built-in theme for the system's appearance: `"light"` in light mode,
/// `"default"` (dark) in dark mode or when the system does not say.
pub fn theme_for(system: Option<egui::Theme>) -> &'static str {
    match system {
        Some(egui::Theme::Light) => "light",
        Some(egui::Theme::Dark) | None => "default",
    }
}

/// An output the app can pause while stopped: [`AudioService`], or a fake
/// in tests.
pub trait Output {
    /// See `AudioService::suspend_output`.
    fn suspend_output(&mut self);
    /// See `AudioService::resume_output`.
    fn resume_output(&mut self);
}

impl Output for AudioService {
    fn suspend_output(&mut self) {
        AudioService::suspend_output(self);
    }

    fn resume_output(&mut self) {
        AudioService::resume_output(self);
    }
}

/// What the audio session gives the app: its events, delivered on the main
/// thread, and activation and deactivation, both asynchronous (activation
/// `id` answers with [`SessionEvent::Activated`] carrying that `id`).
pub struct SessionLink {
    pub events: Receiver<SessionEvent>,
    pub activate: Box<dyn Fn(u64)>,
    pub deactivate: Box<dyn Fn()>,
}

pub struct IosApp<A: Audio + Device + Output = AudioService> {
    shell: Shell<A>,
    session: SessionLink,
    gate: OutputGate,
    /// The built-in theme in use; `None` until the first check.
    theme: Option<&'static str>,
    /// Frames are drawn (`false` from `on_background` to the next frame).
    visible: bool,
}

impl<A: Audio + Device + Output> IosApp<A> {
    /// Starts from the saved `settings` in the full view, saving into
    /// `settings_dir`, showing `messages`. Activates nothing: `audio` should
    /// start suspended, and the session is activated at the first Play.
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
            gate: OutputGate::new(),
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

    /// The transport was toggled (`ui.playing` already flipped): a Play
    /// waits for the session, a Stop releases it a little later.
    fn toggled(&mut self, now: Instant) {
        let effects = self.gate.toggle(self.shell.ctl.ui.playing, now);
        self.run(&effects);
    }

    /// Carries out the gate's effects, in order.
    fn run(&mut self, effects: &[Effect]) {
        for effect in effects {
            match effect {
                Effect::Activate(id) => (self.session.activate)(*id),
                Effect::Deactivate => (self.session.deactivate)(),
                Effect::Resume => self.shell.audio_mut().resume_output(),
                Effect::Pause => self.shell.audio_mut().suspend_output(),
                Effect::Toggle => {
                    let _ = self.shell.apply(UiAction::TogglePlay);
                }
                Effect::Cancel => self.shell.ctl.ui.playing = false,
            }
        }
    }

    fn on_event(&mut self, event: SessionEvent, now: Instant) {
        match event {
            SessionEvent::Interruption(i) => self.interrupted(i, now),
            SessionEvent::Activated { id, result } => {
                let effects = self.gate.activated(id, result.is_ok());
                if let Err(reason) = &result {
                    let cancelled = if effects.contains(&Effect::Cancel) {
                        ", not playing"
                    } else {
                        ""
                    };
                    eprintln!(
                        "polyrhythmic-metronome: audio session not activated{cancelled}: {reason}"
                    );
                }
                self.run(&effects);
            }
            SessionEvent::MediaReset => {
                let effects = self.gate.media_reset();
                self.run(&effects);
            }
        }
    }

    /// Stops on an interruption that begins while playing, as the
    /// transport button would, then lets the gate release.
    fn interrupted(&mut self, i: Interruption, now: Instant) {
        let ui = &mut self.shell.ctl.ui;
        if on_interruption(i, ui.playing).is_some() {
            ui.playing = !ui.playing;
            self.toggled(now);
        }
        let effects = self.gate.interrupted(i);
        self.run(&effects);
    }

    /// [`AppShell::logic`] at `now`.
    fn logic_at(&mut self, now: Instant) {
        let events: Vec<SessionEvent> = self.session.events.try_iter().collect();
        for event in events {
            self.on_event(event, now);
        }
        self.shell.logic(now, self.visible);
        let effects = self.gate.tick(now);
        self.run(&effects);
    }
}

impl<A: Audio + Device + Output> AppShell for IosApp<A> {
    /// Before every frame and, in the background, on the host's wake-ups.
    fn logic(&mut self, ctx: &egui::Context) {
        self.follow_system_theme(ctx);
        self.logic_at(Instant::now());
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.visible = true;
        // This frame's appearance, before anything is painted with the old one.
        self.follow_system_theme(ui.ctx());
        let now = Instant::now();
        let mut toggled = false;
        // Nothing comes back: the only host action is the hidden mini button's.
        let _ = self.shell.view(ui, now, |ui, state, now| {
            let mut actions = full_view(ui, state, now);
            // The transport goes through the gate, after the view's other
            // actions.
            toggled = actions.contains(&UiAction::TogglePlay);
            actions.retain(|a| *a != UiAction::TogglePlay);
            actions
        });
        if toggled {
            self.toggled(now);
        }
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
    use std::time::Duration;

    use egui::Theme;
    use pm_audio::{BeatEvent, Command, DeviceInfo, OutputLatency};
    use pm_core::session::SessionProgress;

    use super::*;
    use crate::output_gate::RELEASE_AFTER;

    #[test]
    fn light_system_picks_light_theme() {
        assert_eq!(theme_for(Some(Theme::Light)), "light");
        assert_eq!(theme_for(Some(Theme::Dark)), "default");
        assert_eq!(theme_for(None), "default");
    }

    /// What the output and the session were asked, in order.
    type Log = Rc<RefCell<Vec<String>>>;

    /// Records the transport commands, pauses and resumes (shared, the
    /// shell owns the fake); one fixed output.
    struct Fake {
        log: Log,
    }

    impl Audio for Fake {
        fn send(&mut self, command: Command) -> Result<(), Command> {
            match command {
                Command::Play => self.log.borrow_mut().push("Play".to_owned()),
                Command::Stop => self.log.borrow_mut().push("Stop".to_owned()),
                _ => {}
            }
            Ok(())
        }

        fn sample_rate(&self) -> u32 {
            48_000
        }
    }

    impl Output for Fake {
        fn suspend_output(&mut self) {
            self.log.borrow_mut().push("pause".to_owned());
        }

        fn resume_output(&mut self) {
            self.log.borrow_mut().push("resume".to_owned());
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

    /// An app on the fake output and session, the sender of its session
    /// events and the log both write to.
    fn app() -> (IosApp<Fake>, Sender<SessionEvent>, Log) {
        let log: Log = Rc::default();
        let (tx, rx) = mpsc::channel();
        let (activate, deactivate) = (Rc::clone(&log), Rc::clone(&log));
        let session = SessionLink {
            events: rx,
            activate: Box::new(move |id| activate.borrow_mut().push(format!("activate {id}"))),
            deactivate: Box::new(move || deactivate.borrow_mut().push("deactivate".to_owned())),
        };
        let audio = Fake {
            log: Rc::clone(&log),
        };
        let app = IosApp::new(
            audio,
            &Settings::default(),
            None,
            Messages::english(),
            session,
        );
        (app, tx, log)
    }

    /// The transport button, as the view presses it.
    fn press(app: &mut IosApp<Fake>, now: Instant) {
        let ui = &mut app.shell.ctl.ui;
        ui.playing = !ui.playing;
        app.toggled(now);
    }

    /// Takes what was logged so far.
    fn taken(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.borrow_mut())
    }

    /// Activation `id` finished, `ok` or not.
    fn activated(tx: &Sender<SessionEvent>, id: u64, ok: bool) {
        let result = if ok {
            Ok(())
        } else {
            Err("refused".to_owned())
        };
        tx.send(SessionEvent::Activated { id, result }).unwrap();
    }

    /// An app playing since `t0`, the log emptied.
    fn playing(t0: Instant) -> (IosApp<Fake>, Sender<SessionEvent>, Log) {
        let (mut app, tx, log) = app();
        press(&mut app, t0);
        activated(&tx, 1, true);
        app.logic_at(t0);
        assert!(app.shell.ctl.ui.playing);
        taken(&log);
        (app, tx, log)
    }

    #[test]
    fn ios_app_hides_mac_only_parts() {
        let (app, _tx, _log) = app();
        assert!(!app.shell.ctl.ui.show_mini_button);
        assert!(!app.shell.ctl.ui.show_sessions);
        assert_eq!(app.shell.ctl.ui.mode, WindowMode::Full);
    }

    #[test]
    fn launch_activates_nothing() {
        let (mut app, _tx, log) = app();
        let t0 = Instant::now();
        for n in 0..20 {
            app.logic_at(t0 + Duration::from_millis(250) * n);
        }
        assert_eq!(taken(&log), Vec::<&str>::new());
    }

    #[test]
    fn play_activates_then_resumes_then_plays() {
        let (mut app, tx, log) = app();
        let t0 = Instant::now();
        press(&mut app, t0);
        // Held until the session is active; the button already shows it.
        assert_eq!(taken(&log), ["activate 1"]);
        assert!(app.shell.ctl.ui.playing);
        app.logic_at(t0 + Duration::from_millis(16));
        assert_eq!(taken(&log), Vec::<&str>::new());
        activated(&tx, 1, true);
        app.logic_at(t0 + Duration::from_millis(32));
        assert_eq!(taken(&log), ["resume", "Play"]);
        assert!(app.shell.ctl.ui.playing);
    }

    #[test]
    fn stop_releases_after_the_tail() {
        let t0 = Instant::now();
        let (mut app, _tx, log) = playing(t0);
        let t1 = t0 + Duration::from_secs(10);
        press(&mut app, t1);
        assert_eq!(taken(&log), ["Stop"]);
        app.logic_at(t1 + RELEASE_AFTER / 2);
        assert_eq!(taken(&log), Vec::<&str>::new(), "the last click rings out");
        app.logic_at(t1 + RELEASE_AFTER);
        assert_eq!(taken(&log), ["pause", "deactivate"]);
        app.logic_at(t1 + RELEASE_AFTER * 4);
        assert_eq!(taken(&log), Vec::<&str>::new());
        // The next Play goes through the session again.
        press(&mut app, t1 + Duration::from_secs(60));
        assert_eq!(taken(&log), ["activate 2"]);
    }

    #[test]
    fn play_again_within_the_tail_keeps_the_session() {
        let t0 = Instant::now();
        let (mut app, _tx, log) = playing(t0);
        press(&mut app, t0 + Duration::from_secs(1));
        press(&mut app, t0 + Duration::from_millis(1_200));
        app.logic_at(t0 + Duration::from_secs(5));
        assert_eq!(taken(&log), ["Stop", "Play"]);
    }

    #[test]
    fn stop_while_activating_never_plays_and_deactivates() {
        let (mut app, tx, log) = app();
        let t0 = Instant::now();
        press(&mut app, t0);
        press(&mut app, t0 + Duration::from_millis(10));
        assert!(!app.shell.ctl.ui.playing);
        activated(&tx, 1, true);
        app.logic_at(t0 + Duration::from_millis(40));
        assert_eq!(taken(&log), ["activate 1", "deactivate"]);
        // A Play then waits for the session again.
        press(&mut app, t0 + Duration::from_secs(1));
        assert_eq!(taken(&log), ["activate 2"]);
    }

    #[test]
    fn failed_activation_shows_stopped() {
        let (mut app, tx, log) = app();
        let t0 = Instant::now();
        press(&mut app, t0);
        activated(&tx, 1, false);
        app.logic_at(t0 + Duration::from_millis(40));
        assert!(!app.shell.ctl.ui.playing);
        assert_eq!(taken(&log), ["activate 1"]);
    }

    #[test]
    fn interruption_stops_releases_and_never_resumes() {
        let t0 = Instant::now();
        let (mut app, tx, log) = playing(t0);
        tx.send(SessionEvent::Interruption(Interruption::Began))
            .unwrap();
        app.logic_at(t0 + Duration::from_secs(1));
        assert!(!app.shell.ctl.ui.playing);
        // The system already silenced the output: no tail to wait for.
        assert_eq!(taken(&log), ["Stop", "pause", "deactivate"]);
        tx.send(SessionEvent::Interruption(Interruption::Ended {
            should_resume: true,
        }))
        .unwrap();
        app.logic_at(t0 + Duration::from_secs(30));
        assert!(!app.shell.ctl.ui.playing);
        // cpal activates the session again on "should resume": undone.
        assert_eq!(taken(&log), ["deactivate"]);
    }

    #[test]
    fn media_reset_activates_again_only_while_held() {
        let t0 = Instant::now();
        let (mut app, tx, log) = playing(t0);
        tx.send(SessionEvent::MediaReset).unwrap();
        app.logic_at(t0 + Duration::from_millis(100));
        assert_eq!(taken(&log), ["activate 2"]);
        activated(&tx, 2, true);
        app.logic_at(t0 + Duration::from_millis(200));
        assert_eq!(taken(&log), Vec::<&str>::new(), "already playing");
        let (mut app, tx, log) = self::app();
        tx.send(SessionEvent::MediaReset).unwrap();
        app.logic_at(t0);
        assert_eq!(taken(&log), Vec::<&str>::new());
    }

    /// Playing, a media reset asks for activation 2, then Stop releases
    /// before it answers.
    fn released_with_a_reactivation_pending(
        t0: Instant,
    ) -> (IosApp<Fake>, Sender<SessionEvent>, Log) {
        let (mut app, tx, log) = playing(t0);
        tx.send(SessionEvent::MediaReset).unwrap();
        app.logic_at(t0 + Duration::from_millis(100));
        press(&mut app, t0 + Duration::from_millis(200));
        app.logic_at(t0 + Duration::from_secs(1));
        assert_eq!(taken(&log), ["activate 2", "Stop", "pause", "deactivate"]);
        (app, tx, log)
    }

    #[test]
    fn a_stale_activation_does_not_start_the_next_play() {
        let t0 = Instant::now();
        let (mut app, tx, log) = released_with_a_reactivation_pending(t0);
        press(&mut app, t0 + Duration::from_secs(2));
        assert_eq!(taken(&log), ["activate 3"]);
        // The reset's answer comes in first: not this Play's.
        activated(&tx, 2, true);
        app.logic_at(t0 + Duration::from_millis(2_010));
        assert_eq!(taken(&log), Vec::<&str>::new());
        assert!(app.shell.ctl.ui.playing, "still waiting");
        activated(&tx, 3, true);
        app.logic_at(t0 + Duration::from_millis(2_020));
        assert_eq!(taken(&log), ["resume", "Play"]);
    }

    #[test]
    fn a_late_activation_while_released_is_given_back() {
        let t0 = Instant::now();
        let (mut app, tx, log) = released_with_a_reactivation_pending(t0);
        activated(&tx, 2, true);
        app.logic_at(t0 + Duration::from_secs(2));
        assert_eq!(taken(&log), ["deactivate"]);
        // A late failure leaves nothing to undo.
        activated(&tx, 2, false);
        app.logic_at(t0 + Duration::from_secs(3));
        assert_eq!(taken(&log), Vec::<&str>::new());
    }
}
