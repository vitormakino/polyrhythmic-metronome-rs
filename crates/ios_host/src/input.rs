//! What UIKit hands the host, as egui input. Pure, so it is tested on the Mac.

use egui::epaint::MarginF32;
use egui::{Event, PointerButton, Pos2, SafeAreaInsets, TouchDeviceId, TouchId};

/// Where a touch is in its life (`UITouchPhase`, as the view's
/// `touches…:withEvent:` methods report it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Began,
    Moved,
    Ended,
    Cancelled,
}

/// One touch as egui events, the way egui-winit sends it: first the touch
/// itself (so egui knows there is a touch screen and scrolls lists by
/// dragging them), then the same touch as the pointer. The pointer moves to
/// it, a press on `Began`, a release and the pointer leaving on `Ended` (a
/// finger has no hover, so nothing stays hovered after it lifts).
/// `Cancelled` (the system took the touch) only makes the pointer leave: its
/// release must reach egui a frame later, which [`TouchQueue`] does.
pub fn touch_events(pos: Pos2, phase: TouchPhase) -> Vec<Event> {
    let button = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let touch = Event::Touch {
        // Multiple touch is off: one device, one finger.
        device_id: TouchDeviceId(0),
        id: TouchId(0),
        phase: match phase {
            TouchPhase::Began => egui::TouchPhase::Start,
            TouchPhase::Moved => egui::TouchPhase::Move,
            TouchPhase::Ended => egui::TouchPhase::End,
            TouchPhase::Cancelled => egui::TouchPhase::Cancel,
        },
        pos,
        force: None,
    };
    let mut events = vec![touch, Event::PointerMoved(pos)];
    match phase {
        TouchPhase::Began => events.push(button(true)),
        TouchPhase::Moved => {}
        TouchPhase::Ended => {
            events.push(button(false));
            events.push(Event::PointerGone);
        }
        TouchPhase::Cancelled => events.push(Event::PointerGone),
    }
    events
}

/// The touch events waiting for egui, frame by frame.
///
/// A cancelled touch must not click, nor leave a drag going. egui 0.37 has
/// no pointer cancel and makes any release a click while the press is short
/// and still, wherever the pointer is; but a frame that ends with the
/// pointer gone clears what the press would click or drag. So the queue
/// sends `PointerGone` in one frame and the release in the next.
#[derive(Debug, Default)]
pub struct TouchQueue {
    this_frame: Vec<Event>,
    next_frame: Vec<Event>,
}

