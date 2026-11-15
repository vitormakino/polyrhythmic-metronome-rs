//! When the iPhone app holds the audio: the session is active and the
//! stream runs only while playing (and for a short tail after Stop), so a
//! stopped app lets other apps' audio be and can be suspended in the
//! background. Pure: [`OutputGate`] turns what happens into [`Effect`]s,
//! which [`IosApp`](crate::IosApp) carries out in order.
//!
//! - Launch: nothing is active (the stream opens paused).
//! - Play: activate the session and hold the `Play`; once activated, start
//!   the stream, then send `Play` (the engine takes it at the stream's first
//!   callback, so the bar starts on the "one" with nothing lost).
//! - Stop: send `Stop`; [`RELEASE_AFTER`] later pause the stream, then
//!   deactivate the session (notifying the others).
//! - Interruption (call, alarm): the system already stopped the audio, so
//!   release at once after the app's `Stop`; never resume.

use std::time::{Duration, Instant};

use crate::audio_session::Interruption;

/// How long after Stop the stream keeps running: the engine takes `Stop`
/// and the last clicks ring out before the output pauses, and a quick
/// Stop–Play keeps the session.
pub const RELEASE_AFTER: Duration = Duration::from_millis(500);

/// What the app does, in the order given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Ask the session for activation number `.0`, off the main thread; a
    /// `SessionEvent::Activated` with that id follows.
    Activate(u64),
    /// Start the paused stream.
    Resume,
    /// Apply the transport toggle now (`TogglePlay` with `ui.playing` as it is).
    Toggle,
    /// The held Play is dropped (the session would not activate): show
    /// stopped.
    Cancel,
    /// Pause the stream.
    Pause,
    /// Deactivate the session, off the main thread.
    Deactivate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Session inactive, stream paused.
    Released,
    /// Activation `id` asked for; `play`: a Play waits for it.
    Activating { play: bool, id: u64 },
    /// Session active, stream running, playing.
    Active,
    /// Stopped at `since`; the release is due [`RELEASE_AFTER`] later.
    Releasing { since: Instant },
}

#[derive(Debug)]
pub struct OutputGate {
    state: State,
    /// Id of the last activation asked for.
    last_id: u64,
}

impl Default for OutputGate {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputGate {
    /// At launch: nothing active.
    pub fn new() -> OutputGate {
        OutputGate {
            state: State::Released,
            last_id: 0,
        }
    }

    /// The transport was toggled to `playing` (the view already flipped it).
    pub fn toggle(&mut self, playing: bool, now: Instant) -> Vec<Effect> {
        use State::*;
        let (state, effects) = match (self.state, playing) {
            (Released, true) => {
                let id = self.next_id();
                (Activating { play: true, id }, vec![Effect::Activate(id)])
            }
            (Activating { id, .. }, true) => (Activating { play: true, id }, vec![]),
            // Still held: the stream runs and the session is active.
            (Releasing { .. } | Active, true) => (Active, vec![Effect::Toggle]),
            // `Play` was never sent: nothing to stop.
            (Activating { id, .. }, false) => (Activating { play: false, id }, vec![]),
            (Active, false) => (Releasing { since: now }, vec![Effect::Toggle]),
            (state @ (Releasing { .. } | Released), false) => (state, vec![Effect::Toggle]),
        };
        self.state = state;
        effects
    }

    /// Activation `id` finished, `ok` or not. Only the latest one asked for
    /// counts: an older answer (a media reset's, or one overtaken by a
    /// Stop and a new Play) is not the current request's.
    pub fn activated(&mut self, id: u64, ok: bool) -> Vec<Effect> {
        let play = match self.state {
            State::Activating { play, id: current } if id == current => play,
            // A late success left the session active while stopped.
            State::Released if ok => return vec![Effect::Deactivate],
            // Held already (a media reset's re-activation), or the latest
            // activation will answer.
            _ => return vec![],
        };
        let (state, effects) = match (play, ok) {
            (true, true) => (State::Active, vec![Effect::Resume, Effect::Toggle]),
            (true, false) => (State::Released, vec![Effect::Cancel]),
            (false, true) => (State::Released, vec![Effect::Deactivate]),
            (false, false) => (State::Released, vec![]),
        };
        self.state = state;
        effects
    }

    /// An interruption, after the app stopped for it.
    pub fn interrupted(&mut self, i: Interruption) -> Vec<Effect> {
        match (i, self.state) {
            // The system stopped the output and deactivated the session:
            // pause our side now (so cpal does not restart the unit when
            // the interruption ends) and make it ours.
            (Interruption::Began, State::Active | State::Releasing { .. }) => {
                self.state = State::Released;
                vec![Effect::Pause, Effect::Deactivate]
            }
            // cpal's own observer activates the session on "should
            // resume"; a stopped app gives it back.
            (
                Interruption::Ended {
                    should_resume: true,
                },
                State::Released,
            ) => vec![Effect::Deactivate],
            _ => vec![],
        }
    }

    /// The system's media services restarted: the session is inactive again.
    pub fn media_reset(&mut self) -> Vec<Effect> {
        match self.state {
            State::Released => vec![],
            State::Activating { play, .. } => {
                let id = self.next_id();
                self.state = State::Activating { play, id };
                vec![Effect::Activate(id)]
            }
            State::Active | State::Releasing { .. } => vec![Effect::Activate(self.next_id())],
        }
    }

    fn next_id(&mut self) -> u64 {
        self.last_id += 1;
        self.last_id
    }

    /// Periodic: releases once the tail after Stop has passed.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        match self.state {
            State::Releasing { since } if now.saturating_duration_since(since) >= RELEASE_AFTER => {
                self.state = State::Released;
                vec![Effect::Pause, Effect::Deactivate]
            }
            _ => vec![],
        }
    }
}
