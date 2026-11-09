//! Global hotkeys, active while another app has focus: ⌃⌥Space play/stop,
//! ⌃⌥T tap, ⌃⌥M full ⇄ mini.
//!
//! macOS: register them on the main thread once the event loop runs (the
//! first `App::logic` call). Presses arrive through [`forward`] as [`Remote`]s.

use std::sync::mpsc::Sender;

use eframe::egui;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::remote::Remote;

const MODS: Modifiers = Modifiers::CONTROL.union(Modifiers::ALT);

/// ⌃⌥Space.
pub fn play_stop() -> HotKey {
    HotKey::new(Some(MODS), Code::Space)
}

/// ⌃⌥T.
pub fn tap() -> HotKey {
    HotKey::new(Some(MODS), Code::KeyT)
}

/// ⌃⌥M.
pub fn toggle_mini() -> HotKey {
    HotKey::new(Some(MODS), Code::KeyM)
}

/// What the hotkey with this id asks for.
pub fn map_hotkey(id: u32) -> Option<Remote> {
    [
        (play_stop(), Remote::TogglePlay),
        (tap(), Remote::Tap),
        (toggle_mini(), Remote::ToggleMini),
    ]
    .into_iter()
    .find_map(|(hotkey, remote)| (hotkey.id() == id).then_some(remote))
}

/// Registers the default hotkeys. A hotkey another app already holds is
/// reported on stderr and skipped; the others still work.
pub fn register_hotkeys() -> Result<GlobalHotKeyManager, global_hotkey::Error> {
    let manager = GlobalHotKeyManager::new()?;
    for (name, hotkey) in [
        ("play/stop", play_stop()),
        ("tap", tap()),
        ("mini", toggle_mini()),
    ] {
        if let Err(e) = manager.register(hotkey) {
            eprintln!("polyrhythmic-metronome: hotkey {name} ({hotkey}) unavailable: {e}");
        }
    }
    Ok(manager)
}

/// Sends hotkey presses (not releases) to `tx` and wakes the app, also while
/// its window is hidden.
pub fn forward(tx: Sender<Remote>, ctx: egui::Context) {
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state == HotKeyState::Pressed
            && let Some(remote) = map_hotkey(event.id)
        {
            let _ = tx.send(remote);
            ctx.request_repaint();
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_map_to_actions() {
        assert_eq!(map_hotkey(play_stop().id()), Some(Remote::TogglePlay));
        assert_eq!(map_hotkey(tap().id()), Some(Remote::Tap));
        assert_eq!(map_hotkey(toggle_mini().id()), Some(Remote::ToggleMini));
        // Same key without the modifiers, or another key: not ours.
        assert_eq!(map_hotkey(HotKey::new(None, Code::Space).id()), None);
        assert_eq!(map_hotkey(HotKey::new(Some(MODS), Code::KeyX).id()), None);
    }

    #[test]
    fn defaults_are_control_option_space_t_m() {
        assert_eq!(play_stop().into_string(), "control+alt+Space");
        assert_eq!(tap().into_string(), "control+alt+KeyT");
        assert_eq!(toggle_mini().into_string(), "control+alt+KeyM");
    }
}
