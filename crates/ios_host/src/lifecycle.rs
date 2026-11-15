//! When the app is on screen, as the scene lifecycle moves it. Pure, so the
//! rules are tested on any platform; the host applies the [`Change`]s.
//!
//! The app (the [`AppShell`](pm_shell::AppShell), its audio and the egui
//! context) outlives the scene: UIKit may disconnect a background scene
//! while the process keeps playing background audio, and connect a new one
//! later. Only the view, its surface and its display link go with the scene.

/// What the host must do after a lifecycle event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Change {
    /// Left the screen: start the background timer and call
    /// [`on_background`](pm_shell::AppShell::on_background).
    pub hidden: bool,
    /// Back on screen: stop the background timer and run the display link.
    pub shown: bool,
}

/// Whether the scene is in the foreground and whether a view is attached.
/// The app is on screen only with both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lifecycle {
    foreground: bool,
    attached: bool,
}

impl Default for Lifecycle {
    /// Before the first scene connects: in the foreground (the first scene
    /// shows as soon as it connects), no view yet.
    fn default() -> Self {
        Lifecycle {
            foreground: true,
            attached: false,
        }
    }
}

impl Lifecycle {
    /// Whether frames are drawn.
    pub fn on_screen(&self) -> bool {
        self.foreground && self.attached
    }

    /// A scene connected and its view is attached.
    pub fn attach(&mut self) -> Change {
        self.update(|s| s.attached = true)
    }

    /// The scene disconnected: its view is gone, and a scene that connects
    /// later is in the background until it says otherwise.
    pub fn detach(&mut self) -> Change {
        self.update(|s| {
            s.attached = false;
            s.foreground = false;
        })
    }

    /// `sceneDidEnterBackground`.
    pub fn enter_background(&mut self) -> Change {
        self.update(|s| s.foreground = false)
    }

    /// `sceneWillEnterForeground`.
    pub fn enter_foreground(&mut self) -> Change {
        self.update(|s| s.foreground = true)
    }

    fn update(&mut self, f: impl FnOnce(&mut Self)) -> Change {
        let before = self.on_screen();
        f(self);
        let after = self.on_screen();
        Change {
            hidden: before && !after,
            shown: !before && after,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Change = Change {
        hidden: false,
        shown: false,
    };
    const HIDDEN: Change = Change {
        hidden: true,
        shown: false,
    };
    const SHOWN: Change = Change {
        hidden: false,
        shown: true,
    };

    #[test]
    fn first_scene_shows_at_once() {
        let mut l = Lifecycle::default();
        assert!(!l.on_screen());
        assert_eq!(l.attach(), SHOWN);
        // UIKit's own sceneWillEnterForeground at launch changes nothing.
        assert_eq!(l.enter_foreground(), NONE);
        assert!(l.on_screen());
    }

    #[test]
    fn background_then_disconnect_hides_once() {
        let mut l = Lifecycle::default();
        l.attach();
        assert_eq!(l.enter_background(), HIDDEN);
        // The app keeps running (audio, timer): no second on_background.
        assert_eq!(l.detach(), NONE);
        assert!(!l.on_screen());
    }

    #[test]
    fn disconnect_without_background_hides() {
        let mut l = Lifecycle::default();
        l.attach();
        assert_eq!(l.detach(), HIDDEN);
    }

    #[test]
    fn reconnected_scene_waits_for_the_foreground() {
        let mut l = Lifecycle::default();
        l.attach();
        l.enter_background();
        l.detach();
        // A scene connected in the background draws nothing yet.
        assert_eq!(l.attach(), NONE);
        assert!(!l.on_screen());
        assert_eq!(l.enter_foreground(), SHOWN);
        assert!(l.on_screen());
    }

    #[test]
    fn foreground_without_a_view_stays_hidden() {
        let mut l = Lifecycle::default();
        l.attach();
        l.detach();
        assert_eq!(l.enter_foreground(), NONE);
        assert_eq!(l.attach(), SHOWN);
    }
}
