//! A minimal egui host for iOS: a UIScene app, a `UIView` backed by
//! `CAMetalLayer`, egui-wgpu painting driven by `CADisplayLink`, touches as
//! egui pointer events, the safe area, the light/dark trait and the scene
//! lifecycle. It drives any [`pm_shell::AppShell`] and knows nothing about the
//! metronome.
//!
//! It exists because eframe's winit 0.30 has no UIScene lifecycle, which iOS
//! 27 requires (rust-windowing/winit#4723). Delete this crate once
//! winit/eframe support UIScene; it is self-contained for that reason.
//!
//! Only [`input`] and [`lifecycle`] (pure helpers) build on other platforms;
//! [`run`] is iOS only.

pub mod input;
pub mod lifecycle;

#[cfg(target_os = "ios")]
mod delegates;
#[cfg(target_os = "ios")]
mod render;
#[cfg(target_os = "ios")]
mod view;

#[cfg(target_os = "ios")]
pub use delegates::run;

/// The Objective-C class name of the host's `UIWindowSceneDelegate`, for
/// `UISceneDelegateClassName` in the app's Info.plist.
pub const SCENE_DELEGATE_CLASS: &str = "PMSceneDelegate";

/// Builds the app once the first scene has connected, given the host's
/// context. Called once: the app outlives scene disconnects.
pub type Factory = Box<dyn FnOnce(&egui::Context) -> Box<dyn pm_shell::AppShell>>;
