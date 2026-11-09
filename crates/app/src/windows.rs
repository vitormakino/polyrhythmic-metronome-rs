//! Window modes on the one app window: full, pinned mini (small, always on
//! top, transparent so the mini card's rounded corners show) and background
//! (window hidden, sound playing).
//!
//! Each mode remembers its own rect. Switching resizes and moves the window
//! to the other mode's rect and changes its level; closing hides it.

use std::time::{Duration, Instant};

use eframe::egui::{Pos2, Vec2, ViewportBuilder, ViewportCommand, ViewportInfo, WindowLevel};
use pm_ui::state::WindowMode;
use pm_ui::views::mini;

/// Smallest full window (pt).
pub const FULL_MIN_SIZE: Vec2 = Vec2::new(420.0, 640.0);
/// Full window size when none was saved (pt): the two-column layout.
pub const FULL_DEFAULT_SIZE: Vec2 = Vec2::new(900.0, 700.0);
/// How long to wait for the window to take a new mode's size before
/// recording rects again.
const SETTLE: Duration = Duration::from_millis(500);

pub struct Windows {
    mode: WindowMode,
    /// `[x, y, w, h]`: outer position and inner size, per mode.
    pub full_rect: Option<[f32; 4]>,
    pub mini_rect: Option<[f32; 4]>,
    /// After a switch: the size asked for and until when to wait for it.
    settling: Option<(Vec2, Instant)>,
    /// Hidden with the close button.
    hidden: bool,
}

fn min_size(mode: WindowMode) -> Vec2 {
    match mode {
        WindowMode::Full => FULL_MIN_SIZE,
        WindowMode::Mini => mini::MIN_SIZE,
    }
}

fn default_size(mode: WindowMode) -> Vec2 {
    match mode {
        WindowMode::Full => FULL_DEFAULT_SIZE,
        WindowMode::Mini => mini::DEFAULT_SIZE,
    }
}

fn level(mode: WindowMode) -> WindowLevel {
    match mode {
        WindowMode::Full => WindowLevel::Normal,
        WindowMode::Mini => WindowLevel::AlwaysOnTop,
    }
}

impl Windows {
    pub fn new(
        mode: WindowMode,
        full_rect: Option<[f32; 4]>,
        mini_rect: Option<[f32; 4]>,
    ) -> Windows {
        Windows {
            mode,
            full_rect,
            mini_rect,
            settling: None,
            hidden: false,
        }
    }

    pub fn mode(&self) -> WindowMode {
        self.mode
    }

    fn rect(&self, mode: WindowMode) -> Option<[f32; 4]> {
        match mode {
            WindowMode::Full => self.full_rect,
            WindowMode::Mini => self.mini_rect,
        }
    }

    /// Size (at least the mode's minimum) and position to open `mode` at.
    fn placement(&self, mode: WindowMode) -> (Vec2, Option<Pos2>) {
        let min = min_size(mode);
        match self.rect(mode) {
            Some([x, y, w, h]) => (Vec2::new(w, h).max(min), Some(Pos2::new(x, y))),
            None => (default_size(mode).max(min), None),
        }
    }

    /// The window the app opens with, in the saved mode at its saved rect.
    pub fn viewport(&self) -> ViewportBuilder {
        let (size, pos) = self.placement(self.mode);
        let builder = ViewportBuilder::default()
            .with_title("Polyrhythmic Metronome")
            .with_app_id("polyrhythmic-metronome")
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_transparent(true)
            .with_min_inner_size(min_size(self.mode))
            .with_inner_size(size)
            .with_window_level(level(self.mode))
            .with_maximize_button(self.mode == WindowMode::Full);
        match pos {
            Some(pos) => builder.with_position(pos),
            None => builder,
        }
    }

    /// Switches to `mode`: the commands that resize, move and re-level the
    /// window. The current rect must have been [`track`](Self::track)ed.
    pub fn switch(&mut self, mode: WindowMode, now: Instant) -> Vec<ViewportCommand> {
        if mode == self.mode {
            return Vec::new();
        }
        self.mode = mode;
        let (size, pos) = self.placement(mode);
        self.settling = Some((size, now + SETTLE));
        // The new size is at least the new minimum, so setting the minimum
        // first never fights the resize.
        let mut commands = vec![
            ViewportCommand::MinInnerSize(min_size(mode)),
            ViewportCommand::InnerSize(size),
        ];
        if let Some(pos) = pos {
            commands.push(ViewportCommand::OuterPosition(pos));
        }
        commands.push(ViewportCommand::WindowLevel(level(mode)));
        commands.push(ViewportCommand::EnableButtons {
            close: true,
            minimized: true,
            maximize: mode == WindowMode::Full,
        });
        commands
    }

