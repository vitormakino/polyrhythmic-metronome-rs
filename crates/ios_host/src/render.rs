//! The [`Host`]: the egui context, the app, the wgpu surface on the view's
//! `CAMetalLayer`, and the two clocks that drive the app (the display link on
//! screen, a timer in the background).

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::Instant;

use egui::{Pos2, Rect, ViewportId, vec2};
use egui_wgpu::wgpu;
use objc2::rc::Retained;
use objc2::sel;
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes, NSTimer};
use objc2_quartz_core::CADisplayLink;
use pm_shell::{AppShell, TICK};

use crate::Factory;
use crate::input::safe_area;
use crate::view::{MetalView, take_events};

thread_local! {
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

/// Builds the app and the GPU state for `view` and starts the display link.
pub(crate) fn start(view: Retained<MetalView>, make: Factory) {
    let host = Host::new(view, make);
    HOST.with(|h| *h.borrow_mut() = Some(host));
}

/// The scene is gone: tells the app and tears the host down.
pub(crate) fn stop() {
    let host = HOST.with(|h| h.borrow_mut().take());
    if let Some(mut host) = host {
        host.shell.on_exit();
        host.link.invalidate();
        if let Some(timer) = host.timer.take() {
            timer.invalidate();
        }
    }
}

pub(crate) struct Host {
    view: Retained<MetalView>,
    ctx: egui::Context,
    shell: Box<dyn AppShell>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: egui_wgpu::Renderer,
    start: Instant,
    /// Retains the view (its target) until invalidated.
    link: Retained<CADisplayLink>,
    /// Wakes `logic` while in the background.
    timer: Option<Retained<NSTimer>>,
}

impl Host {
    fn new(view: Retained<MetalView>, make: Factory) -> Self {
        let ctx = egui::Context::default();
        let shell = make(&ctx);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let layer = view.layer();
        // SAFETY: the layer is the view's `CAMetalLayer` (`layerClass`); the
        // host keeps the view, and so the layer, alive as long as the surface.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(
                Retained::as_ptr(&layer) as *mut c_void,
            ))
        }
        .expect("wgpu surface on CAMetalLayer");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
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
        let (w, h) = view.pixel_size();
        let config = wgpu::SurfaceConfiguration {
            format,
            present_mode: wgpu::PresentMode::Fifo,
            ..surface
                .get_default_config(&adapter, w, h)
                .expect("surface config")
        };
        surface.configure(&device, &config);
        let renderer = egui_wgpu::Renderer::new(&device, format, Default::default());

        // SAFETY: the view implements `step:` taking the display link.
        let link = unsafe { CADisplayLink::displayLinkWithTarget_selector(&view, sel!(step:)) };
        // SAFETY: the main run loop, on the main thread.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };

        Self {
            view,
            ctx,
            shell,
            surface,
            device,
            queue,
            config,
            renderer,
            start: Instant::now(),
            link,
            timer: None,
        }
    }

    /// Off screen: no frames, `logic` every [`TICK`] (the audio keeps
    /// playing, settings keep saving).
    pub(crate) fn enter_background(&mut self) {
        self.link.setPaused(true);
        if self.timer.is_none() {
            // SAFETY: the view implements `tick:` taking the timer; the timer
            // retains it until invalidated.
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    TICK.as_secs_f64(),
                    &self.view,
                    sel!(tick:),
                    None,
                    true,
                )
            };
            self.timer = Some(timer);
        }
        self.shell.on_background();
    }

    /// Back on screen: frames again.
    pub(crate) fn enter_foreground(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.invalidate();
        }
        self.link.setPaused(false);
    }

    pub(crate) fn background_tick(&mut self) {
        self.shell.logic(&self.ctx);
    }

    /// One frame: input, `logic`, `ui`, paint.
    pub(crate) fn frame(&mut self) {
        // The scene may move to a screen of another scale.
        if let Some(screen) = self.view.screen() {
            self.view.sync_scale(&screen);
        }
        let (w, h) = self.view.pixel_size();
        if (w, h) != (self.config.width, self.config.height) {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
        }
        let bounds = self.view.bounds().size;
        let insets = self.view.safeAreaInsets();
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
            system_theme: self.view.system_theme(),
            focused: true,
            ..Default::default()
        };
        raw.viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(self.view.contentScaleFactor() as f32);

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
                self.renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        let ppp = out.pixels_per_point;
        let jobs = self.ctx.tessellate(std::mem::take(&mut out.shapes), ppp);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: ppp,
        };
        // Suboptimal still presents, then reconfigures; Outdated and Lost
        // reconfigure and skip; Timeout, Occluded and Validation skip.
        let (frame, reconfigure) = match self.surface.get_current_texture() {
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
                self.surface.configure(&self.device, &self.config);
            }
            // The uploaded textures stay; the freed ones are freed.
            self.free_textures(&mut out.textures_delta);
            return;
        };
        let target = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mut buffers =
            self.renderer
                .update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);
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
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        buffers.push(encoder.finish());
        self.queue.submit(buffers);
        self.queue.present(frame);
        if reconfigure {
            self.surface.configure(&self.device, &self.config);
        }
        self.free_textures(&mut out.textures_delta);
    }

    /// Frees what egui let go of and empties the delta (egui asserts that
    /// every delta is consumed).
    fn free_textures(&mut self, delta: &mut egui::TexturesDelta) {
        for id in &delta.free {
            self.renderer.free_texture(id);
        }
        delta.clear();
    }
}
