//! iOS: identity and latency from the shared `AVAudioSession`. A headphone
//! plug or unplug changes the first output of the current route, so it shows
//! up as a default-device change to the watcher.

use std::time::Duration;

use objc2_avf_audio::AVAudioSession;

use crate::stream::OutputLatency;

/// UID and port name of the first output of the current route; a route
/// without outputs is a device not available (worth retrying).
pub(crate) fn output_identity(_device: &cpal::Device) -> Result<(String, String), cpal::Error> {
    current_output().ok_or_else(|| cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable))
}

fn current_output() -> Option<(String, String)> {
    // SAFETY: `sharedInstance` and the route getters are thread-safe
    // AVFoundation reads that return retained objects.
    unsafe {
        let session = AVAudioSession::sharedInstance();
        let outputs = session.currentRoute().outputs();
        let port = outputs.firstObject()?;
        Some((port.UID().to_string(), port.portName().to_string()))
    }
}

/// Output latency of the current route, all of it in `device_and_safety`
/// (`outputLatency` + `IOBufferDuration`) and `stream` zero: cpal 0.18.2
/// already adds both to `OutputCallbackInfo::timestamp().playback` on iOS
/// (`cpal::host::coreaudio::ios`, `output_latency_frames`: the IO buffer
/// frames plus `outputLatency`), so nothing is left uncompensated. The route
/// has one output at a time, so `_uid` is not used to look anything up.
pub fn query_output_latency(_uid: &str) -> Option<OutputLatency> {
    // SAFETY: plain property reads on the shared session.
    let (device, buffer) = unsafe {
        let session = AVAudioSession::sharedInstance();
        (session.outputLatency(), session.IOBufferDuration())
    };
    let seconds = |s: f64| (s.is_finite() && s >= 0.0).then(|| Duration::from_secs_f64(s));
    Some(OutputLatency {
        device_and_safety: seconds(device)? + seconds(buffer)?,
        stream: Duration::ZERO,
    })
}
