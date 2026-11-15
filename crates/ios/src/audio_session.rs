//! The app's `AVAudioSession`: category `playback` (plays with the silent
//! switch on and, with `UIBackgroundModes` = `audio`, with the screen
//! locked), activated asynchronously, and its interruptions forwarded to the
//! app.
//!
//! The decisions are pure and build anywhere ([`Interruption`],
//! [`interruption_from`], [`on_interruption`]); the Objective-C side,
//! `AudioSession`, is iOS only.
//!
//! Route changes (headphones in or out) are not observed here: `pm_audio`
//! reads the session's current route on every service tick and rebuilds the
//! stream on the new output, the engine continuing from the same point.

use pm_ui::state::UiAction;

/// An `AVAudioSessionInterruptionNotification`: a call, an alarm or another
/// app took the audio (`Began`), or gave it back (`Ended`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interruption {
    Began,
    /// `should_resume`: the system suggests resuming. The metronome never
    /// does (starting again on its own after a call would startle).
    Ended {
        should_resume: bool,
    },
}

/// `AVAudioSessionInterruptionTypeBegan` / `Ended` (AVAudioSessionTypes.h).
const TYPE_BEGAN: usize = 1;
const TYPE_ENDED: usize = 0;
/// `AVAudioSessionInterruptionOptionShouldResume`.
const OPTION_SHOULD_RESUME: usize = 1;

/// The interruption a notification's `userInfo` describes, from the raw
/// values of `AVAudioSessionInterruptionTypeKey` and
/// `AVAudioSessionInterruptionOptionKey`. `None` for a missing or unknown type.
pub fn interruption_from(kind: Option<usize>, options: Option<usize>) -> Option<Interruption> {
    match kind? {
        TYPE_BEGAN => Some(Interruption::Began),
        TYPE_ENDED => Some(Interruption::Ended {
            should_resume: options.unwrap_or(0) & OPTION_SHOULD_RESUME != 0,
        }),
        _ => None,
    }
}

/// What the app does on an interruption, given whether it is `playing`:
/// stop (the transport button's [`UiAction::TogglePlay`]) when it begins
/// while playing; nothing otherwise, and never resume when it ends.
pub fn on_interruption(i: Interruption, playing: bool) -> Option<UiAction> {
    match i {
        Interruption::Began if playing => Some(UiAction::TogglePlay),
        Interruption::Began | Interruption::Ended { .. } => None,
    }
}

/// How the session is activated off the caller's thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationPath {
    /// `activateWithOptions:completionHandler:` (iOS 27 and later).
    Async,
    /// `setActive:error:` on a background dispatch queue (older systems).
    BackgroundQueue,
}

/// The activation path, given whether the session answers
/// `activateWithOptions:completionHandler:`. The app runs from iOS 17, where
/// calling it would be an unrecognized selector.
pub fn activation_path(has_async: bool) -> ActivationPath {
    if has_async {
        ActivationPath::Async
    } else {
        ActivationPath::BackgroundQueue
    }
}

#[cfg(target_os = "ios")]
pub use native::AudioSession;

#[cfg(target_os = "ios")]
mod native {
    use std::ptr::NonNull;
    use std::sync::mpsc::Sender;

