//! How the engine's session progress reaches the UI: a seqlock over atomics,
//! like the position cell. The audio thread writes, wait-free; readers retry
//! while a write is in progress.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering, fence};

use pm_core::session::{PROGRESS_WORDS, SessionProgress};

#[derive(Default)]
struct SessionCell {
    seq: AtomicU64,
    present: AtomicBool,
    /// Engine frame of the instant the progress describes (its latest bar
    /// or beat).
    frame: AtomicU64,
    words: [AtomicU64; PROGRESS_WORDS],
}

/// Engine side of the session cell.
pub(crate) struct SessionWriter(Arc<SessionCell>);

/// UI side: the latest [`SessionProgress`] the engine published.
#[derive(Clone)]
pub struct SessionReader(Arc<SessionCell>);

pub(crate) fn session_cell() -> (SessionWriter, SessionReader) {
    let cell = Arc::new(SessionCell::default());
    (SessionWriter(Arc::clone(&cell)), SessionReader(cell))
}

impl SessionWriter {
    /// Publishes `progress`, taken at engine frame `frame`, or clears the
    /// cell with `None`. Never blocks or allocates.
    pub(crate) fn publish(&self, progress: Option<(u64, &SessionProgress)>) {
        let c = &*self.0;
        let seq = c.seq.load(Ordering::Relaxed);
        c.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        c.present.store(progress.is_some(), Ordering::Relaxed);
        if let Some((frame, progress)) = progress {
            c.frame.store(frame, Ordering::Relaxed);
            for (slot, word) in c.words.iter().zip(progress.to_words()) {
                slot.store(word, Ordering::Relaxed);
            }
        }
        c.seq.store(seq.wrapping_add(2), Ordering::Release);
    }
}

impl SessionReader {
    /// The latest progress the engine published: at every reference beat
    /// while a session runs, and when it finishes. `None` until a session
    /// runs, and again after a `Play` without a plan.
    pub fn latest(&self) -> Option<SessionProgress> {
        self.latest_at().map(|(_, progress)| progress)
    }

    /// [`latest`](Self::latest) with the engine frame it was taken at.
    pub(crate) fn latest_at(&self) -> Option<(u64, SessionProgress)> {
        let c = &*self.0;
        loop {
            let before = c.seq.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let present = c.present.load(Ordering::Relaxed);
            let frame = c.frame.load(Ordering::Relaxed);
            let words: [u64; PROGRESS_WORDS] =
                std::array::from_fn(|i| c.words[i].load(Ordering::Relaxed));
            fence(Ordering::Acquire);
            if c.seq.load(Ordering::Relaxed) == before {
                return present.then(|| (frame, SessionProgress::from_words(&words)));
            }
        }
    }
}
