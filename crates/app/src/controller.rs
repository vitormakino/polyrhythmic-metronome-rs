//! Turns what the views ask for ([`UiAction`]) into engine [`Command`]s.
//!
//! Every play/stop goes through [`Controller::set_playing`] whatever its
//! source (the full or mini window's button, the menu bar item, the global
//! hotkey: all as [`UiAction::TogglePlay`]), so the lit beats are always cleared on a transport
//! change. Commands go through [`Audio::send`] (`AudioService::send`, R10) and
//! wait in order in an outbox when the engine's queue is full.
//!
//! A practice session never edits the user's setup: Play sends the selected
//! plan with it, and when the session ends (Stop, or the session stopping
//! itself) the base tempo is sent back, moved only by the user's own changes.

use std::collections::VecDeque;
use std::sync::Arc;

use pm_audio::{AudioService, Command, PatternUpdate};
use pm_core::session::SessionProgress;
use pm_ui::state::{SessionEntry, UiAction, UiState};

/// Sample rate used to build clips while there is no output device (the
/// service's placeholder engine runs at the same rate).
const IDLE_SAMPLE_RATE: u32 = 48_000;

/// Where commands go: the audio service, or a fake in tests.
pub trait Audio {
    /// Sends `command`, or gives it back if the engine's queue is full.
    fn send(&mut self, command: Command) -> Result<(), Command>;
    /// Rate to build a pattern's clips at.
    fn sample_rate(&self) -> u32;
}

impl Audio for AudioService {
    fn send(&mut self, command: Command) -> Result<(), Command> {
        AudioService::send(self, command)
    }

    fn sample_rate(&self) -> u32 {
        self.device_info()
            .map_or(IDLE_SAMPLE_RATE, |info| info.sample_rate)
    }
}

/// A command waiting to be sent.
enum Outgoing {
    Command(Command),
    /// `SetPattern` with the pattern as it is when sent; the clips are built
    /// then, at the output's rate at that moment.
    Pattern,
}

pub struct Controller {
    pub ui: UiState,
    outbox: VecDeque<Outgoing>,
    /// The last Play carried a session plan and no Stop followed yet.
    session_running: bool,
}

impl Controller {
    pub fn new(ui: UiState) -> Controller {
        Controller {
            ui,
            outbox: VecDeque::new(),
            session_running: false,
        }
    }

    /// Forwards a view's action. `ToggleMini` and `SetVisualOffset` concern
    /// the windows and the settings, not the engine, and are ignored here.
    pub fn apply(&mut self, audio: &mut impl Audio, action: UiAction) {
        match action {
            // The view already flipped `ui.playing`.
            UiAction::TogglePlay => self.set_playing(audio, self.ui.playing),
            UiAction::SetBpm(bpm) => self.push(audio, Outgoing::Command(Command::SetBpm(bpm))),
            UiAction::PatternChanged => self.push(audio, Outgoing::Pattern),
            UiAction::TakeOverPulse(index) => {
                self.push(audio, Outgoing::Command(Command::UserPulse(index)));
            }
            // The view applies the tapped tempo itself and sends `SetBpm`.
            // A selection needs no command: the plan goes with the next Play
            // (and the selector is disabled while a session runs).
            UiAction::Tap(_)
            | UiAction::ToggleMini
            | UiAction::SetVisualOffset(_)
            | UiAction::SelectSession => {}
        }
    }

    /// Starts or stops, from any source. Always clears the lit beats: after
    /// Stop nothing stays lit, and Play never shows a stale beat.
    ///
    /// Play sends the selected session's plan as it is now (`None` without
    /// one), so an edited session file counts from the next Play, and
    /// forgets the last summary. Stop after a session sends the base tempo
    /// back.
    pub fn set_playing(&mut self, audio: &mut impl Audio, playing: bool) {
        self.ui.playing = playing;
        self.ui.clear_beats();
        if playing {
            let plan = self
                .ui
                .selected_session()
                .map(|entry| Box::new((*entry.plan).clone()));
            self.session_running = plan.is_some();
            self.ui.session_summary = None;
            self.ui.session_progress = None;
            self.push(audio, Outgoing::Command(Command::Session(plan)));
            self.push(audio, Outgoing::Command(Command::Play));
        } else {
            self.push(audio, Outgoing::Command(Command::Stop));
            self.end_session(audio);
        }
    }

