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
    UIEvent, UIResponder, UIScreen, UITouch, UITraitEnvironment, UIUserInterfaceStyle, UIView,
};

use crate::input::{TouchPhase, TouchQueue};
use crate::render;

thread_local! {
    /// Touch events waiting for egui.
    static TOUCHES: RefCell<TouchQueue> = RefCell::new(TouchQueue::default());
}

/// The touch events for the frame about to run.
pub(crate) fn take_events() -> Vec<egui::Event> {
    TOUCHES.with(|t| t.borrow_mut().take_frame())
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

    /// Matches the drawable's scale to the screen's native pixels. A view
    /// backed by a `CAMetalLayer` keeps a scale of 1 unless told, which
    /// renders at a third of the resolution on a 3× screen.
    pub(crate) fn sync_scale(&self, screen: &UIScreen) {
        let scale = screen.nativeScale();
        if self.contentScaleFactor() != scale {
            self.setContentScaleFactor(scale);
        }
    }

    /// The screen of the view's window scene, once it is in a window.
    pub(crate) fn screen(&self) -> Option<Retained<UIScreen>> {
        Some(self.window()?.windowScene()?.screen())
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
        let pos = Pos2::new(p.x as f32, p.y as f32);
        TOUCHES.with(|t| t.borrow_mut().push(pos, phase));
    }
}
