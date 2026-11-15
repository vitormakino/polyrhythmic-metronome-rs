//! What every host does before its window opens: load the saved settings
//! and start the audio from them.

use std::path::PathBuf;

use pm_audio::{AudioError, AudioService, EngineInit};

use crate::settings::{self, Settings};

/// The settings directory ([`settings::default_dir`]) and the settings saved
/// there (the defaults when there is none or nothing is saved).
pub fn load_settings() -> (Option<PathBuf>, Settings) {
    let dir = settings::default_dir();
    let settings = dir.as_deref().map(Settings::load).unwrap_or_default();
    (dir, settings)
}

/// Starts the audio on the default output with the saved pattern and tempo.
/// Exits the process when that fails, which happens only when an output
/// refuses every configuration: nothing the app could show would make it
/// play.
pub fn start_audio(settings: &Settings) -> AudioService {
    start(settings, AudioService::start)
}

/// [`start_audio`] with the stream opened but not started (see
/// `AudioService::start_suspended`): for the iPhone app, which starts it
/// only once its audio session is active.
pub fn start_audio_suspended(settings: &Settings) -> AudioService {
    start(settings, AudioService::start_suspended)
}

fn start(
    settings: &Settings,
    start: impl FnOnce(EngineInit) -> Result<AudioService, AudioError>,
) -> AudioService {
    match start(EngineInit {
        pattern: settings.pattern.clone(),
        bpm: settings.bpm,
        resume: None,
        session: None,
    }) {
        Ok(audio) => audio,
        Err(e) => {
            eprintln!("polyrhythmic-metronome: {e}");
            std::process::exit(1);
        }
    }
}
