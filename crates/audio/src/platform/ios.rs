//! iOS: identity and latency from the shared `AVAudioSession`. A headphone
//! plug or unplug changes the first output of the current route, so it shows
//! up as a default-device change to the watcher.

use std::time::Duration;

use objc2_avf_audio::AVAudioSession;

use crate::stream::OutputLatency;

/// UID and port name of the first output of the current route.
pub(crate) fn output_identity(_device: &cpal::Device) -> Option<(String, String)> {
    current_output()
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

/// Output latency of the current route: `outputLatency` as the device part
/// and `IOBufferDuration` as the stream part. The route has one output at a
/// time, so `_uid` is not used to look anything up.
pub fn query_output_latency(_uid: &str) -> Option<OutputLatency> {
    // SAFETY: plain property reads on the shared session.
    let (device, buffer) = unsafe {
        let session = AVAudioSession::sharedInstance();
        (session.outputLatency(), session.IOBufferDuration())
    };
    let seconds = |s: f64| (s.is_finite() && s >= 0.0).then(|| Duration::from_secs_f64(s));
    Some(OutputLatency {
        device_and_safety: seconds(device)?,
        stream: seconds(buffer)?,
    })
}