impl TouchQueue {
    /// Queues one touch.
    pub fn push(&mut self, pos: Pos2, phase: TouchPhase) {
        match phase {
            // Its press supersedes a release still waiting for the next
            // frame (egui takes a press while down; this touch's release
            // ends both).
            TouchPhase::Began => self.next_frame.clear(),
            TouchPhase::Cancelled => self.next_frame.push(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }),
            TouchPhase::Moved | TouchPhase::Ended => {}
        }
        self.this_frame.extend(touch_events(pos, phase));
    }

    /// The events for the frame about to run.
    pub fn take_frame(&mut self) -> Vec<Event> {
        let next = std::mem::take(&mut self.next_frame);
        std::mem::replace(&mut self.this_frame, next)
    }
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

    fn touch(p: Pos2, phase: egui::TouchPhase) -> Event {
        Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(0),
            phase,
            pos: p,
            force: None,
        }
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
            vec![touch(p, egui::TouchPhase::Start), moved(p), button(p, true)]
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
                touch(a, egui::TouchPhase::Start),
                moved(a),
                button(a, true),
                touch(b, egui::TouchPhase::Move),
                moved(b),
                touch(c, egui::TouchPhase::Move),
                moved(c),
                touch(c, egui::TouchPhase::End),
                moved(c),
                button(c, false),
                Event::PointerGone,
            ]
        );
    }

    #[test]
    fn cancelled_leaves_without_releasing() {
        let p = Pos2::new(5.0, 6.0);
        assert_eq!(
            touch_events(p, TouchPhase::Cancelled),
            vec![
                touch(p, egui::TouchPhase::Cancel),
                moved(p),
                Event::PointerGone
            ]
        );
    }

    #[test]
    fn queue_releases_a_cancelled_touch_the_frame_after_it_leaves() {
        let p = Pos2::new(5.0, 6.0);
        let mut queue = TouchQueue::default();
        queue.push(p, TouchPhase::Began);
        assert_eq!(queue.take_frame(), touch_events(p, TouchPhase::Began));
        queue.push(p, TouchPhase::Cancelled);
        assert_eq!(queue.take_frame(), touch_events(p, TouchPhase::Cancelled));
        assert_eq!(queue.take_frame(), vec![button(p, false)]);
        assert_eq!(queue.take_frame(), vec![]);
    }

    #[test]
    fn queue_puts_a_new_touch_after_the_pending_release() {
        let a = Pos2::new(5.0, 6.0);
        let b = Pos2::new(50.0, 60.0);
        let mut queue = TouchQueue::default();
        queue.push(a, TouchPhase::Cancelled);
        let _ = queue.take_frame();
        queue.push(b, TouchPhase::Began);
        let mut expected = vec![button(a, false)];
        expected.extend(touch_events(b, TouchPhase::Began));
        assert_eq!(queue.take_frame(), expected);
    }

    #[test]
    fn queue_drops_the_pending_release_when_a_touch_begins_in_the_same_frame() {
        let a = Pos2::new(5.0, 6.0);
        let b = Pos2::new(50.0, 60.0);
        let mut queue = TouchQueue::default();
        queue.push(a, TouchPhase::Cancelled);
        queue.push(b, TouchPhase::Began);
        let mut expected = touch_events(a, TouchPhase::Cancelled);
        expected.extend(touch_events(b, TouchPhase::Began));
        assert_eq!(queue.take_frame(), expected);
        assert_eq!(queue.take_frame(), vec![]);
    }

    /// What egui made of a touch on a click-and-drag button: `steps` are
    /// offsets from the button's centre, one frame each through a
    /// [`TouchQueue`], then a few empty frames.
    struct Outcome {
        clicked: bool,
        down: bool,
        dragged: bool,
    }

    fn on_button(steps: &[(f32, TouchPhase)]) -> Outcome {
        // One frame: the button's centre and whether it was clicked.
        fn frame(ctx: &egui::Context, events: Vec<Event>, t: f64) -> (Pos2, bool) {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(400.0, 800.0),
                )),
                time: Some(t),
                events,
                ..Default::default()
            };
            let mut out = (Pos2::ZERO, false);
            let mut output = ctx.run_ui(raw, |ui| {
                let response =
                    ui.add(egui::Button::new("tap").sense(egui::Sense::click_and_drag()));
                out = (response.rect.center(), response.clicked());
            });
            output.textures_delta.clear(); // egui asserts it is consumed
            out
        }
        let ctx = egui::Context::default();
        let mut queue = TouchQueue::default();
        let (center, mut clicked) = frame(&ctx, Vec::new(), 0.0);
        let mut t = 0.0;
        for (dy, phase) in steps {
            t += 0.05;
            queue.push(center + egui::vec2(0.0, *dy), *phase);
            clicked |= frame(&ctx, queue.take_frame(), t).1;
        }
        for _ in 0..3 {
            t += 0.05;
            clicked |= frame(&ctx, queue.take_frame(), t).1;
        }
        Outcome {
            clicked,
            down: ctx.input(|i| i.pointer.any_down()),
            dragged: ctx.dragged_id().is_some(),
        }
    }

    #[test]
    fn ended_touch_clicks() {
        let out = on_button(&[(0.0, TouchPhase::Began), (0.0, TouchPhase::Ended)]);
        assert!(out.clicked && !out.down);
    }

    #[test]
    fn cancelled_touch_never_clicks_nor_stays_down() {
        let out = on_button(&[(0.0, TouchPhase::Began), (0.0, TouchPhase::Cancelled)]);
        assert!(!out.clicked && !out.down);
    }

    #[test]
    fn cancelled_drag_stops() {
        use TouchPhase::*;
        let out = on_button(&[(0.0, Began), (40.0, Moved), (40.0, Cancelled)]);
        assert!(!out.clicked && !out.down && !out.dragged);
    }

    #[test]
    fn a_tap_after_a_cancelled_touch_clicks() {
        use TouchPhase::*;
        let out = on_button(&[(0.0, Began), (0.0, Cancelled), (0.0, Began), (0.0, Ended)]);
        assert!(out.clicked && !out.down);
    }

    /// A finger dragged up a list of buttons, one frame per step through a
    /// [`TouchQueue`]: how far the list scrolled and whether a button clicked.
    fn drag_list(dy: f32) -> (f32, bool) {
        fn frame(ctx: &egui::Context, events: Vec<Event>, t: f64) -> (f32, bool) {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                time: Some(t),
                events,
                ..Default::default()
            };
            let mut out = (0.0, false);
            let mut output = ctx.run_ui(raw, |ui| {
                let scroll = egui::ScrollArea::vertical().show(ui, |ui| {
                    for i in 0..40 {
                        out.1 |= ui.button(format!("beat {i}")).clicked();
                    }
                });
                out.0 = scroll.state.clamped_offset().y;
            });
            output.textures_delta.clear(); // egui asserts it is consumed
            out
        }
        let ctx = egui::Context::default();
        let mut queue = TouchQueue::default();
        let start = Pos2::new(30.0, 200.0);
        let mut t = 0.0;
        // egui picks what a touch drags when it lands, from the last frame's
        // widgets, and the list only takes drags once egui has seen a touch:
        // the first touch since launch (here a tap on the blank side) cannot
        // scroll yet.
        let side = Pos2::new(390.0, 290.0);
        for phase in [TouchPhase::Began, TouchPhase::Ended] {
            queue.push(side, phase);
            frame(&ctx, queue.take_frame(), t);
            t += 1.0 / 60.0;
        }
        let mut clicked = frame(&ctx, Vec::new(), t).1;
        let steps = 10;
        for i in 0..=steps {
            let phase = match i {
                0 => TouchPhase::Began,
                i if i == steps => TouchPhase::Ended,
                _ => TouchPhase::Moved,
            };
            let p = start - egui::vec2(0.0, dy * i as f32 / steps as f32);
            queue.push(p, phase);
            t += 1.0 / 60.0;
            clicked |= frame(&ctx, queue.take_frame(), t).1;
        }
        t += 1.0 / 60.0;
        let (offset, c) = frame(&ctx, queue.take_frame(), t);
        (offset, clicked || c)
    }

    #[test]
    fn dragging_a_list_with_a_finger_scrolls_it_without_clicking() {
        let (offset, clicked) = drag_list(150.0);
        assert!(offset >= 140.0, "scrolled only {offset}");
        assert!(!clicked);
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
