//! Loads the language from `config.toml` / `locales/` and reloads it when
//! those files change. Reading and parsing happen here, on the `config-watch`
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

use crate::config::{self, Config};

/// Quiet period after the last file event before reloading (an editor save
/// is often several events).
const DEBOUNCE: Duration = Duration::from_millis(200);

/// The last valid configuration and the messages built from it.
pub struct Loader {
    config: Config,
    /// Language the current `messages` were built for.
    shown: String,
    pub messages: Arc<Messages>,
}

impl Default for Loader {
    fn default() -> Self {
        let config = Config::default();
        Loader {
            shown: config.language.clone(),
            config,
            messages: Arc::new(Messages::english()),
        }
    }
}

/// What a reload produced.
pub struct Reload {
    /// The new messages; `Some` only when the text changed.
    pub messages: Option<Arc<Messages>>,
    /// One line per problem, each naming the file.
    pub diagnostics: Vec<String>,
}

impl Loader {
    /// Re-reads `config.toml` and the language file in `dir` through `read`.
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

        // `parse` only lets valid codes through, so this never leaves `locales/`.
        let language = self.config.language.clone();
        let locale_path = dir.join("locales").join(format!("{language}.toml"));
        let text = match read(&locale_path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                diagnostics.push(format!("{}: {e}", locale_path.display()));
                None
            }
        };
        let resolved = resolve(&language, text.as_deref());
        diagnostics.extend(
            resolved
                .warnings
                .iter()
                .map(|w| format!("{}: {w}", locale_path.display())),
        );
        let syntax_error = resolved
            .warnings
            .iter()
            .any(|w| w.starts_with(SYNTAX_ERROR));
        // A syntax error in the language file makes `resolve` return the
        // base; keep what was showing when the language did not change.
        let messages = if syntax_error && language == self.shown {
            None
        } else {
            let new = Arc::new(resolved.messages);
            self.shown = language;
            if *new == *self.messages {
                None
            } else {
                self.messages = new.clone();
                Some(new)
            }
        };
        Reload {
            messages,
            diagnostics,
        }
    }
}

/// Start of the warning `resolve` gives for a locale file that does not parse.
const SYNTAX_ERROR: &str = "locale file syntax error";

fn read_file(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

/// Watches `dir` (the folder, recursively: editors save by replacing the
/// file) on a `config-watch` thread. Each change that alters the text sends
/// the new messages on `tx` and wakes the app. Problems go to stderr; if the
/// watcher cannot start the app simply runs without live reload.
pub fn spawn(dir: PathBuf, mut loader: Loader, tx: Sender<Arc<Messages>>, ctx: egui::Context) {
    let spawned = std::thread::Builder::new()
        .name("config-watch".to_owned())
        .spawn(move || {
            let (events_tx, events_rx) = mpsc::channel();
            let mut debouncer = match new_debouncer(DEBOUNCE, move |result| {
                let _ = events_tx.send(result);
            }) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("polyrhythmic-metronome: no live reload of the language: {e}");
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
                if let Some(messages) = reload.messages {
                    if tx.send(messages).is_err() {
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
        assert!(r.messages.is_none(), "nothing changed");
    }

    #[test]
    fn switching_language_and_back() {
        let mut l = Loader::default();
        let r = l.reload(
            &files(&[("/c/config.toml", "language = \"pt-BR\"")]),
            Path::new("/c"),
        );
        assert_eq!(r.messages.unwrap().add_pulse, "Adicionar pulso");
        let r = l.reload(&files(&[]), Path::new("/c"));
        assert_eq!(
            r.messages.unwrap().add_pulse,
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
        assert!(r.messages.is_none());
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
        assert!(r.messages.is_none());
        assert_eq!(l.messages.add_pulse, "Puls");
        assert!(r.diagnostics[0].contains("de.toml"));
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
        assert_eq!(m.add_pulse, "Adicionar pulso");

        fs::remove_file(dir.0.join("config.toml")).unwrap();
        let m = rx.recv_timeout(wait).expect("delete delivers messages");
        assert_eq!(m.add_pulse, "Add pulse");
    }
}
