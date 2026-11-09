//! What the menu bar item and the global hotkeys ask for, and how it becomes
//! the same [`UiAction`]s the views emit, so it goes through the same
//! handling (one transport function, one outbox).

use std::time::Instant;

use pm_core::Bpm;
use pm_ui::state::{UiAction, UiState, WindowMode};

/// A request from outside the window: the menu bar item or a global hotkey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remote {
    TogglePlay,
    /// Tempo up or down by this many BPM.
    BpmStep(i32),
    Tap,
    /// Full ⇄ mini, showing the window if it was hidden.
    ToggleMini,
    /// Show the window in this mode.
    Show(WindowMode),
    /// Quit for real (the close button only hides the window).
    Quit,
}

/// For the requests a view could make too: edits `ui` the way the view does
/// and returns the view's actions. Window requests return nothing (see
/// [`target_mode`]).
pub fn view_actions(ui: &mut UiState, remote: Remote, now: Instant) -> Vec<UiAction> {
    let mut actions = Vec::new();
    match remote {
        Remote::TogglePlay => {
            ui.playing = !ui.playing;
            actions.push(UiAction::TogglePlay);
        }
        Remote::BpmStep(step) => {
            let bpm = Bpm::new(i32::from(ui.bpm.get()) + step);
            if bpm != ui.bpm {
                ui.bpm = bpm;
                actions.push(UiAction::SetBpm(bpm));
            }
        }
        Remote::Tap => ui.tap(now, &mut actions),
        Remote::ToggleMini | Remote::Show(_) | Remote::Quit => {}
    }
    actions
}

/// The mode to show the window in for a window request, else `None`.
pub fn target_mode(remote: Remote, current: WindowMode) -> Option<WindowMode> {
    match remote {
        Remote::Show(mode) => Some(mode),
        Remote::ToggleMini => Some(match current {
            WindowMode::Full => WindowMode::Mini,
            WindowMode::Mini => WindowMode::Full,
        }),
        Remote::TogglePlay | Remote::BpmStep(_) | Remote::Tap | Remote::Quit => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn toggle_play_flips_and_asks_like_the_play_button() {
        let mut ui = UiState::default();
        let now = Instant::now();
        assert_eq!(
            view_actions(&mut ui, Remote::TogglePlay, now),
            [UiAction::TogglePlay]
        );
        assert!(ui.playing);
        view_actions(&mut ui, Remote::TogglePlay, now);
        assert!(!ui.playing);
    }

    #[test]
    fn bpm_steps_stay_in_range() {
        let mut ui = UiState::default();
        let now = Instant::now();
        assert_eq!(
            view_actions(&mut ui, Remote::BpmStep(1), now),
            [UiAction::SetBpm(Bpm::new(121))]
        );
        assert_eq!(
            view_actions(&mut ui, Remote::BpmStep(-1), now),
            [UiAction::SetBpm(Bpm::new(120))]
        );
        ui.bpm = Bpm::new(300);
        assert!(view_actions(&mut ui, Remote::BpmStep(1), now).is_empty());
        assert_eq!(ui.bpm, Bpm::new(300));
        ui.bpm = Bpm::new(40);
        assert!(view_actions(&mut ui, Remote::BpmStep(-1), now).is_empty());
    }

    #[test]
    fn taps_set_the_tempo_like_the_tap_button() {
        let mut ui = UiState::default();
        ui.bpm = Bpm::new(90);
        let start = Instant::now();
        let mut last = Vec::new();
        for i in 0..4 {
            last = view_actions(&mut ui, Remote::Tap, start + Duration::from_millis(500 * i));
            assert!(matches!(last[0], UiAction::Tap(_)));
        }
        assert_eq!(last.last(), Some(&UiAction::SetBpm(Bpm::new(120))));
        assert_eq!(ui.bpm, Bpm::new(120));
    }

    #[test]
    fn window_requests_pick_a_mode_and_leave_the_state_alone() {
        use WindowMode::{Full, Mini};
        assert_eq!(target_mode(Remote::Show(Full), Mini), Some(Full));
        assert_eq!(target_mode(Remote::Show(Mini), Mini), Some(Mini));
        assert_eq!(target_mode(Remote::ToggleMini, Full), Some(Mini));
        assert_eq!(target_mode(Remote::ToggleMini, Mini), Some(Full));
        assert_eq!(target_mode(Remote::TogglePlay, Full), None);
        assert_eq!(target_mode(Remote::Quit, Full), None);

        let mut ui = UiState::default();
        let now = Instant::now();
        for r in [Remote::Show(Mini), Remote::ToggleMini, Remote::Quit] {
            assert!(view_actions(&mut ui, r, now).is_empty());
        }
        assert_eq!(ui.mode, Full);
    }
}