    /// The engine's session progress for this frame, and whether the engine
    /// plays. `progress` must be the current run's or `None` (see
    /// `AudioService::session_progress`); while a Play still waits in the
    /// outbox it is the run before it and is ignored. A finished session the
    /// engine stopped while the UI still plays ends here.
    pub fn sync_session(
        &mut self,
        audio: &mut impl Audio,
        progress: Option<SessionProgress>,
        engine_playing: bool,
    ) {
        let play_waiting = self
            .outbox
            .iter()
            .any(|o| matches!(o, Outgoing::Command(Command::Play)));
        self.ui.session_progress = progress.filter(|_| !play_waiting);
        let finished = self.ui.session_progress.is_some_and(|p| p.finished);
        if finished && self.ui.playing && !engine_playing {
            self.session_finished(audio);
        }
    }

    /// The session stopped itself (`OnFinish::Stop`): the UI stops too,
    /// keeps the finished progress as the summary and sends the base tempo
    /// back. `Stop` goes first so the engine and its recorded transport
    /// agree that nothing plays.
    pub fn session_finished(&mut self, audio: &mut impl Audio) {
        self.ui.playing = false;
        self.ui.clear_beats();
        self.ui.session_summary = self.ui.session_progress.filter(|p| p.finished);
        self.push(audio, Outgoing::Command(Command::Stop));
        self.push(audio, Outgoing::Command(Command::SetBpm(self.ui.bpm)));
        self.session_running = false;
        self.drop_missing_selection();
    }

    /// New session list from the config files. A selected session that is
    /// gone becomes "No session", but not while it runs.
    pub fn set_sessions(&mut self, sessions: Arc<[SessionEntry]>) {
        self.ui.sessions = sessions;
        self.drop_missing_selection();
    }

    /// After a session: the base tempo goes back to the engine.
    fn end_session(&mut self, audio: &mut impl Audio) {
        if std::mem::take(&mut self.session_running) {
            self.push(audio, Outgoing::Command(Command::SetBpm(self.ui.bpm)));
        }
        self.drop_missing_selection();
    }

    fn drop_missing_selection(&mut self) {
        if !self.session_running
            && self.ui.session.is_some()
            && self.ui.selected_session().is_none()
        {
            self.ui.session = None;
            self.ui.session_summary = None;
        }
    }

    /// Sends what is waiting, in order, until the engine's queue is full.
    pub fn flush(&mut self, audio: &mut impl Audio) {
        while let Some(next) = self.outbox.pop_front() {
            let command = match next {
                Outgoing::Command(command) => command,
                Outgoing::Pattern => Command::SetPattern(PatternUpdate::build(
                    audio.sample_rate(),
                    self.ui.pattern.clone(),
                )),
            };
            if let Err(command) = audio.send(command) {
                let back = match command {
                    Command::SetPattern(_) => Outgoing::Pattern,
                    other => Outgoing::Command(other),
                };
                self.outbox.push_front(back);
                return;
            }
        }
    }

    /// Commands still waiting for room in the engine's queue.
    #[cfg(test)]
    pub fn waiting(&self) -> usize {
        self.outbox.len()
    }

