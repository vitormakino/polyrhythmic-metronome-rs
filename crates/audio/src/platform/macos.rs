//! macOS: identity from cpal, latency from the CoreAudio HAL.

use std::time::Duration;

use cpal::traits::DeviceTrait;

use crate::stream::OutputLatency;

/// CoreAudio UID and display name of `device`.
pub(crate) fn output_identity(device: &cpal::Device) -> Option<(String, String)> {
    let uid = device.id().ok()?.id().to_owned();
    let name = device
        .description()
        .map_or_else(|_| uid.clone(), |d| d.name().to_owned());
    Some((uid, name))
}

/// Output latency of the device with CoreAudio UID `device_uid`:
/// `kAudioDevicePropertyLatency` + `kAudioDevicePropertySafetyOffset` (output
/// scope) and `kAudioStreamPropertyLatency` of its first output stream, in
/// frames at the device's nominal sample rate. `None` if the device is not
/// found or a query fails. The only `unsafe` code in the workspace.
pub fn query_output_latency(device_uid: &str) -> Option<OutputLatency> {
    use std::ffi::c_void;
    use std::ptr::{self, NonNull};

    use objc2_core_audio::{
        AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
        AudioObjectPropertyScope, AudioObjectPropertySelector, kAudioDevicePropertyLatency,
        kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
        kAudioDevicePropertyStreams, kAudioHardwarePropertyTranslateUIDToDevice,
        kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject, kAudioObjectUnknown,
        kAudioStreamPropertyLatency,
    };
    use objc2_core_foundation::CFString;

    /// Reads a property into `out`; returns the bytes written.
    fn get(
        object: AudioObjectID,
        selector: AudioObjectPropertySelector,
        scope: AudioObjectPropertyScope,
        qualifier: Option<&CFString>,
        out: NonNull<c_void>,
        capacity: u32,
    ) -> Option<u32> {
        let address = AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: scope,
            mElement: kAudioObjectPropertyElementMain,
        };
        let qualifier_ref: *const CFString = qualifier.map_or(ptr::null(), ptr::from_ref);
        let (qualifier_size, qualifier_data) = if qualifier.is_some() {
            (
                size_of::<*const CFString>() as u32,
                ptr::from_ref(&qualifier_ref).cast::<c_void>(),
            )
        } else {
            (0, ptr::null())
        };
        let mut size = capacity;
        // SAFETY: `address` and `size` live across the call, `out` points to
        // `capacity` writable bytes (the caller's local), and the qualifier
        // is null or a pointer to a live CFStringRef, as
        // kAudioHardwarePropertyTranslateUIDToDevice expects.
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&address),
                qualifier_size,
                qualifier_data,
                NonNull::from(&mut size),
                out,
            )
        };
        (status == 0).then_some(size)
    }
    let u32_of = |object, selector, scope, qualifier| {
        let mut v: u32 = 0;
        let n = get(
            object,
            selector,
            scope,
            qualifier,
            NonNull::from(&mut v).cast(),
            size_of::<u32>() as u32,
        )?;
        (n == size_of::<u32>() as u32).then_some(v)
    };

    let uid = CFString::from_str(device_uid);
    let device = u32_of(
        kAudioObjectSystemObject as AudioObjectID,
        kAudioHardwarePropertyTranslateUIDToDevice,
        kAudioObjectPropertyScopeGlobal,
        Some(&uid),
    )
    .filter(|&id| id != kAudioObjectUnknown)?;
    let output = kAudioObjectPropertyScopeOutput;
    let device_latency = u32_of(device, kAudioDevicePropertyLatency, output, None)?;
    let safety_offset = u32_of(device, kAudioDevicePropertySafetyOffset, output, None)?;
    let mut streams = [0 as AudioObjectID; 16];
    let bytes = get(
        device,
        kAudioDevicePropertyStreams,
        output,
        None,
        NonNull::from(&mut streams).cast(),
        size_of_val(&streams) as u32,
    )?;
    let stream_latency = if bytes >= size_of::<AudioObjectID>() as u32 {
        u32_of(
            streams[0],
            kAudioStreamPropertyLatency,
            kAudioObjectPropertyScopeGlobal,
            None,
        )?
    } else {
        0
    };
    let mut rate: f64 = 0.0;
    let n = get(
        device,
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
        None,
        NonNull::from(&mut rate).cast(),
        size_of::<f64>() as u32,
    )?;
    if n != size_of::<f64>() as u32 || !rate.is_finite() || rate <= 0.0 {
        return None;
    }
    let seconds = |frames: f64| Duration::from_secs_f64(frames / rate);
    Some(OutputLatency {
        device_and_safety: seconds(f64::from(device_latency) + f64::from(safety_offset)),
        stream: seconds(f64::from(stream_latency)),
    })
}
