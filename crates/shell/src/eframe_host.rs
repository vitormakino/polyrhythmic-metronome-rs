//! Runs an [`AppShell`] as an [`eframe::App`].

use crate::AppShell;

/// The eframe adapter: every eframe callback goes to the shell. eframe has
/// no "left the screen" callback, so [`AppShell::on_background`] is never
/// called here (a hidden Mac window keeps running `logic`).
pub struct EframeHost<S: AppShell>(pub S);

impl<S: AppShell> eframe::App for EframeHost<S> {
    /// Runs before every frame and, while the window is hidden, on every
    /// requested repaint.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.logic(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }

    fn on_exit(&mut self) {
        self.0.on_exit();
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.0.clear_color(visuals)
    }
}
