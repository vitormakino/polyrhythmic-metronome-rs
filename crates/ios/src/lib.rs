//! The iPhone app: [`pm_ios_main`] (iOS only) sets up the audio session,
//! loads the settings, picks the language and runs [`IosApp`] in the iOS
//! host. The decisions ([`language_for`], [`theme_for`],
//! [`on_interruption`]) and [`IosApp`] build on any platform, for the tests.
//!
//! Threads: everything runs on the main thread except the audio callback;
//! the session's notifications arrive on the main queue and reach the app
//! through a channel drained in `logic`.

pub mod app;
pub mod audio_session;
pub mod locale;

pub use app::{IosApp, SessionLink, theme_for};
pub use audio_session::{Interruption, interruption_from, on_interruption};
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
/// eframe adopts it, switch by making this function run the app through
/// `pm_shell::EframeHost` (`eframe::run_native`) instead of
/// `pm_ios_host::run`, then delete `crates/ios_host`.
#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn pm_ios_main() -> ! {
    use std::sync::mpsc;

    use pm_audio::{AudioService, EngineInit};
    use pm_shell::{Settings, settings};
    use pm_ui::i18n::Messages;
    use pm_ui::theme;

    use crate::audio_session::AudioSession;

    // Before the stream opens, so it opens in the `playback` category.
    let (interruptions_tx, interruptions) = mpsc::channel();
    let session = AudioSession::start(interruptions_tx);

    // `dirs::data_dir()` is `$HOME/Library/Application Support`, and iOS
    // sets `$HOME` to the app's data container: the file stays inside the
    // sandbox and survives reinstalls with the same bundle identifier.
    let settings_dir = settings::default_dir();
    let settings = settings_dir
        .as_deref()
        .map(Settings::load)
        .unwrap_or_default();
    let language = language_for(&locale::preferred_languages());
    let messages = Messages::builtin(language).unwrap_or_else(Messages::english);

    pm_ios_host::run(Box::new(move |ctx| {
        theme::install(ctx);
        let audio = match AudioService::start(EngineInit {
            pattern: settings.pattern.clone(),
            bpm: settings.bpm,
            resume: None,
            session: None,
        }) {
            Ok(audio) => audio,
            Err(e) => {
                // Only when an output refuses every configuration: nothing
                // the app could show would make it play.
                eprintln!("polyrhythmic-metronome: {e}");
                std::process::exit(1);
            }
        };
        let link = SessionLink {
            interruptions,
            activate: Box::new(move || session.activate()),
        };
        Box::new(IosApp::new(audio, &settings, settings_dir, messages, link))
    }))
}
