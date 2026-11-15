//! What UIKit hands the host, as egui input. Pure, so it is tested on the Mac.

use egui::epaint::MarginF32;
use egui::{Event, PointerButton, Pos2, SafeAreaInsets};

/// Where a touch is in its life (`UITouchPhase`, as the view's
/// `touches…:withEvent:` methods report it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Began,
    Moved,
    Ended,
    Cancelled,
}

/// One touch as egui pointer events: the pointer moves to it, a press on
/// `Began`, a release and the pointer leaving on `Ended`/`Cancelled` (a
/// finger has no hover, so nothing stays hovered after it lifts).
pub fn touch_events(pos: Pos2, phase: TouchPhase) -> Vec<Event> {
    let button = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let mut events = vec![Event::PointerMoved(pos)];
    match phase {
        TouchPhase::Began => events.push(button(true)),
        TouchPhase::Moved => {}
        TouchPhase::Ended | TouchPhase::Cancelled => {
            events.push(button(false));
            events.push(Event::PointerGone);
        }
    }
    events
}

/// The view's `safeAreaInsets`, in points.
pub fn safe_area(top: f64, left: f64, bottom: f64, right: f64) -> SafeAreaInsets {
    SafeAreaInsets(MarginF32 {
        top: top as f32,
        left: left as f32,
        bottom: bottom as f32,
        right: right as f32,
    })
}

/// The drawable size in pixels for a view of this many points: rounded, and
/// at least 1×1 (a surface cannot be configured to zero).
pub fn pixel_size(width_pt: f64, height_pt: f64, scale: f64) -> (u32, u32) {
    let px = |pt: f64| ((pt * scale).round() as u32).max(1);
    (px(width_pt), px(height_pt))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(p: Pos2) -> Event {
        Event::PointerMoved(p)
    }

    fn button(p: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn began_presses_at_position() {
        let p = Pos2::new(10.0, 20.0);
        assert_eq!(
            touch_events(p, TouchPhase::Began),
            vec![moved(p), button(p, true)]
        );
    }

    #[test]
    fn drag_sequence_reaches_egui_in_order() {
        let a = Pos2::new(10.0, 100.0);
        let b = Pos2::new(10.0, 140.0);
        let c = Pos2::new(10.0, 180.0);
        let events: Vec<Event> = [
            (a, TouchPhase::Began),
            (b, TouchPhase::Moved),
            (c, TouchPhase::Moved),
            (c, TouchPhase::Ended),
        ]
        .into_iter()
        .flat_map(|(p, phase)| touch_events(p, phase))
        .collect();
        assert_eq!(
            events,
            vec![
                moved(a),
                button(a, true),
                moved(b),
                moved(c),
                moved(c),
                button(c, false),
                Event::PointerGone,
            ]
        );
    }

    #[test]
    fn cancelled_releases_and_leaves() {
        let p = Pos2::new(5.0, 6.0);
        let events = touch_events(p, TouchPhase::Cancelled);
        assert_eq!(
            events[events.len() - 2..],
            [button(p, false), Event::PointerGone]
        );
    }

    #[test]
    fn safe_area_maps_each_side() {
        let SafeAreaInsets(m) = safe_area(62.0, 0.0, 34.0, 0.0);
        assert_eq!((m.top, m.left, m.bottom, m.right), (62.0, 0.0, 34.0, 0.0));
        let SafeAreaInsets(m) = safe_area(1.0, 2.0, 3.0, 4.0);
        assert_eq!((m.top, m.left, m.bottom, m.right), (1.0, 2.0, 3.0, 4.0));
    }

    #[test]
    fn pixel_size_rounds_and_never_zero() {
        assert_eq!(pixel_size(402.0, 874.0, 3.0), (1206, 2622));
        assert_eq!(pixel_size(0.0, 0.0, 3.0), (1, 1));
        assert_eq!(pixel_size(100.4, 100.6, 1.0), (100, 101));
    }
}
