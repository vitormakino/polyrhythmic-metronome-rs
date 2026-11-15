//! The [`Host`]: the egui context, the app and the GPU device, which live
//! as long as the process, and the [`Screen`] (the view, its wgpu surface and
//! display link), which lives as long as the scene. Two clocks drive the
//! app: the display link on screen, a timer off screen (see
//! [`Lifecycle`](crate::lifecycle::Lifecycle)).

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::Instant;

use egui::{Pos2, Rect, ViewportId, vec2};
use egui_wgpu::wgpu;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes, NSTimer};
use objc2_quartz_core::CADisplayLink;
use pm_shell::{AppShell, TICK};

use crate::Factory;
use crate::input::safe_area;
use crate::lifecycle::{Change, Lifecycle};
use crate::view::{MetalView, take_events};

thread_local! {
    /// Built when the first scene connects, kept until the process ends.
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Runs `f` on the host, if it is running.
pub(crate) fn with_host(f: impl FnOnce(&mut Host)) {
    HOST.with(|h| {
        if let Some(host) = h.borrow_mut().as_mut() {
            f(host);
        }
    });
}

/// Whether the app has been built (by an earlier scene).
pub(crate) fn started() -> bool {
    HOST.with(|h| h.borrow().is_some())
}

/// Builds the app (no view yet).
pub(crate) fn start(mtm: MainThreadMarker, make: Factory) {
    let host = Host::new(mtm, make);
    HOST.with(|h| *h.borrow_mut() = Some(host));
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PMTicker"]
    /// The background timer's target: it outlives any view.
    struct Ticker;

    unsafe impl NSObjectProtocol for Ticker {}

    impl Ticker {
        /// `NSTimer` callback: app logic without a frame.
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            with_host(|host| host.shell.logic(&host.ctx));
        }
    }
);

impl Ticker {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

/// The GPU state that does not depend on the view: built with the first
/// surface (the adapter must be able to present to it), then reused for
/// every later scene's surface. Keeping the renderer keeps the textures egui
/// uploaded once, such as the font atlas.
struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    renderer: egui_wgpu::Renderer,
}

/// What goes with the scene. The surface is declared before the view, so it
/// is dropped first.
struct Screen {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// Retains the view (its target) until invalidated.
    link: Retained<CADisplayLink>,
    view: Retained<MetalView>,
}

pub(crate) struct Host {
    ctx: egui::Context,
    shell: Box<dyn AppShell>,
    start: Instant,
    lifecycle: Lifecycle,
    gpu: Option<Gpu>,
    screen: Option<Screen>,
    ticker: Retained<Ticker>,
    /// Wakes `logic` while off screen.
    timer: Option<Retained<NSTimer>>,
}

impl Host {
    fn new(mtm: MainThreadMarker, make: Factory) -> Self {
        let ctx = egui::Context::default();
        let shell = make(&ctx);
        Self {
            ctx,
            shell,
            start: Instant::now(),
            lifecycle: Lifecycle::default(),
            gpu: None,
            screen: None,
            ticker: Ticker::new(mtm),
            timer: None,
        }
    }

