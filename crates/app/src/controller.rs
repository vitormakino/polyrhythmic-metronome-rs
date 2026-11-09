//! Turns what the views ask for ([`UiAction`]) into engine [`Command`]s.
//!
//! Every play/stop goes through [`Controller::set_playing`] whatever its
//! source (the full or mini window's button, the menu bar item, the global
//! hotkey: all as [`UiAction::TogglePlay`]), so the lit beats are always cleared on a transport
//! change. Commands go through [`Audio::send`] (`AudioService::send`, R10) and
//! wait in order in an outbox when the engine's queue is full.

use std::collections::VecDeque;

use pm_audio::{AudioService, Command, PatternUpdate};
use pm_ui::state::{UiAction, UiState};

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
}

impl Controller {
    pub fn new(ui: UiState) -> Controller {
        Controller {
            ui,
            outbox: VecDeque::new(),
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
            // The view applies the tapped tempo itself and sends `SetBpm`.
            UiAction::Tap(_) | UiAction::ToggleMini | UiAction::SetVisualOffset(_) => {}
        }
    }

    /// Starts or stops, from any source. Always clears the lit beats: after
    /// Stop nothing stays lit, and Play never shows a stale beat.
    pub fn set_playing(&mut self, audio: &mut impl Audio, playing: bool) {
        self.ui.playing = playing;
        self.ui.clear_beats();
        let command = if playing {
            Command::Play
        } else {
            Command::Stop
        };
        self.push(audio, Outgoing::Command(command));
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
    use std::time::{Duration, Instant};

    use pm_audio::BeatEvent;
    use pm_core::{Bpm, Pattern, SampleBank};

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
        for action in remote::view_actions(&mut c.ui, Remote::TogglePlay, Instant::now()) {
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
        });
        c.set_playing(&mut audio, true);
        assert_eq!(c.ui.active_beat(pulse), None);
        assert_eq!(c.ui.pending_beats(), 0);
        assert_eq!(audio.sent, ["Play"]);
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
        assert_eq!(audio.sent, ["Play", "Bpm 90", "Pattern 3"]);
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
        assert_eq!(c.waiting(), 3, "the second pattern edit joins the first");

        audio.full = false;
        audio.rate = 96_000; // the device changed meanwhile
        c.flush(&mut audio);
        assert_eq!(audio.sent, ["Pattern 3", "Bpm 100", "Play"]);
        assert_eq!(audio.patterns, [(96_000, c.ui.pattern.clone())]);
        assert_eq!(c.waiting(), 0);
    }
}