    /// Records the window's rect for the current mode. Ignored while hidden
    /// (occluded), minimized, fullscreen or maximized, and right after a
    /// switch until the window has taken the new size (or [`SETTLE`] passed),
    /// so one mode's rect never lands in the other's.
    pub fn track(&mut self, info: &ViewportInfo, now: Instant) {
        let (Some(outer), Some(inner)) = (info.outer_rect, info.inner_rect) else {
            return;
        };
        if let Some((size, until)) = self.settling {
            let reached = (inner.size() - size).abs().max_elem() < 1.0;
            if reached || now >= until {
                self.settling = None;
            } else {
                return;
            }
        }
        let shown = info.visible() != Some(false)
            && info.fullscreen != Some(true)
            && info.maximized != Some(true);
        if !shown {
            return;
        }
        let rect = Some([outer.min.x, outer.min.y, inner.width(), inner.height()]);
        match self.mode {
            WindowMode::Full => self.full_rect = rect,
            WindowMode::Mini => self.mini_rect = rect,
        }
    }

    /// The close button: keep running in the background with the window
    /// hidden. (Quitting is ⌘Q, or the menu bar's "Sair".)
    pub fn hide(&mut self) -> Vec<ViewportCommand> {
        self.hidden = true;
        vec![
            ViewportCommand::CancelClose,
            ViewportCommand::Visible(false),
        ]
    }

    /// Whether the window is hidden (closed to the background).
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// Brings the window back, from hidden or minimized; with `focus` the
    /// app is also activated and the window made key.
    pub fn show(&mut self, focus: bool) -> Vec<ViewportCommand> {
        self.hidden = false;
        let mut commands = vec![
            ViewportCommand::Visible(true),
            ViewportCommand::Minimized(false),
        ];
        if focus {
            commands.push(ViewportCommand::Focus);
        }
        commands
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::Rect;

    use super::*;

    fn info(outer_min: (f32, f32), size: (f32, f32)) -> ViewportInfo {
        let outer_min = Pos2::new(outer_min.0, outer_min.1);
        let size = Vec2::new(size.0, size.1);
        ViewportInfo {
            outer_rect: Some(Rect::from_min_size(outer_min, size + Vec2::new(0.0, 28.0))),
            inner_rect: Some(Rect::from_min_size(outer_min + Vec2::new(0.0, 28.0), size)),
            minimized: Some(false),
            occluded: Some(false),
            ..ViewportInfo::default()
        }
    }

    #[test]
    fn mini_is_on_top_at_its_own_rect_and_full_comes_back() {
        let now = Instant::now();
        let mut w = Windows::new(WindowMode::Full, None, Some([1200.0, 40.0, 240.0, 280.0]));
        w.track(&info((10.0, 20.0), (900.0, 700.0)), now);
        assert_eq!(w.full_rect, Some([10.0, 20.0, 900.0, 700.0]));

        let to_mini = w.switch(WindowMode::Mini, now);
        assert_eq!(w.mode(), WindowMode::Mini);
        assert_eq!(
            to_mini,
            [
                ViewportCommand::MinInnerSize(mini::MIN_SIZE),
                ViewportCommand::InnerSize(Vec2::new(240.0, 280.0)),
                ViewportCommand::OuterPosition(Pos2::new(1200.0, 40.0)),
                ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop),
                ViewportCommand::EnableButtons {
                    close: true,
                    minimized: true,
                    maximize: false
                },
            ]
        );
        // The window still has the full size for a frame or two: not the mini rect.
        w.track(&info((10.0, 20.0), (900.0, 700.0)), now);
        assert_eq!(w.mini_rect, Some([1200.0, 40.0, 240.0, 280.0]));
        assert_eq!(w.full_rect, Some([10.0, 20.0, 900.0, 700.0]));
        // Once there, moving the mini window is remembered as the mini rect.
        w.track(&info((1200.0, 40.0), (240.0, 280.0)), now);
        w.track(&info((1100.0, 60.0), (240.0, 280.0)), now);
        assert_eq!(w.mini_rect, Some([1100.0, 60.0, 240.0, 280.0]));

        let to_full = w.switch(WindowMode::Full, now);
        assert_eq!(
            to_full,
            [
                ViewportCommand::MinInnerSize(FULL_MIN_SIZE),
                ViewportCommand::InnerSize(Vec2::new(900.0, 700.0)),
                ViewportCommand::OuterPosition(Pos2::new(10.0, 20.0)),
                ViewportCommand::WindowLevel(WindowLevel::Normal),
                ViewportCommand::EnableButtons {
                    close: true,
                    minimized: true,
                    maximize: true
                },
            ]
        );
        assert!(w.switch(WindowMode::Full, now).is_empty(), "already full");
    }

