//! Native macOS polyrhythmic metronome: the platform-free app
//! ([`pm_shell::Shell`]) plus what only the Mac has (menu bar item, global
//! hotkeys, full and mini windows, config folder watching), run by eframe
//! through [`EframeHost`].
//!
//! Threads: the UI (main) thread owns the [`AudioService`], the views and
//! the settings. A "ui-tick" thread asks for a repaint every 250 ms, so
//! [`App::logic`](AppShell::logic) keeps running while the window is
//! hidden (eframe then runs only `logic`, no egui pass): the service is
//! ticked (device following, garbage), beat events are drained and the
//! settings saved even in the background.
//!
//! The menu bar item and the global hotkeys are set up on the first `logic`
//! call (macOS wants them on the main thread with the event loop running);
//! their events come through a channel as [`Remote`]s, drained in `logic`,
//! so they work while the window is hidden.

mod config;
mod config_watch;
mod hotkeys;
mod remote;
mod sessions;
mod tray;
mod windows;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use eframe::egui::{self, Sense, ViewportCommand};
use pm_audio::AudioService;

// Debug builds abort on an allocation inside the audio callback (the
// `assert_no_alloc` guard in `pm_audio`'s data callback); release builds use
// the system allocator unchanged.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
use pm_shell::{AppShell, EframeHost, Settings, Shell, TICK, WindowSettings};
use pm_ui::state::{UiAction, UiState, WindowMode};
use pm_ui::theme;
use pm_ui::views::{full::full_view, mini::mini_view};

use remote::Remote;
use tray::Tray;
use windows::Windows;

struct App {
    shell: Shell,
    windows: Windows,
    /// Menu bar item and hotkeys: built on the first `logic` call, kept
    /// alive here (dropping them removes them).
    started: bool,
    tray: Option<Tray>,
    _hotkeys: Option<global_hotkey::GlobalHotKeyManager>,
    remote_tx: Sender<Remote>,
    remote_rx: Receiver<Remote>,
    /// New messages and theme from the config watcher.
    config_rx: Receiver<config_watch::Appearance>,
    /// "Quit" was chosen: let the next close request through.
    quitting: bool,
}

impl App {
    fn new(
        audio: AudioService,
        settings: &Settings,
        settings_dir: Option<PathBuf>,
        appearance: config_watch::Appearance,
        config_rx: Receiver<config_watch::Appearance>,
    ) -> App {
        let (remote_tx, remote_rx) = mpsc::channel();
        let mut ui = UiState::default();
        ui.messages = appearance.messages;
        ui.theme = appearance.theme;
        let mut shell = Shell::new(audio, ui, settings, settings_dir);
        shell.ctl.set_sessions(appearance.sessions);
        App {
            shell,
            windows: Windows::new(settings.mode, settings.full_rect, settings.mini_rect),
            started: false,
            tray: None,
            _hotkeys: None,
            remote_tx,
            remote_rx,
            config_rx,
            quitting: false,
        }
    }

    /// The windows as the settings keep them.
    fn window_settings(&self) -> WindowSettings {
        WindowSettings {
            mode: self.windows.mode(),
            full_rect: self.windows.full_rect,
            mini_rect: self.windows.mini_rect,
        }
    }

    /// Writes the settings now, windows included.
    fn save(&mut self) {
        self.shell.set_window(self.window_settings());
        self.shell.save();
    }

    /// Builds the menu bar item and registers the hotkeys. Either may fail
    /// (reported on stderr); the app runs on without it.
    fn start_remotes(&mut self, ctx: &egui::Context) {
        tray::forward(self.remote_tx.clone(), ctx.clone());
        hotkeys::forward(self.remote_tx.clone(), ctx.clone());
        let ui = &self.shell.ctl.ui;
        match Tray::build(ui.playing, &ui.messages) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => eprintln!("polyrhythmic-metronome: no menu bar item: {e}"),
        }
        match hotkeys::register_hotkeys() {
            Ok(manager) => self._hotkeys = Some(manager),
            Err(e) => eprintln!("polyrhythmic-metronome: no global hotkeys: {e}"),
        }
    }

    /// A request from the menu bar item or a hotkey: window ones here, the
    /// others through the views' actions.
    fn remote(&mut self, ctx: &egui::Context, remote: Remote, now: Instant) {
        if remote == Remote::Quit {
            self.save();
            self.quitting = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
            ctx.request_repaint();
        } else if let Some(mode) = remote::target_mode(remote, self.windows.mode()) {
            self.show(ctx, mode, now);
        } else {
            for action in remote::view_actions(&mut self.shell.ctl.ui, remote) {
                self.handle(ctx, action, now);
            }
        }
    }

    /// Shows the window in `mode`. The full window also takes focus; the
    /// mini one floats on top without taking it from the app in use.
    fn show(&mut self, ctx: &egui::Context, mode: WindowMode, now: Instant) {
        for command in self.windows.show(mode == WindowMode::Full) {
            ctx.send_viewport_cmd(command);
        }
        if mode != self.shell.ctl.ui.mode {
            self.shell.ctl.ui.mode = mode;
            self.handle(ctx, UiAction::ToggleMini, now);
        }
        ctx.request_repaint();
    }

    /// A view's action: the shell's, else the windows' (`ToggleMini`).
    fn handle(&mut self, ctx: &egui::Context, action: UiAction, now: Instant) {
        if let Some(action) = self.shell.apply(action) {
            self.window_action(ctx, action, now);
        }
    }

    fn window_action(&mut self, ctx: &egui::Context, action: UiAction, now: Instant) {
        if action == UiAction::ToggleMini {
            // The view already flipped `ui.mode`.
            for command in self.windows.switch(self.shell.ctl.ui.mode, now) {
                ctx.send_viewport_cmd(command);
            }
        }
    }
}

