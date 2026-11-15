//! The app's `AVAudioSession`: category `playback` with `mixWithOthers`
//! (plays with the silent switch on, with the screen locked thanks to
//! `UIBackgroundModes` = `audio`, and over music from other apps), activated
//! only while playing (see [`crate::output_gate`]), off the main thread, and
//! its events forwarded to the app.
//!
//! The decisions are pure and build anywhere ([`Interruption`],
//! [`interruption_from`], [`on_interruption`]); the Objective-C side,
//! `AudioSession`, is iOS only.
//!
//! Route changes (headphones in or out) are not observed here: `pm_audio`
//! reads the session's current route on every service tick and rebuilds the
//! stream on the new output, the engine continuing from the same point.
//!
//! cpal 0.18.2 (`host::coreaudio::ios`) never sets the category: it only
//! sets the preferred IO buffer duration, so ours stands. Its own
//! interruption observer stops the unit when one begins and, when it ends
//! with "should resume", activates the session synchronously and restarts
//! the unit only if it was running when the interruption began. The app
//! pauses the stream right after an interruption begins (so cpal's restart
//! is skipped) and deactivates again what cpal activated.

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

/// What the session tells the app, on the main thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Interruption(Interruption),
    /// Activation `id`, which the app asked for, finished: the session is
    /// active, or why not.
    Activated {
        id: u64,
        result: Result<(), String>,
    },
    /// The system's media services restarted
    /// (`AVAudioSessionMediaServicesWereResetNotification`): the category
    /// is set again, the session is inactive.
    MediaReset,
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
    use dispatch2::{
        DispatchQoS, DispatchQueue, DispatchQueueAttr, DispatchRetained, GlobalQueueIdentifier,
    };
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, ProtocolObject};
    use objc2::sel;
    use objc2_avf_audio::{
        AVAudioSession, AVAudioSessionActivationOptions, AVAudioSessionCategoryOptions,
        AVAudioSessionCategoryPlayback, AVAudioSessionInterruptionNotification,
        AVAudioSessionInterruptionOptionKey, AVAudioSessionInterruptionTypeKey,
        AVAudioSessionMediaServicesWereResetNotification, AVAudioSessionSetActiveOptions,
    };
    use objc2_foundation::{
        NSError, NSNotification, NSNotificationCenter, NSNotificationName, NSNumber,
        NSObjectProtocol, NSOperationQueue, NSString,
    };

    use super::{ActivationPath, SessionEvent, activation_path, interruption_from};

    type Observer = Retained<ProtocolObject<dyn NSObjectProtocol>>;

    /// The configured session, its notification observers (removed on drop)
    /// and the queue its activations and deactivations run on.
    pub struct AudioSession {
        observers: Vec<Observer>,
        /// Serial: a deactivation and the activation asked for right after
        /// it run in that order.
        queue: DispatchRetained<DispatchQueue>,
        events: Sender<SessionEvent>,
    }

    impl AudioSession {
        /// Sets the category and starts forwarding the session's events to
        /// `events`, on the main thread (the observers run on the main
        /// queue). Activates nothing: see [`activate`](Self::activate).
        /// Call before the audio stream opens, so it opens in this category.
        pub fn start(events: Sender<SessionEvent>) -> AudioSession {
            configure();
            let tx = events.clone();
            let on_interruption = RcBlock::new(move |note: NonNull<NSNotification>| {
                // SAFETY: the center passes a valid notification.
                let note = unsafe { note.as_ref() };
                if let Some(i) = parse(note) {
                    // The app is gone if the receiver is: nothing to stop.
                    let _ = tx.send(SessionEvent::Interruption(i));
                }
            });
            // The system's media services restarted: every session setting
            // is lost (Apple's guidance is to set it up again). The stream
            // fails and `pm_audio` rebuilds it; the app activates again if
            // it holds the audio.
            let tx = events.clone();
            let on_reset = RcBlock::new(move |_note: NonNull<NSNotification>| {
                configure();
                let _ = tx.send(SessionEvent::MediaReset);
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
            let queue = DispatchQueue::new_with_target(
                "polyrhythmic-metronome.audio-session",
                DispatchQueueAttr::SERIAL,
                Some(&DispatchQueue::global_queue(
                    GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated),
                )),
            );
            AudioSession {
                observers,
                queue,
                events,
            }
        }

        /// Activates the session without blocking the caller (activation can
        /// take a while; Apple advises against a synchronous activation on
        /// the main thread): the asynchronous API where the system has it
        /// (iOS 27), else the synchronous one, both from the session's
        /// queue. Answers with [`SessionEvent::Activated`] carrying `id`.
        pub fn activate(&self, id: u64) {
            let events = self.events.clone();
            self.queue.exec_async(move || {
                // SAFETY: the shared session, a thread-safe singleton.
                let session = unsafe { AVAudioSession::sharedInstance() };
                let has_async =
                    session.respondsToSelector(sel!(activateWithOptions:completionHandler:));
                match activation_path(has_async) {
                    ActivationPath::Async => {
                        let done = RcBlock::new(move |activated: Bool, error: *mut NSError| {
                            let result = if activated.as_bool() {
                                Ok(())
                            } else {
                                // SAFETY: AVFAudio passes a valid error or null.
                                Err(reason(unsafe { error.as_ref() }))
                            };
                            let _ = events.send(SessionEvent::Activated { id, result });
                        });
                        // SAFETY: the shared session, which answers the
                        // selector (checked above); the block is `'static`
                        // and only prints and sends (AVFAudio may call it on
                        // any queue).
                        unsafe {
                            session.activateWithOptions_completionHandler(
                                AVAudioSessionActivationOptions::None,
                                &done,
                            );
                        }
                    }
                    ActivationPath::BackgroundQueue => {
                        // SAFETY: the shared session is thread-safe.
                        let result =
                            unsafe { session.setActive_error(true) }.map_err(|e| reason(Some(&e)));
                        let _ = events.send(SessionEvent::Activated { id, result });
                    }
                }
            });
        }

        /// Deactivates the session from its queue, telling the other apps
        /// they may resume. Pause the stream first: the session must have no
        /// running I/O.
        pub fn deactivate(&self) {
            self.queue.exec_async(|| {
                // SAFETY: the shared session is thread-safe.
                let result = unsafe {
                    AVAudioSession::sharedInstance().setActive_withOptions_error(
                        false,
                        AVAudioSessionSetActiveOptions::NotifyOthersOnDeactivation,
                    )
                };
                if let Err(e) = result {
                    eprintln!(
                        "polyrhythmic-metronome: audio session not deactivated: {}",
                        reason(Some(&e))
                    );
                }
            });
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

    /// Category `playback`, mixing with other apps' audio. A plain setter,
    /// quick enough for the main thread (activation is the slow part).
    fn configure() {
        // SAFETY: AVFAudio's category constant and the shared session.
        let result = unsafe {
            let Some(playback) = AVAudioSessionCategoryPlayback else {
                return;
            };
            AVAudioSession::sharedInstance().setCategory_withOptions_error(
                playback,
                AVAudioSessionCategoryOptions::MixWithOthers,
            )
        };
        if let Err(e) = result {
            eprintln!(
                "polyrhythmic-metronome: audio session category: {}",
                e.localizedDescription()
            );
        }
    }

    /// What went wrong, for the log.
    fn reason(error: Option<&NSError>) -> String {
        error.map_or_else(
            || "unknown error".to_owned(),
            |e| e.localizedDescription().to_string(),
        )
    }

    /// The interruption in `note`'s `userInfo`.
    fn parse(note: &NSNotification) -> Option<super::Interruption> {
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