    use block2::RcBlock;
    use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, ProtocolObject};
    use objc2::sel;
    use objc2_avf_audio::{
        AVAudioSession, AVAudioSessionActivationOptions, AVAudioSessionCategoryPlayback,
        AVAudioSessionInterruptionNotification, AVAudioSessionInterruptionOptionKey,
        AVAudioSessionInterruptionTypeKey, AVAudioSessionMediaServicesWereResetNotification,
    };
    use objc2_foundation::{
        NSError, NSNotification, NSNotificationCenter, NSNotificationName, NSNumber,
        NSObjectProtocol, NSOperationQueue, NSString,
    };

    use super::{ActivationPath, Interruption, activation_path, interruption_from};

    type Observer = Retained<ProtocolObject<dyn NSObjectProtocol>>;

    /// The configured session and its notification observers (removed on drop).
    pub struct AudioSession {
        observers: Vec<Observer>,
    }

    impl AudioSession {
        /// Sets the category, asks for activation and starts forwarding
        /// interruptions to `interruptions`, on the main thread (the
        /// observers run on the main queue). Call before the audio stream
        /// opens, so it opens in the `playback` category.
        pub fn start(interruptions: Sender<Interruption>) -> AudioSession {
            configure();
            activate();
            let on_interruption = RcBlock::new(move |note: NonNull<NSNotification>| {
                // SAFETY: the center passes a valid notification.
                let note = unsafe { note.as_ref() };
                if let Some(i) = parse(note) {
                    // The app is gone if the receiver is: nothing to stop.
                    let _ = interruptions.send(i);
                }
            });
            // The system's media services restarted: every session setting
            // is lost (Apple's guidance is to set it up again). The stream
            // stalls and `pm_audio` rebuilds it.
            let on_reset = RcBlock::new(|_note: NonNull<NSNotification>| {
                configure();
                activate();
            });
            // SAFETY: AVFAudio's notification name constants.
            let (interruption, reset) = unsafe {
                (
                    AVAudioSessionInterruptionNotification,
                    AVAudioSessionMediaServicesWereResetNotification,
                )
            };
            let mut observers = Vec::new();
            if let Some(name) = interruption {
                observers.push(observe(name, &on_interruption));
            }
            if let Some(name) = reset {
                observers.push(observe(name, &on_reset));
            }
            AudioSession { observers }
        }

        /// Asks for activation again, before playing: an interruption leaves
        /// the session inactive. Cheap when it is already active.
        pub fn activate(&self) {
            activate();
        }
    }

    impl Drop for AudioSession {
        fn drop(&mut self) {
            let center = NSNotificationCenter::defaultCenter();
            for observer in &self.observers {
                // SAFETY: a token returned by this center's
                // `addObserverForName:object:queue:usingBlock:`.
                unsafe { center.removeObserver(observer.as_ref()) };
            }
        }
    }

    /// Runs `block` on the main queue for every `name` notification.
    fn observe(
        name: &NSNotificationName,
        block: &block2::DynBlock<dyn Fn(NonNull<NSNotification>)>,
    ) -> Observer {
        // SAFETY: the block is `'static` and only touches what it owns; the
        // center keeps a copy of it until the observer is removed.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                Some(&NSOperationQueue::mainQueue()),
                block,
            )
        }
    }

    /// Category `playback`. A plain setter, quick enough for the main thread
    /// (activation is the slow part).
    fn configure() {
        // SAFETY: AVFAudio's category constant and the shared session.
        let result = unsafe {
            let Some(playback) = AVAudioSessionCategoryPlayback else {
                return;
            };
            AVAudioSession::sharedInstance().setCategory_error(playback)
        };
        if let Err(e) = result {
            eprintln!(
                "polyrhythmic-metronome: audio session category: {}",
                e.localizedDescription()
            );
        }
    }

    /// Activates the session without blocking the caller (activation can take
    /// a while; Apple advises against a synchronous activation on the main
    /// thread): the asynchronous API where the system has it (iOS 27),
    /// else the synchronous one on a background queue. The outcome is only
    /// reported: the stream follows the session on its own.
    fn activate() {
        // SAFETY: the shared session, a thread-safe singleton.
        let session = unsafe { AVAudioSession::sharedInstance() };
        let has_async = session.respondsToSelector(sel!(activateWithOptions:completionHandler:));
        match activation_path(has_async) {
            ActivationPath::Async => {
                let done = RcBlock::new(|activated: Bool, error: *mut NSError| {
                    if !activated.as_bool() {
                        // SAFETY: AVFAudio passes a valid error or null.
                        report(unsafe { error.as_ref() });
                    }
                });
                // SAFETY: the shared session, which answers the selector
                // (checked above); the block is `'static` and only prints
                // (AVFAudio may call it on any queue).
                unsafe {
                    session.activateWithOptions_completionHandler(
                        AVAudioSessionActivationOptions::None,
                        &done,
                    );
                }
            }
            ActivationPath::BackgroundQueue => {
                let queue = DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(
                    DispatchQoS::UserInitiated,
                ));
                queue.exec_async(|| {
                    // SAFETY: the shared session is thread-safe.
                    let result = unsafe { AVAudioSession::sharedInstance().setActive_error(true) };
                    if let Err(e) = result {
                        report(Some(&e));
                    }
                });
            }
        }
    }

    fn report(error: Option<&NSError>) {
        let reason = error.map_or_else(
            || "unknown error".to_owned(),
            |e| e.localizedDescription().to_string(),
        );
        eprintln!("polyrhythmic-metronome: audio session not activated: {reason}");
    }

    /// The interruption in `note`'s `userInfo`.
    fn parse(note: &NSNotification) -> Option<Interruption> {
        let info = note.userInfo()?;
        let number = |key: Option<&NSString>| -> Option<usize> {
            let value = info.objectForKey(key?)?;
            Some(value.downcast::<NSNumber>().ok()?.unsignedIntegerValue())
        };
        // SAFETY: AVFAudio's userInfo key constants.
        let (kind, options) = unsafe {
            (
                AVAudioSessionInterruptionTypeKey,
                AVAudioSessionInterruptionOptionKey,
            )
        };
        interruption_from(number(kind), number(options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interruption_stops_and_never_resumes() {
        assert_eq!(
            on_interruption(Interruption::Began, true),
            Some(UiAction::TogglePlay)
        );
        assert_eq!(on_interruption(Interruption::Began, false), None);
        let ended = Interruption::Ended {
            should_resume: true,
        };
        assert_eq!(on_interruption(ended, false), None);
        assert_eq!(on_interruption(ended, true), None);
    }

    #[test]
    fn activation_falls_back_without_the_async_api() {
        assert_eq!(activation_path(true), ActivationPath::Async);
        assert_eq!(activation_path(false), ActivationPath::BackgroundQueue);
    }

    #[test]
    fn interruption_user_info_is_read() {
        assert_eq!(interruption_from(Some(1), None), Some(Interruption::Began));
        assert_eq!(
            interruption_from(Some(0), Some(1)),
            Some(Interruption::Ended {
                should_resume: true
            })
        );
        assert_eq!(
            interruption_from(Some(0), None),
            Some(Interruption::Ended {
                should_resume: false
            })
        );
        assert_eq!(interruption_from(None, Some(1)), None);
        assert_eq!(interruption_from(Some(7), None), None);
    }
}
