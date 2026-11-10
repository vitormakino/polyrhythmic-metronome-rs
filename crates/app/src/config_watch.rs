//! Loads the language and theme from `config.toml`, `locales/` and `themes/`
//! and reloads them when those files change. Reading and parsing happen here, on the `config-watch`
//! thread (and once at startup, before the window opens), never in a frame or
//! on the audio thread.
//!
//! A broken file never takes the app down: the last valid state is kept and
//! the problem goes to stderr.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use eframe::egui;
use notify::RecursiveMode;
use notify_debouncer_mini::new_debouncer;
use pm_ui::i18n::{Messages, resolve};
use pm_ui::palette::{self, Palette, RESERVED_THEMES};
use pm_ui::theme::{DARK, Theme};

use crate::config::{self, Config};

/// Quiet period after the last file event before reloading (an editor save
/// is often several events).
const DEBOUNCE: Duration = Duration::from_millis(200);

/// The last valid configuration and what was built from it.
pub struct Loader {
    config: Config,
    /// Language the current `messages` were built for.
    shown: String,
    /// Theme name the current `theme` was built for.
    shown_theme: String,
    /// Diagnostics of the previous reload, to report only what changes.
    reported: Vec<String>,
    pub messages: Arc<Messages>,
    pub theme: Arc<Theme>,
}

impl Default for Loader {
    fn default() -> Self {
        let config = Config::default();
        Loader {
            shown: config.language.clone(),
            shown_theme: config.theme.clone(),
            reported: Vec::new(),
            config,
            messages: Arc::new(Messages::english()),
            theme: Arc::new(DARK),
        }
    }
}

/// The texts and colors the interface is showing.
#[derive(Clone)]
pub struct Appearance {
    pub messages: Arc<Messages>,
    pub theme: Arc<Theme>,
}

/// What a reload produced.
pub struct Reload {
    /// The current messages and theme; `Some` only when either changed.
    pub appearance: Option<Appearance>,
    /// One line per problem, each naming the file. Empty when the problems
    /// are the same as in the previous reload.
    pub diagnostics: Vec<String>,
}

impl Loader {
    /// Re-reads `config.toml`, the language file and the theme file in `dir`
    /// through `read`.
    pub fn reload(&mut self, read: &dyn Fn(&Path) -> io::Result<String>, dir: &Path) -> Reload {
        let mut diagnostics = Vec::new();
        let config_path = dir.join("config.toml");
        match read(&config_path) {
            Ok(text) => match config::parse(&text) {
                Ok((config, warnings)) => {
                    diagnostics.extend(
                        warnings
                            .into_iter()
                            .map(|w| format!("{}: {w}", config_path.display())),
                    );
                    self.config = config;
                }
                Err(e) => diagnostics.push(format!("{}: {e}", config_path.display())),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => self.config = Config::default(),
            Err(e) => diagnostics.push(format!("{}: {e}", config_path.display())),
        }

        let messages_changed = self.reload_messages(read, dir, &mut diagnostics);
        let theme_changed = self.reload_theme(read, dir, &mut diagnostics);

        if diagnostics == self.reported {
            diagnostics.clear();
        } else {
            self.reported = diagnostics.clone();
        }
        Reload {
            appearance: (messages_changed || theme_changed).then(|| Appearance {
                messages: self.messages.clone(),
                theme: self.theme.clone(),
            }),
            diagnostics,
        }
    }

    /// Rebuilds the messages for the configured language; true when they
    /// changed.
    fn reload_messages(
        &mut self,
        read: &dyn Fn(&Path) -> io::Result<String>,
        dir: &Path,
        diagnostics: &mut Vec<String>,
    ) -> bool {
        // `parse` only lets valid codes through, so this never leaves `locales/`.
        let language = self.config.language.clone();
        let locale_path = dir.join("locales").join(format!("{language}.toml"));
        let mut unusable = false;
        let text = match read(&locale_path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                diagnostics.push(format!("{}: {e}", locale_path.display()));
                unusable = true;
                None
            }
        };
        let resolved = resolve(&language, text.as_deref());
        // An unreadable file already has its diagnostic; "unknown language"
        // would be misleading.
        if !unusable {
            diagnostics.extend(
                resolved
                    .warnings
                    .iter()
                    .map(|w| format!("{}: {w}", locale_path.display())),
            );
        }
        unusable |= resolved.syntax_error;
        // A language file that cannot be read or parsed makes `resolve`
        // return the base; keep what was showing when the language did not
        // change.
        if unusable && language == self.shown {
            return false;
        }
        let new = Arc::new(resolved.messages);
        self.shown = language;
        if *new == *self.messages {
            false
        } else {
            self.messages = new;
            true
        }
    }

