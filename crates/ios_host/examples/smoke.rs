//! Smoke test for the bridge on a simulator or device: a label, a counter
//! button and how often `logic` ran. Build with
//! `cargo build -p polyrhythmic-metronome-ios-host --example smoke --target aarch64-apple-ios-sim`
//! and bundle it with an Info.plist naming `pm_ios_host::SCENE_DELEGATE_CLASS`.

#[cfg(target_os = "ios")]
fn main() {
    use pm_shell::AppShell;

    #[derive(Default)]
    struct Smoke {
        logic_calls: u64,
        taps: u32,
    }

    impl AppShell for Smoke {
        fn logic(&mut self, _ctx: &egui::Context) {
            self.logic_calls += 1;
        }

        fn ui(&mut self, ui: &mut egui::Ui) {
            ui.heading("pm_ios_host smoke");
            ui.label(format!("logic calls: {}", self.logic_calls));
            if ui.button(format!("taps: {}", self.taps)).clicked() {
                self.taps += 1;
            }
        }

        fn on_background(&mut self) {
            eprintln!("smoke: on_background");
        }

        fn on_exit(&mut self) {
            eprintln!("smoke: on_exit");
        }
    }

    pm_ios_host::run(Box::new(|ctx| {
        // egui has no font by default (the app brings its own): borrow one.
        let mut fonts = egui::FontDefinitions::empty();
        fonts.font_data.insert(
            "Inter".into(),
            std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
                "../../ui/assets/fonts/Inter-Regular.ttf"
            ))),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.insert(family, vec!["Inter".into()]);
        }
        ctx.set_fonts(fonts);
        Box::new(Smoke::default())
    }));
}

#[cfg(not(target_os = "ios"))]
fn main() {
    eprintln!("the smoke example only runs on iOS");
}