    #[test]
    fn first_switch_uses_default_size_and_keeps_position() {
        let now = Instant::now();
        let mut w = Windows::new(WindowMode::Full, None, None);
        let c = w.switch(WindowMode::Mini, now);
        assert!(c.contains(&ViewportCommand::InnerSize(mini::DEFAULT_SIZE)));
        assert!(
            !c.iter()
                .any(|c| matches!(c, ViewportCommand::OuterPosition(_)))
        );
        // Gives up waiting for the size after a while and records again.
        w.track(&info((0.0, 0.0), (300.0, 320.0)), now + SETTLE);
        assert_eq!(w.mini_rect, Some([0.0, 0.0, 300.0, 320.0]));
    }

    #[test]
    fn sizes_never_go_below_the_mode_minimum() {
        let now = Instant::now();
        let mut w = Windows::new(
            WindowMode::Mini,
            Some([0.0, 0.0, 100.0, 100.0]),
            Some([0.0, 0.0, 50.0, 50.0]),
        );
        let b = w.viewport();
        assert_eq!(b.min_inner_size, Some(mini::MIN_SIZE));
        assert_eq!(b.inner_size, Some(mini::MIN_SIZE));
        assert_eq!(b.window_level, Some(WindowLevel::AlwaysOnTop));
        let c = w.switch(WindowMode::Full, now);
        assert!(c.contains(&ViewportCommand::MinInnerSize(FULL_MIN_SIZE)));
        assert!(c.contains(&ViewportCommand::InnerSize(FULL_MIN_SIZE)));
    }

    #[test]
    fn full_window_is_frameless_transparent_and_bounded() {
        let w = Windows::new(WindowMode::Full, Some([5.0, 6.0, 1000.0, 800.0]), None);
        let b = w.viewport();
        assert_eq!(b.fullsize_content_view, Some(true));
        assert_eq!(b.titlebar_shown, Some(false));
        assert_eq!(b.title_shown, Some(false));
        assert_eq!(b.transparent, Some(true));
        assert_eq!(b.min_inner_size, Some(FULL_MIN_SIZE));
        assert_eq!(b.inner_size, Some(Vec2::new(1000.0, 800.0)));
        assert_eq!(b.position, Some(Pos2::new(5.0, 6.0)));
        assert_eq!(b.window_level, Some(WindowLevel::Normal));
    }

    #[test]
    fn hidden_or_minimized_window_is_not_recorded() {
        let now = Instant::now();
        let mut w = Windows::new(WindowMode::Full, None, None);
        let mut i = info((10.0, 10.0), (500.0, 700.0));
        i.occluded = Some(true);
        w.track(&i, now);
        i.occluded = Some(false);
        i.minimized = Some(true);
        w.track(&i, now);
        assert_eq!(w.full_rect, None);
        i.minimized = Some(false);
        w.track(&i, now);
        assert_eq!(w.full_rect, Some([10.0, 10.0, 500.0, 700.0]));
    }

    #[test]
    fn close_hides_instead_of_quitting() {
        let mut w = Windows::new(WindowMode::Full, None, None);
        assert!(!w.hidden());
        assert_eq!(
            w.hide(),
            [
                ViewportCommand::CancelClose,
                ViewportCommand::Visible(false)
            ]
        );
        assert!(w.hidden());
    }

    #[test]
    fn show_brings_a_hidden_or_minimized_window_back() {
        let mut w = Windows::new(WindowMode::Full, None, None);
        w.hide();
        assert_eq!(
            w.show(true),
            [
                ViewportCommand::Visible(true),
                ViewportCommand::Minimized(false),
                ViewportCommand::Focus
            ]
        );
        assert!(!w.hidden());
        // Mini floats on top without taking the focus from the app in use.
        assert!(!w.show(false).contains(&ViewportCommand::Focus));
    }
}