    /// Rebuilds the theme for the configured name; true when it changed.
    fn reload_theme(
        &mut self,
        read: &dyn Fn(&Path) -> io::Result<String>,
        dir: &Path,
        diagnostics: &mut Vec<String>,
    ) -> bool {
        // `parse` only lets valid names through, so this never leaves `themes/`.
        let name = self.config.theme.clone();
        let path = dir.join("themes").join(format!("{name}.toml"));
        let new = if RESERVED_THEMES.contains(&name.as_str()) {
            // Reserved names always mean the built-in; a file of that name
            // is ignored (a successful read means it exists).
            if read(&path).is_ok() {
                diagnostics.push(format!(
                    "{}: \"{name}\" is a reserved theme name, the file is ignored",
                    path.display()
                ));
            }
            match Theme::builtin(&name) {
                Some(theme) => theme,
                None => {
                    diagnostics.push(format!(
                        "{}: the omarchy theme comes with the Linux version, using default",
                        dir.join("config.toml").display()
                    ));
                    DARK
                }
            }
        } else {
            match read(&path) {
                Ok(text) => match palette::parse(&text, &Palette::DEFAULT) {
                    Ok((palette, warnings)) => {
                        diagnostics.extend(
                            warnings
                                .into_iter()
                                .map(|w| format!("{}: {w}", path.display())),
                        );
                        Theme::from_palette(&palette)
                    }
                    Err(e) => {
                        diagnostics.push(format!("{}: {e}", path.display()));
                        return self.keep_or_default(&name);
                    }
                },
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    diagnostics.push(format!(
                        "{}: theme file not found, using default",
                        path.display()
                    ));
                    DARK
                }
                Err(e) => {
                    diagnostics.push(format!("{}: {e}", path.display()));
                    return self.keep_or_default(&name);
                }
            }
        };
        self.shown_theme = name;
        self.set_theme(new)
    }

    /// A theme file that cannot be used: keep the theme showing when the
    /// name did not change, else fall back to the default.
    fn keep_or_default(&mut self, name: &str) -> bool {
        if name == self.shown_theme {
            return false;
        }
        self.shown_theme = name.to_owned();
        self.set_theme(DARK)
    }

    fn set_theme(&mut self, new: Theme) -> bool {
        if *self.theme == new {
            false
        } else {
            self.theme = Arc::new(new);
            true
        }
    }
}

