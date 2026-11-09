//! The menu bar item: play/stop, BPM ±1, open the window, mini, quit. It is
//! also the only way to bring back a hidden window (eframe/winit give no
//! Dock "reopen" hook).
//!
//! macOS: build it on the main thread once the event loop runs (the first
//! `App::logic` call). Menu clicks arrive through [`forward`] as [`Remote`]s.

use std::error::Error;
use std::sync::mpsc::Sender;

use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

use pm_ui::state::WindowMode;

use crate::remote::Remote;

const PLAY: &str = "play";
const BPM_UP: &str = "bpm-up";
const BPM_DOWN: &str = "bpm-down";
const OPEN: &str = "open";
const MINI: &str = "mini";
const QUIT: &str = "quit";

/// Icon side in pixels (the menu bar shows it at 16 to 22 pt).
const ICON_SIZE: u32 = 32;

/// What a click on the menu item `id` asks for.
pub fn map_menu(id: &MenuId) -> Option<Remote> {
    Some(match id.as_ref() {
        PLAY => Remote::TogglePlay,
        BPM_UP => Remote::BpmStep(1),
        BPM_DOWN => Remote::BpmStep(-1),
        OPEN => Remote::Show(WindowMode::Full),
        MINI => Remote::Show(WindowMode::Mini),
        QUIT => Remote::Quit,
        _ => return None,
    })
}

/// The transport item's text for the current state.
pub fn play_label(playing: bool) -> &'static str {
    if playing { "Stop" } else { "Play" }
}

/// The menu bar item and the parts of it that change.
pub struct Tray {
    _icon: TrayIcon,
    play: MenuItem,
    playing: bool,
}

impl Tray {
    pub fn build(playing: bool) -> Result<Tray, Box<dyn Error>> {
        let play = MenuItem::with_id(PLAY, play_label(playing), true, None);
        let item = |id: &str, text: &str| MenuItem::with_id(id, text, true, None);
        let menu = Menu::with_items(&[
            &play,
            &PredefinedMenuItem::separator(),
            &item(BPM_UP, "BPM +1"),
            &item(BPM_DOWN, "BPM \u{2212}1"),
            &PredefinedMenuItem::separator(),
            &item(OPEN, "Abrir janela"),
            &item(MINI, "Mini"),
            &PredefinedMenuItem::separator(),
            &item(QUIT, "Sair"),
        ])?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon_templated(Icon::from_rgba(icon_rgba(), ICON_SIZE, ICON_SIZE)?)
            .with_tooltip("Polyrhythmic Metronome")
            .build()?;
        Ok(Tray {
            _icon: icon,
            play,
            playing,
        })
    }

    /// Keeps the transport item's text in step with the state.
    pub fn set_playing(&mut self, playing: bool) {
        if playing != self.playing {
            self.playing = playing;
            self.play.set_text(play_label(playing));
        }
    }
}

/// Sends menu clicks to `tx` and wakes the app (also while its window is
/// hidden). Clicks and hovers on the icon itself are dropped: the menu
/// opens on its own, and undrained icon events would pile up.
pub fn forward(tx: Sender<Remote>, ctx: egui::Context) {
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some(remote) = map_menu(&event.id) {
            let _ = tx.send(remote);
            ctx.request_repaint();
        }
    }));
    TrayIconEvent::set_event_handler(Some(|_: TrayIconEvent| {}));
}

/// A monochrome ring with a pendulum, as RGBA. macOS uses only its alpha
/// (template image) and tints it for light and dark menu bars. Static: a
/// blink on the "one" cost too much CPU (docs/VERIFICATION.md, Task 14).
pub fn icon_rgba() -> Vec<u8> {
    let n = ICON_SIZE as usize;
    let mut rgba = vec![0; n * n * 4];
    let c = ICON_SIZE as f32 / 2.0;
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let r = (px * px + py * py).sqrt();
            // Ring: radius 13, 2.5 px wide.
            let ring = 1.25 - (r - 13.0).abs();
            // Pendulum: from the centre up and to the right, 2 px wide.
            let (dx, dy) = (0.34_f32, -0.94_f32);
            let along = px * dx + py * dy;
            let across = (px * dy - py * dx).abs();
            let rod = if (-2.0..=10.0).contains(&along) {
                1.0 - across
            } else {
                f32::MIN
            };
            // Pivot weight at the centre.
            let weight = 3.0 - r;
            let coverage = (ring.max(rod).max(weight) + 0.5).clamp(0.0, 1.0);
            rgba[(y * n + x) * 4 + 3] = (coverage * 255.0).round() as u8;
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_actions() {
        let id = |s: &str| MenuId::new(s);
        assert_eq!(map_menu(&id(PLAY)), Some(Remote::TogglePlay));
        assert_eq!(map_menu(&id(BPM_UP)), Some(Remote::BpmStep(1)));
        assert_eq!(map_menu(&id(BPM_DOWN)), Some(Remote::BpmStep(-1)));
        assert_eq!(map_menu(&id(OPEN)), Some(Remote::Show(WindowMode::Full)));
        assert_eq!(map_menu(&id(MINI)), Some(Remote::Show(WindowMode::Mini)));
        assert_eq!(map_menu(&id(QUIT)), Some(Remote::Quit));
        assert_eq!(map_menu(&id("something-else")), None);
    }

    #[test]
    fn play_item_says_what_a_click_does() {
        assert_eq!(play_label(false), "Play");
        assert_eq!(play_label(true), "Stop");
    }

    #[test]
    fn icon_is_a_template_glyph() {
        let rgba = icon_rgba();
        let n = ICON_SIZE as usize;
        assert_eq!(rgba.len(), n * n * 4);
        let alpha = |x: usize, y: usize| rgba[(y * n + x) * 4 + 3];
        // Black (template), transparent corners, opaque ring and centre.
        assert!(rgba.chunks(4).all(|p| p[..3] == [0, 0, 0]));
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(n - 1, n - 1), 0);
        assert_eq!(alpha(n / 2, n / 2), 255);
        assert_eq!(alpha(n / 2, 3), 255, "top of the ring");
        assert_eq!(
            alpha(n / 2, n / 2 + 8),
            0,
            "inside the ring, below the pivot"
        );
    }
}