    /// A scene connected: draws into `view`, around the existing app.
    pub(crate) fn attach(&mut self, view: Retained<MetalView>) {
        self.detach_screen();
        let instance = match &self.gpu {
            Some(gpu) => gpu.instance.clone(),
            None => wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::METAL,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            }),
        };
        let layer = view.layer();
        // SAFETY: the layer is the view's `CAMetalLayer` (`layerClass`); the
        // screen keeps the view, and so the layer, alive as long as the
        // surface (dropped first).
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(
                Retained::as_ptr(&layer) as *mut c_void,
            ))
        }
        .expect("wgpu surface on CAMetalLayer");
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(instance, &surface));
        let (w, h) = view.pixel_size();
        let config = wgpu::SurfaceConfiguration {
            format: gpu.format,
            present_mode: wgpu::PresentMode::Fifo,
            ..surface
                .get_default_config(&gpu.adapter, w, h)
                .expect("surface config")
        };
        surface.configure(&gpu.device, &config);

        // SAFETY: the view implements `step:` taking the display link.
        let link = unsafe { CADisplayLink::displayLinkWithTarget_selector(&view, sel!(step:)) };
        // SAFETY: the main run loop, on the main thread.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        self.screen = Some(Screen {
            surface,
            config,
            link,
            view,
        });
        let change = self.lifecycle.attach();
        self.apply(change);
    }

    /// The scene disconnected: drops the view, its surface and display link.
    /// The app keeps running off screen (background audio).
    pub(crate) fn detach(&mut self) {
        self.detach_screen();
        let change = self.lifecycle.detach();
        self.apply(change);
    }

    pub(crate) fn enter_background(&mut self) {
        let change = self.lifecycle.enter_background();
        self.apply(change);
    }

    pub(crate) fn enter_foreground(&mut self) {
        let change = self.lifecycle.enter_foreground();
        self.apply(change);
    }

    /// The app is about to quit.
    pub(crate) fn exit(&mut self) {
        self.shell.on_exit();
    }

    fn detach_screen(&mut self) {
        if let Some(screen) = self.screen.take() {
            screen.link.invalidate();
        }
    }

    /// Off screen: no frames, `logic` every [`TICK`] (the audio keeps
    /// playing, settings keep saving). On screen: frames again.
    fn apply(&mut self, change: Change) {
        if change.hidden && self.timer.is_none() {
            // SAFETY: the ticker implements `tick:` taking the timer; the
            // timer retains it until invalidated.
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    TICK.as_secs_f64(),
                    &self.ticker,
                    sel!(tick:),
                    None,
                    true,
                )
            };
            self.timer = Some(timer);
        }
        if change.shown
            && let Some(timer) = self.timer.take()
        {
            timer.invalidate();
        }
        if let Some(screen) = &self.screen {
            screen.link.setPaused(!self.lifecycle.on_screen());
        }
        if change.hidden {
            self.shell.on_background();
        }
    }

    /// One frame: input, `logic`, `ui`, paint.
    pub(crate) fn frame(&mut self) {
        let (Some(screen), Some(gpu)) = (&mut self.screen, &mut self.gpu) else {
            return;
        };
        let view = &screen.view;
        // The scene may move to a screen of another scale.
        if let Some(ui_screen) = view.screen() {
            view.sync_scale(&ui_screen);
        }
        let (w, h) = view.pixel_size();
        if (w, h) != (screen.config.width, screen.config.height) {
            screen.config.width = w;
            screen.config.height = h;
            screen.surface.configure(&gpu.device, &screen.config);
        }
        let bounds = view.bounds().size;
        let insets = view.safeAreaInsets();
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                vec2(bounds.width as f32, bounds.height as f32),
            )),
            time: Some(self.start.elapsed().as_secs_f64()),
            events: take_events(),
            safe_area_insets: Some(safe_area(
                insets.top,
                insets.left,
                insets.bottom,
                insets.right,
            )),
            system_theme: view.system_theme(),
            focused: true,
            ..Default::default()
        };
        raw.viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(view.contentScaleFactor() as f32);

        self.shell.logic(&self.ctx);
        let shell = &mut self.shell;
        let mut out = self.ctx.run_ui(raw, |ui| {
            // The app draws inside the safe area; the clear colour fills the
            // whole screen behind it.
            let content = ui.ctx().content_rect();
            ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                shell.ui(ui);
            });
        });
        let [r, g, b, a] = self.shell.clear_color(&self.ctx.global_style().visuals);

        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                gpu.renderer
                    .update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        let ppp = out.pixels_per_point;
        let jobs = self.ctx.tessellate(std::mem::take(&mut out.shapes), ppp);
        let screen_desc = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [screen.config.width, screen.config.height],
            pixels_per_point: ppp,
        };
        // Suboptimal still presents, then reconfigures; Outdated and Lost
        // reconfigure and skip; Timeout, Occluded and Validation skip.
        let (frame, reconfigure) = match screen.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => (Some(t), false),
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => (Some(t), true),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                (None, true)
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => (None, false),
        };
        let Some(frame) = frame else {
            if reconfigure {
                screen.surface.configure(&gpu.device, &screen.config);
            }
            // The uploaded textures stay; the freed ones are freed.
            free_textures(&mut gpu.renderer, &mut out.textures_delta);
            return;
        };
        let target = frame.texture.create_view(&Default::default());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let mut buffers =
            gpu.renderer
                .update_buffers(&gpu.device, &gpu.queue, &mut encoder, &jobs, &screen_desc);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(r),
                            g: f64::from(g),
                            b: f64::from(b),
                            a: f64::from(a),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let mut pass = pass.forget_lifetime();
            gpu.renderer.render(&mut pass, &jobs, &screen_desc);
        }
        buffers.push(encoder.finish());
        gpu.queue.submit(buffers);
        gpu.queue.present(frame);
        if reconfigure {
            screen.surface.configure(&gpu.device, &screen.config);
        }
        free_textures(&mut gpu.renderer, &mut out.textures_delta);
    }
}

/// Frees what egui let go of and empties the delta (egui asserts that every
/// delta is consumed).
fn free_textures(renderer: &mut egui_wgpu::Renderer, delta: &mut egui::TexturesDelta) {
    for id in &delta.free {
        renderer.free_texture(id);
    }
    delta.clear();
}

impl Gpu {
    fn new(instance: wgpu::Instance, surface: &wgpu::Surface<'_>) -> Self {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(surface),
            ..Default::default()
        }))
        .expect("Metal adapter");
        // The simulator's GPU is below wgpu's default limits: ask for what it has.
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .expect("Metal device");
        let caps = surface.get_capabilities(&adapter);
        // egui-wgpu wants a non-sRGB target (it outputs gamma colours).
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let renderer = egui_wgpu::Renderer::new(&device, format, Default::default());
        Gpu {
            instance,
            adapter,
            device,
            queue,
            format,
            renderer,
        }
    }
}
