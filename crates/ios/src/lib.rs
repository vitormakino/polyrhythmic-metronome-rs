//! The iPhone app: [`pm_ios_main`] (iOS only) sets up the audio session,
//! loads the settings, picks the language and runs [`IosApp`] in the iOS
//! host. The decisions ([`language_for`], [`theme_for`],
//! [`on_interruption`], [`output_gate`]) and [`IosApp`] build on any
//! platform, for the tests.
//!
//! Threads: everything runs on the main thread except the audio callback
//! and the session's activations and deactivations (on its own serial
//! queue); the session's notifications and activation results reach the app
//! through a channel drained in `logic`.

pub mod app;
pub mod audio_session;
pub mod locale;
pub mod output_gate;

pub use app::{IosApp, Output, SessionLink, theme_for};
pub use audio_session::{Interruption, SessionEvent, interruption_from, on_interruption};
pub use locale::language_for;

// Debug builds abort on an allocation inside the audio callback (the
// `assert_no_alloc` guard in `pm_audio`'s data callback), as on the Mac;
// release builds use the system allocator unchanged.
#[cfg(all(target_os = "ios", debug_assertions))]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// The app's entry point, called from the Xcode project's `main`. Never returns.
///
/// This is the one place that picks the frame the app runs in: today
/// `pm_ios_host`, because eframe's winit has no UIScene lifecycle, which iOS
/// 27 requires. Once winit supports UIScene (rust-windowing/winit#4723) and
/// eframe adopts it, switching takes every step of "Switching to eframe" in
/// the README:
///
/// 1. Run the app here through `pm_shell::EframeHost`
///    (`eframe::run_native`) instead of `pm_ios_host::run`.
/// 2. In `crates/ios/Cargo.toml`, drop the `polyrhythmic-metronome-ios-host`
///    dependency, enable `polyrhythmic-metronome-shell`'s `eframe` feature
///    and add `eframe`.
/// 3. `EframeHost` never calls [`AppShell::on_background`] and nothing wakes
///    `logic` off screen: hook the app's suspend to `on_background` (it
///    saves the settings) and add a background ticker that asks for a
///    repaint every `pm_shell::TICK`, as the Mac app's `ui-tick` thread does.
/// 4. In `ios/Info.plist`, set `UISceneDelegateClassName` to winit's scene
///    delegate, or remove it if winit installs its own.
/// 5. Delete `crates/ios_host`, remove it from the root `Cargo.toml`
///    `members`, drop `-p polyrhythmic-metronome-ios-host` from the `ios` job
///    in `.github/workflows/ci.yml` and its row from the README's project
///    layout table.
///
/// [`AppShell::on_background`]: pm_shell::AppShell::on_background
#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn pm_ios_main() -> ! {
    use std::rc::Rc;
    use std::sync::mpsc;

    use pm_ui::i18n::Messages;
    use pm_ui::theme;

    use crate::audio_session::AudioSession;

    // Before the stream opens, so it opens in the session's category.
    // Nothing is activated until the first Play.
    let (events_tx, events) = mpsc::channel();
    let session = Rc::new(AudioSession::start(events_tx));

    // `dirs::data_dir()` is `$HOME/Library/Application Support`, and iOS
    // sets `$HOME` to the app's data container: the file stays inside the
    // sandbox and survives reinstalls with the same bundle identifier.
    let (settings_dir, settings) = pm_shell::load_settings();
    let language = language_for(&locale::preferred_languages());
    let messages = Messages::builtin(language).unwrap_or_else(Messages::english);

    pm_ios_host::run(Box::new(move |ctx| {
        theme::install(ctx);
        // Opened paused: starting an output unit would activate the
        // session, synchronously, on this (the main) thread.
        let audio = pm_shell::start_audio_suspended(&settings);
        let (on, off) = (Rc::clone(&session), session);
        let link = SessionLink {
            events,
            activate: Box::new(move || on.activate()),
            deactivate: Box::new(move || off.deactivate()),
        };
        Box::new(IosApp::new(audio, &settings, settings_dir, messages, link))
    }))
}