    fn push(&mut self, audio: &mut impl Audio, item: Outgoing) {
        // A queued `Pattern` sends the latest pattern anyway.
        let queued_pattern = self.outbox.iter().any(|o| matches!(o, Outgoing::Pattern));
        if !(matches!(item, Outgoing::Pattern) && queued_pattern) {
            self.outbox.push_back(item);
        }
        self.flush(audio);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use pm_audio::BeatEvent;
    use pm_core::session::{OnFinish, Runner, SessionPlan, SessionProgress, Span, Stage};
    use pm_core::{Bpm, MAX_PULSES, Pattern, SampleBank};
    use pm_ui::state::SessionEntry;

    use super::*;
    use crate::remote::{self, Remote};

    /// Records what was sent; refuses everything while `full`.
    #[derive(Default)]
    struct Fake {
        sent: Vec<String>,
        patterns: Vec<(u32, Pattern)>,
        full: bool,
        rate: u32,
    }

    impl Audio for Fake {
        fn send(&mut self, command: Command) -> Result<(), Command> {
            if self.full {
                return Err(command);
            }
            self.sent.push(match &command {
                Command::Play => "Play".to_owned(),
                Command::Stop => "Stop".to_owned(),
                Command::SetBpm(b) => format!("Bpm {}", b.get()),
                Command::UserPulse(i) => format!("UserPulse {i}"),
                Command::Session(plan) => format!(
                    "Session {}",
                    plan.as_ref().map_or("None", |p| p.name.as_str())
                ),
                Command::SetPattern(u) => {
                    // Which rate the clips were built at, by clip length.
                    let len = u.bank.clip(0, false).len();
                    let rate = [44_100, 96_000]
                        .into_iter()
                        .find(|&r| SampleBank::build(r, &u.pattern).clip(0, false).len() == len)
                        .unwrap_or(0);
                    self.patterns.push((rate, u.pattern.clone()));
                    format!("Pattern {}", u.pattern.pulses.len())
                }
            });
            Ok(())
        }

        fn sample_rate(&self) -> u32 {
            self.rate
        }
    }

    fn fake() -> Fake {
        Fake {
            rate: 44_100,
            ..Fake::default()
        }
    }

    /// A controller that is playing with pulse 0 lit.
    fn lit() -> (Controller, Fake) {
        let mut audio = fake();
        let mut c = Controller::new(UiState::default());
        c.set_playing(&mut audio, true);
        let now = Instant::now();
        let pulse = c.ui.pattern.pulses[0].id;
        c.ui.on_beat(&BeatEvent {
            pulse,
            beat: 2,
            frame: 100,
            at: now - Duration::from_millis(5),
            muted: false,
        });
        c.ui.advance(now, 0.016);
        assert_eq!(c.ui.active_beat(pulse), Some(2));
        audio.sent.clear();
        (c, audio)
    }

    #[test]
    fn stop_from_any_source_clears_lit_beats() {
        let (mut c, mut audio) = lit();
        let pulse = c.ui.pattern.pulses[0].id;
        // Not through a view's button: the menu bar item or the hotkey.
        for action in remote::view_actions(&mut c.ui, Remote::TogglePlay) {
            c.apply(&mut audio, action);
        }
        assert!(!c.ui.playing);
        assert_eq!(c.ui.active_beat(pulse), None);
        assert_eq!(c.ui.pending_beats(), 0);
        assert_eq!(audio.sent, ["Stop"]);
    }

    #[test]
    fn play_clears_stale_lit_state() {
        let (mut c, mut audio) = lit();
        let pulse = c.ui.pattern.pulses[0].id;
        // A beat still queued while (wrongly) left lit: Play starts clean.
        c.ui.on_beat(&BeatEvent {
            pulse,
            beat: 1,
            frame: 200,
            at: Instant::now() + Duration::from_secs(5),
            muted: false,
        });
        c.set_playing(&mut audio, true);
        assert_eq!(c.ui.active_beat(pulse), None);
        assert_eq!(c.ui.pending_beats(), 0);
        assert_eq!(audio.sent, ["Session None", "Play"]);
    }

    #[test]
    fn view_actions_become_commands() {
        let mut audio = fake();
        let mut c = Controller::new(UiState::default());
        c.ui.playing = true; // the view flipped it
        c.apply(&mut audio, UiAction::TogglePlay);
        c.apply(&mut audio, UiAction::SetBpm(Bpm::new(90)));
        c.apply(&mut audio, UiAction::Tap(Duration::from_millis(500)));
        c.apply(&mut audio, UiAction::ToggleMini);
        c.apply(&mut audio, UiAction::SetVisualOffset(Duration::ZERO));
        c.ui.pattern.add_pulse();
        c.apply(&mut audio, UiAction::PatternChanged);
        assert_eq!(audio.sent, ["Session None", "Play", "Bpm 90", "Pattern 3"]);
        assert_eq!(audio.patterns, [(44_100, c.ui.pattern.clone())]);
    }

    #[test]
    fn full_queue_keeps_order_and_sends_the_latest_pattern() {
        let mut audio = fake();
        let mut c = Controller::new(UiState::default());
        audio.full = true;
        c.apply(&mut audio, UiAction::PatternChanged);
        c.apply(&mut audio, UiAction::SetBpm(Bpm::new(100)));
        c.ui.pattern.add_pulse();
        c.apply(&mut audio, UiAction::PatternChanged);
        c.set_playing(&mut audio, true);
        assert!(audio.sent.is_empty());
        assert_eq!(c.waiting(), 4, "the second pattern edit joins the first");

        audio.full = false;
        audio.rate = 96_000; // the device changed meanwhile
        c.flush(&mut audio);
        assert_eq!(audio.sent, ["Pattern 3", "Bpm 100", "Session None", "Play"]);
        assert_eq!(audio.patterns, [(96_000, c.ui.pattern.clone())]);
        assert_eq!(c.waiting(), 0);
    }

    fn plan(name: &str) -> SessionPlan {
        SessionPlan {
            name: name.to_owned(),
            stages: vec![Stage {
                length: Span::Bars(2),
                tempo: None,
                mute: None,
                random_mute: None,
                pulses: [None; MAX_PULSES],
            }],
            on_finish: OnFinish::Stop,
            show_muted_visuals: false,
            seed: 1,
        }
    }

    fn entries(names: &[&str]) -> Arc<[SessionEntry]> {
        names
            .iter()
            .map(|&name| SessionEntry {
                name: name.to_owned(),
                plan: Arc::new(plan(name)),
            })
            .collect()
    }

    /// The first bar of `name`'s plan from a base of 100 BPM, edited by `edit`.
    fn progress(name: &str, edit: impl FnOnce(&mut SessionProgress)) -> SessionProgress {
        let plan = plan(name);
        let mut runner = Runner::start(&plan, Bpm::new(100), 48_000, 0);
        runner.bar(&plan, 0);
        let mut p = runner.progress();
        edit(&mut p);
        p
    }

    /// Stopped at 100 BPM with sessions "warm" and "cool", `selected` picked.
    fn with_sessions(selected: Option<&str>) -> (Controller, Fake) {
        let mut c = Controller::new(UiState::default());
        c.ui.bpm = Bpm::new(100);
        c.ui.session = selected.map(str::to_owned);
        c.set_sessions(entries(&["warm", "cool"]));
        (c, fake())
    }

    #[test]
    fn play_sends_selected_session_then_play() {
        let (mut c, mut audio) = with_sessions(Some("warm"));
        c.ui.session_summary = Some(progress("warm", |p| p.finished = true));
        c.ui.playing = true;
        c.apply(&mut audio, UiAction::TogglePlay);
        assert_eq!(audio.sent, ["Session warm", "Play"]);
        assert_eq!(c.ui.session_summary, None);

        let (mut c, mut audio) = with_sessions(None);
        c.set_playing(&mut audio, true);
        assert_eq!(audio.sent, ["Session None", "Play"]);
    }

    #[test]
    fn stop_restores_base_plus_user_delta() {
        let (mut c, mut audio) = with_sessions(Some("warm"));
        c.set_playing(&mut audio, true);
        let at_110 = progress("warm", |p| p.moment.bpm = Bpm::new(110));
        c.sync_session(&mut audio, Some(at_110), true);
        assert_eq!(c.ui.shown_bpm(), Bpm::new(110));
        let mut actions = Vec::new();
        c.ui.set_shown_bpm(Bpm::new(111), &mut actions);
        for action in actions {
            c.apply(&mut audio, action);
        }
        for action in remote::view_actions(&mut c.ui, Remote::TogglePlay) {
            c.apply(&mut audio, action);
        }
        assert!(
            audio
                .sent
                .ends_with(&["Bpm 111".to_owned(), "Stop".into(), "Bpm 101".into()])
        );
        assert_eq!(c.ui.bpm, Bpm::new(101));
        assert_eq!(c.ui.shown_bpm(), Bpm::new(101));
    }

    #[test]
    fn finish_stop_sets_not_playing_and_summary() {
        let (mut c, mut audio) = with_sessions(Some("warm"));
        c.set_playing(&mut audio, true);
        let pulse = c.ui.pattern.pulses[0].id;
        c.ui.on_beat(&BeatEvent {
            pulse,
            beat: 0,
            frame: 0,
            at: Instant::now() + Duration::from_secs(5),
            muted: false,
        });
        audio.sent.clear();
        let done = progress("warm", |p| p.finished = true);
        // Still playing (`OnFinish::Continue`): nothing ends.
        c.sync_session(&mut audio, Some(done), true);
        assert!(c.ui.playing);
        assert!(audio.sent.is_empty());

        c.sync_session(&mut audio, Some(done), false);
        assert!(!c.ui.playing);
        assert_eq!(c.ui.pending_beats(), 0);
        assert_eq!(c.ui.session_summary, Some(done));
        assert_eq!(audio.sent, ["Stop", "Bpm 100"]);
        // Seen again on the next frames: handled once.
        c.sync_session(&mut audio, Some(done), false);
        assert_eq!(audio.sent.len(), 2);
    }

    #[test]
    fn a_play_still_waiting_ignores_the_previous_run() {
        let (mut c, mut audio) = with_sessions(Some("warm"));
        audio.full = true;
        c.set_playing(&mut audio, true);
        let done = progress("warm", |p| p.finished = true);
        c.sync_session(&mut audio, Some(done), false);
        assert!(c.ui.playing, "the new Play is not dead");
        assert_eq!(c.ui.session_progress, None);
        assert_eq!(c.ui.session_summary, None);
        audio.full = false;
        c.flush(&mut audio);
        assert_eq!(audio.sent, ["Session warm", "Play"]);
    }

    #[test]
    fn taking_over_a_pulse_sends_user_pulse_before_the_pattern() {
        let (mut c, mut audio) = with_sessions(Some("warm"));
        c.set_playing(&mut audio, true);
        c.sync_session(
            &mut audio,
            Some(progress("warm", |p| p.moment.pulses.off = 0b10)),
            true,
        );
        audio.sent.clear();
        let mut actions = Vec::new();
        c.ui.take_over_pulse(1, &mut actions);
        // What the card does next: the pattern as clicked (already on).
        actions.push(UiAction::PatternChanged);
        for action in actions {
            c.apply(&mut audio, action);
        }
        assert_eq!(audio.sent, ["UserPulse 1", "Pattern 2"]);
        assert!(c.ui.pulse_shown_enabled(1));
    }

    #[test]
    fn a_vanished_selection_clears_only_when_not_running() {
        let (mut c, mut audio) = with_sessions(Some("warm"));
        c.set_sessions(entries(&["cool"]));
        assert_eq!(c.ui.session, None);

        let (mut c, mut audio2) = with_sessions(Some("warm"));
        c.set_playing(&mut audio2, true);
        c.set_sessions(entries(&["cool"]));
        assert_eq!(c.ui.session.as_deref(), Some("warm"), "running");
        c.set_playing(&mut audio, false);
        assert_eq!(c.ui.session, None, "dropped once stopped");
    }
}