fn read_file(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

/// Watches `dir` (the folder, recursively: editors save by replacing the
/// file) on a `config-watch` thread. Each change that alters the text sends
/// the new messages and theme on `tx` and wakes the app. Problems go to stderr; if the
/// watcher cannot start the app simply runs without live reload.
pub fn spawn(dir: PathBuf, mut loader: Loader, tx: Sender<Appearance>, ctx: egui::Context) {
    let spawned = std::thread::Builder::new()
        .name("config-watch".to_owned())
        .spawn(move || {
            let (events_tx, events_rx) = mpsc::channel();
            let mut debouncer = match new_debouncer(DEBOUNCE, move |result| {
                let _ = events_tx.send(result);
            }) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("polyrhythmic-metronome: no live reload: {e}");
                    return;
                }
            };
            if let Err(e) = debouncer.watcher().watch(&dir, RecursiveMode::Recursive) {
                eprintln!(
                    "polyrhythmic-metronome: cannot watch {}, no live reload: {e}",
                    dir.display()
                );
                return;
            }
            // A file changed between the caller's load and the watch being
            // active would be missed: check once now, then on every event.
            // `debouncer` lives as long as this loop.
            let mut pending = true;
            loop {
                if pending {
                    pending = false;
                } else {
                    match events_rx.recv() {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            eprintln!(
                                "polyrhythmic-metronome: {}: watch error: {e}",
                                dir.display()
                            );
                            continue;
                        }
                        Err(_) => return,
                    }
                }
                let reload = loader.reload(&read_file, &dir);
                for line in &reload.diagnostics {
                    eprintln!("polyrhythmic-metronome: {line}");
                }
                if let Some(appearance) = reload.appearance {
                    if tx.send(appearance).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("polyrhythmic-metronome: no config watcher, no live reload: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pm_ui::theme::{DARK, Theme};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn files(list: &[(&str, &str)]) -> impl Fn(&Path) -> io::Result<String> {
        let map: HashMap<PathBuf, String> = list
            .iter()
            .map(|(p, t)| (PathBuf::from(p), (*t).to_owned()))
            .collect();
        move |p| {
            map.get(p)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    #[test]
    fn missing_config_is_english() {
        let mut l = Loader::default();
        let r = l.reload(&files(&[]), Path::new("/c"));
        assert_eq!(*l.messages, Messages::english());
        assert!(r.appearance.is_none(), "nothing changed");
    }

    #[test]
    fn switching_language_and_back() {
        let mut l = Loader::default();
        let r = l.reload(
            &files(&[("/c/config.toml", "language = \"pt-BR\"")]),
            Path::new("/c"),
        );
        assert_eq!(r.appearance.unwrap().messages.add_pulse, "Adicionar pulso");
        let r = l.reload(&files(&[]), Path::new("/c"));
        assert_eq!(
            r.appearance.unwrap().messages.add_pulse,
            "Add pulse",
            "deleted config: back to the default"
        );
    }

    #[test]
    fn broken_config_keeps_the_last_valid_language() {
        let mut l = Loader::default();
        l.reload(
            &files(&[("/c/config.toml", "language = \"pt-BR\"")]),
            Path::new("/c"),
        );
        let r = l.reload(
            &files(&[("/c/config.toml", "language = \"pt-BR")]),
            Path::new("/c"),
        );
        assert!(r.appearance.is_none());
        assert_eq!(l.messages.add_pulse, "Adicionar pulso");
        assert!(r.diagnostics[0].contains("config.toml"));
    }

    #[test]
    fn broken_locale_keeps_the_previous_messages_of_that_language() {
        let mut l = Loader::default();
        let cfg = ("/c/config.toml", "language = \"de\"");
        l.reload(
            &files(&[cfg, ("/c/locales/de.toml", "add_pulse = \"Puls\"")]),
            Path::new("/c"),
        );
        let r = l.reload(
            &files(&[cfg, ("/c/locales/de.toml", "add_pulse = ")]),
            Path::new("/c"),
        );
        assert!(r.appearance.is_none());
        assert_eq!(l.messages.add_pulse, "Puls");
        assert!(r.diagnostics[0].contains("de.toml"));
    }

    #[test]
    fn same_diagnostics_are_not_reported_twice() {
        let mut l = Loader::default();
        let read = files(&[("/c/config.toml", "language = \"de\"\nfoo = 1")]);
        let first = l.reload(&read, Path::new("/c"));
        assert!(!first.diagnostics.is_empty());
        let second = l.reload(&read, Path::new("/c"));
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    }

    #[test]
    fn unreadable_locale_keeps_the_previous_messages_and_names_the_error() {
        let mut l = Loader::default();
        let cfg = ("/c/config.toml", "language = \"de\"");
        l.reload(
            &files(&[cfg, ("/c/locales/de.toml", "add_pulse = \"Puls\"")]),
            Path::new("/c"),
        );
        let list = [cfg];
        let ok = files(&list);
        let read = move |p: &Path| {
            if p.ends_with("de.toml") {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
            } else {
                ok(p)
            }
        };
        let r = l.reload(&read, Path::new("/c"));
        assert!(r.appearance.is_none());
        assert_eq!(l.messages.add_pulse, "Puls");
        assert_eq!(r.diagnostics.len(), 1, "{:?}", r.diagnostics);
        assert!(r.diagnostics[0].contains("de.toml"));
        assert!(r.diagnostics[0].contains("denied"));
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            static N: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "pm-watch-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn watcher_picks_up_a_rename_save_and_a_delete() {
        let dir = TempDir::new();
        let (tx, rx) = mpsc::channel();
        spawn(
            dir.0.clone(),
            Loader::default(),
            tx,
            egui::Context::default(),
        );
        // The watcher registers on its own thread; the write below may land
        // before or after that, and either way must be seen.
        let wait = Duration::from_secs(2);

        // Save the way editors do: write a temp file, then rename over.
        let tmp = dir.0.join("config.toml.tmp");
        fs::write(&tmp, "language = \"pt-BR\"").unwrap();
        fs::rename(&tmp, dir.0.join("config.toml")).unwrap();
        let m = rx
            .recv_timeout(wait)
            .expect("rename save delivers messages");
        assert_eq!(m.messages.add_pulse, "Adicionar pulso");

        fs::remove_file(dir.0.join("config.toml")).unwrap();
        let m = rx.recv_timeout(wait).expect("delete delivers messages");
        assert_eq!(m.messages.add_pulse, "Add pulse");

        // The watch is certainly active by now: a rename save must be seen.
        fs::write(&tmp, "language = \"pt-BR\"").unwrap();
        fs::rename(&tmp, dir.0.join("config.toml")).unwrap();
        let m = rx
            .recv_timeout(wait)
            .expect("second rename save delivers messages");
        assert_eq!(m.messages.add_pulse, "Adicionar pulso");
    }
    const CFG_X: (&str, &str) = ("/c/config.toml", "theme = \"x\"");

    #[test]
    fn user_theme_loads_and_missing_keys_come_from_default() {
        let mut l = Loader::default();
        let r = l.reload(
            &files(&[CFG_X, ("/c/themes/x.toml", "accent = \"#102030\"")]),
            Path::new("/c"),
        );
        let theme = r.appearance.expect("theme changed").theme;
        assert_eq!(theme.accent, egui::Color32::from_rgb(0x10, 0x20, 0x30));
        assert_eq!(theme.bg, DARK.bg, "absent keys come from default");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    }

    #[test]
    fn broken_theme_keeps_the_previous_theme_of_that_name() {
        let mut l = Loader::default();
        l.reload(
            &files(&[CFG_X, ("/c/themes/x.toml", "accent = \"#102030\"")]),
            Path::new("/c"),
        );
        let r = l.reload(
            &files(&[CFG_X, ("/c/themes/x.toml", "accent = \"orange\"")]),
            Path::new("/c"),
        );
        assert!(r.appearance.is_none());
        assert_eq!(
            l.theme.accent,
            egui::Color32::from_rgb(0x10, 0x20, 0x30),
            "previous theme kept"
        );
        assert_eq!(r.diagnostics.len(), 1, "{:?}", r.diagnostics);
        assert!(r.diagnostics[0].contains("x.toml"));
        assert!(r.diagnostics[0].contains("accent"));
        assert!(r.diagnostics[0].contains("orange"));
    }

    #[test]
    fn switching_to_a_broken_theme_falls_back_to_default() {
        let mut l = Loader::default();
        l.reload(
            &files(&[CFG_X, ("/c/themes/x.toml", "accent = \"#102030\"")]),
            Path::new("/c"),
        );
        let r = l.reload(
            &files(&[
                ("/c/config.toml", "theme = \"y\""),
                ("/c/themes/y.toml", "accent = \"orange\""),
            ]),
            Path::new("/c"),
        );
        assert_eq!(*r.appearance.unwrap().theme, DARK);
        assert!(r.diagnostics.iter().any(|d| d.contains("y.toml")));
    }

    #[test]
    fn missing_theme_file_uses_default_and_warns() {
        let mut l = Loader::default();
        let r = l.reload(&files(&[CFG_X]), Path::new("/c"));
        assert!(r.appearance.is_none(), "default was already showing");
        assert_eq!(*l.theme, DARK);
        assert_eq!(r.diagnostics.len(), 1, "{:?}", r.diagnostics);
        assert!(r.diagnostics[0].contains("x.toml"));
    }

    #[test]
    fn reserved_names_and_omarchy_warn_and_use_builtins() {
        let mut l = Loader::default();
        let r = l.reload(
            &files(&[
                ("/c/config.toml", "theme = \"light\""),
                ("/c/themes/light.toml", "accent = \"#102030\""),
            ]),
            Path::new("/c"),
        );
        let theme = r.appearance.expect("light differs from default").theme;
        assert_eq!(*theme, Theme::builtin("light").unwrap());
        assert_eq!(r.diagnostics.len(), 1, "{:?}", r.diagnostics);
        assert!(r.diagnostics[0].contains("light.toml"));

        let r = l.reload(
            &files(&[("/c/config.toml", "theme = \"omarchy\"")]),
            Path::new("/c"),
        );
        assert_eq!(*r.appearance.unwrap().theme, DARK);
        assert_eq!(r.diagnostics.len(), 1, "{:?}", r.diagnostics);
        assert!(r.diagnostics[0].contains("omarchy"));

        // The built-ins never read a file: no warning without one.
        let r = l.reload(
            &files(&[("/c/config.toml", "theme = \"light\"")]),
            Path::new("/c"),
        );
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    }

    #[test]
    fn language_and_theme_change_independently() {
        let mut l = Loader::default();
        let pt = ("/c/themes/x.toml", "accent = \"#102030\"");
        let r = l.reload(
            &files(&[
                ("/c/config.toml", "language = \"pt-BR\"\ntheme = \"x\""),
                pt,
            ]),
            Path::new("/c"),
        );
        let first = r.appearance.unwrap();
        // Only the theme changes.
        let r = l.reload(
            &files(&[("/c/config.toml", "language = \"pt-BR\"\ntheme = \"light\"")]),
            Path::new("/c"),
        );
        let second = r.appearance.unwrap();
        assert_eq!(second.messages, first.messages);
        assert_ne!(second.theme, first.theme);
        // Only the language changes.
        let r = l.reload(
            &files(&[("/c/config.toml", "language = \"en\"\ntheme = \"light\"")]),
            Path::new("/c"),
        );
        let third = r.appearance.unwrap();
        assert_eq!(third.theme, second.theme);
        assert_eq!(third.messages.add_pulse, "Add pulse");
    }

    #[test]
    fn syntax_error_in_a_locale_keeps_the_previous_messages_via_the_flag() {
        // The Loader reads `Resolved::syntax_error`, not the warning text.
        assert!(resolve("de", Some("add_pulse = ")).syntax_error);
        assert!(!resolve("de", Some("add_pulse = \"x\"")).syntax_error);
    }

    #[test]
    fn watcher_reloads_a_theme_file_edit() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.0.join("themes")).unwrap();
        fs::write(dir.0.join("config.toml"), "theme = \"x\"").unwrap();
        fs::write(dir.0.join("themes/x.toml"), "accent = \"#102030\"").unwrap();
        let mut loader = Loader::default();
        loader.reload(&read_file, &dir.0);
        let (tx, rx) = mpsc::channel();
        spawn(dir.0.clone(), loader, tx, egui::Context::default());
        let wait = Duration::from_secs(2);

        let tmp = dir.0.join("themes/x.toml.tmp");
        fs::write(&tmp, "accent = \"#405060\"").unwrap();
        fs::rename(&tmp, dir.0.join("themes/x.toml")).unwrap();
        let a = rx.recv_timeout(wait).expect("theme edit delivers");
        assert_eq!(a.theme.accent, egui::Color32::from_rgb(0x40, 0x50, 0x60));
    }
}