impl AppShell for App {
    /// Runs before every frame and, while the window is hidden, on every
    /// wake-up of the tick thread.
    fn logic(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if !self.started {
            self.started = true;
            self.start_remotes(ctx);
        }
        // The windows as the last frame left them, for the settings saved
        // once they settle.
        self.shell.set_window(self.window_settings());
        self.shell.logic(now, !self.windows.hidden());
        let remotes: Vec<Remote> = self.remote_rx.try_iter().collect();
        for remote in remotes {
            self.remote(ctx, remote, now);
        }
        // Only the latest language, theme and sessions matter; no audio
        // command is involved (a session plan goes with the next Play).
        if let Some(appearance) = self.config_rx.try_iter().last() {
            let ctl = &mut self.shell.ctl;
            ctl.set_sessions(appearance.sessions);
            ctl.ui.messages = appearance.messages;
            theme::apply(ctx, &appearance.theme);
            ctl.ui.theme = appearance.theme;
            if let Some(tray) = &mut self.tray {
                tray.set_messages(&ctl.ui.messages);
            }
        }
        if let Some(tray) = &mut self.tray {
            let ui = &self.shell.ctl.ui;
            tray.set_playing(ui.playing, &ui.messages);
        }

        if !self.quitting && ctx.input(|i| i.viewport().close_requested()) {
            for command in self.windows.hide() {
                ctx.send_viewport_cmd(command);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let now = Instant::now();
        let mode = self.windows.mode();
        let actions = self.shell.view(ui, now, |ui, state, now| {
            // The window has no title bar: drag it by any empty spot.
            // Widgets drawn after this sit on top of it and keep their own
            // drags.
            let window = ui.max_rect();
            let background =
                ui.interact(window, ui.make_persistent_id("window-drag"), Sense::drag());
            if background.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            match mode {
                WindowMode::Full => full_view(ui, state, now),
                WindowMode::Mini => mini_view(ui, state, now),
            }
        });
        let ctx = ui.ctx().clone();
        for action in actions {
            self.window_action(&ctx, action, now);
        }
        let info = ctx.input(|i| i.viewport().clone());
        self.windows.track(&info, now);
    }

    /// ⌘Q (or "Sair"): write the settings now.
    fn on_exit(&mut self) {
        self.save();
    }

    /// Transparent, so the mini window's rounded card shows its corners.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }
}

/// Wakes the app every [`TICK`] even while its window is hidden.
fn spawn_ticker(ctx: egui::Context) {
    let spawned = std::thread::Builder::new()
        .name("ui-tick".to_owned())
        .spawn(move || {
            loop {
                std::thread::sleep(TICK);
                ctx.request_repaint();
            }
        });
    if let Err(e) = spawned {
        eprintln!("polyrhythmic-metronome: no tick thread, a hidden window stops ticking: {e}");
    }
}

/// Creates the config folder's files (and the example sessions, when
/// `create_examples`) and loads the language, theme and sessions, before the
/// window opens. Any problem goes to stderr and the app runs in English.
/// The last value tells whether the example sessions were written.
fn load_config(create_examples: bool) -> (config_watch::Loader, Option<PathBuf>, bool) {
    let mut loader = config_watch::Loader::default();
    let Some(dir) = config::config_dir() else {
        eprintln!("polyrhythmic-metronome: no home folder, using the English interface");
        return (loader, None, false);
    };
    if let Err(e) = config::ensure_files(&dir) {
        eprintln!("polyrhythmic-metronome: {}: {e}", dir.display());
    }
    let mut examples_created = false;
    if create_examples {
        match config::create_example_sessions(&dir) {
            Ok(()) => examples_created = true,
            Err(e) => eprintln!("polyrhythmic-metronome: example sessions: {e}"),
        }
    }
    let reload = loader.reload(&config_watch::read_file, &config_watch::list_toml, &dir);
    for line in &reload.diagnostics {
        eprintln!("polyrhythmic-metronome: {line}");
    }
    (loader, Some(dir), examples_created)
}

fn main() -> eframe::Result {
    let (settings_dir, mut settings) = pm_shell::load_settings();
    let (loader, config_dir, examples_created) = load_config(!settings.examples_created);
    if examples_created {
        // Saved now, so a deleted example is not written again.
        settings.examples_created = true;
        if let Some(dir) = &settings_dir
            && let Err(e) = settings.save(dir)
        {
            eprintln!("polyrhythmic-metronome: could not save settings: {e}");
        }
    }
    let audio = pm_shell::start_audio(&settings);
    let viewport = Windows::new(settings.mode, settings.full_rect, settings.mini_rect).viewport();
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Polyrhythmic Metronome",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx, &loader.theme);
            spawn_ticker(cc.egui_ctx.clone());
            let (config_tx, config_rx) = mpsc::channel();
            let appearance = config_watch::Appearance {
                messages: loader.messages.clone(),
                theme: loader.theme.clone(),
                sessions: loader.sessions.clone(),
            };
            if let Some(dir) = config_dir {
                config_watch::spawn(dir, loader, config_tx, cc.egui_ctx.clone());
            }
            Ok(Box::new(EframeHost(App::new(
                audio,
                &settings,
                settings_dir,
                appearance,
                config_rx,
            ))))
        }),
    )
}
