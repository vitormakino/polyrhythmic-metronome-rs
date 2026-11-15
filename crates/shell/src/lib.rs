//! The polyrhythmic metronome without its operating system: the frame loop
//! (audio ticks, beat events, device and visual offset following, action
//! handling, delayed settings saving) in [`Shell`], the engine commands in
//! [`controller`] and the saved [`settings`].
//!
//! A host drives an [`AppShell`]: the Mac app through [`EframeHost`] (feature
//! `eframe`), the iPhone app through its own bridge. Each host's app holds a
//! [`Shell`] and adds what only its platform has (menu bar item, hotkeys and
//! windows on the Mac).

pub mod controller;
pub mod settings;
mod shell;

#[cfg(feature = "eframe")]
mod eframe_host;

pub use controller::{Audio, Controller};
#[cfg(feature = "eframe")]
pub use eframe_host::EframeHost;
pub use settings::Settings;
pub use shell::{AppShell, Device, Shell, TICK, WindowSettings};
