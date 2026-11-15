//! The Metal-backed view: its layer is the wgpu surface, its touches become
//! egui events, and it is the target of the display link and of the
//! background timer.

use std::cell::RefCell;

use egui::{Pos2, Theme};
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSObject, NSSet, NSTimer};
use objc2_quartz_core::{CADisplayLink, CAMetalLayer};
use objc2_ui_kit::{
    UIEvent, UIResponder, UITouch, UITraitEnvironment, UIUserInterfaceStyle, UIView,
};

use crate::input::{TouchPhase, touch_events};
use crate::render;

thread_local! {
    /// Touch events since the last frame.
    static EVENTS: RefCell<Vec<egui::Event>> = const { RefCell::new(Vec::new()) };
}

/// The events gathered since the last call.
pub(crate) fn take_events() -> Vec<egui::Event> {
    EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()))
}

define_class!(
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PMMetalView"]
    pub(crate) struct MetalView;

    impl MetalView {
        #[unsafe(method(layerClass))]
        fn layer_class() -> &'static AnyClass {
            CAMetalLayer::class()
        }

        #[unsafe(method(touchesBegan:withEvent:))]
        fn touches_began(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.touch(touches, TouchPhase::Began);
        }

        #[unsafe(method(touchesMoved:withEvent:))]
        fn touches_moved(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.touch(touches, TouchPhase::Moved);
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn touches_ended(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.touch(touches, TouchPhase::Ended);
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn touches_cancelled(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.touch(touches, TouchPhase::Cancelled);
        }

        /// `CADisplayLink` callback: one frame.
        #[unsafe(method(step:))]
        fn step(&self, _link: &CADisplayLink) {
            render::with_host(|host| host.frame());
        }

        /// Background `NSTimer` callback: app logic without a frame.
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            render::with_host(|host| host.background_tick());
        }
    }
);

impl MetalView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }

    /// The view's drawable size in pixels.
    pub(crate) fn pixel_size(&self) -> (u32, u32) {
        let size = self.bounds().size;
        crate::input::pixel_size(size.width, size.height, self.contentScaleFactor())
    }

    /// Light or dark, from the view's trait collection.
    pub(crate) fn system_theme(&self) -> Option<Theme> {
        let style = unsafe { self.traitCollection().userInterfaceStyle() };
        match style {
            UIUserInterfaceStyle::Dark => Some(Theme::Dark),
            UIUserInterfaceStyle::Light => Some(Theme::Light),
            _ => None,
        }
    }

    fn touch(&self, touches: &NSSet<UITouch>, phase: TouchPhase) {
        // Multiple touch is off: the set holds the one touch.
        let Some(touch) = touches.anyObject() else {
            return;
        };
        let p = touch.locationInView(Some(self));
        let events = touch_events(Pos2::new(p.x as f32, p.y as f32), phase);
        EVENTS.with(|e| e.borrow_mut().extend(events));
    }
}
