//! What survives a restart: pattern, tempo, window mode and rects, and the
//! visual offset per output device. Stored as TOML in
//! `dirs::data_dir()/polyrhythmic-metronome/settings.toml`.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use eframe::egui::Vec2;
use pm_core::{Bpm, Pattern};
use pm_ui::state::WindowMode;
use pm_ui::views::footer::MAX_VISUAL_OFFSET_MS;
use pm_ui::views::mini;
use serde::Serialize;

use crate::windows::FULL_MIN_SIZE;

const FILE: &str = "settings.toml";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Settings {
    pub pattern: Pattern,
    pub bpm: Bpm,
    pub mode: WindowMode,
    /// `[x, y, width, height]` of the full window (outer position, inner size), in points.
    pub full_rect: Option<[f32; 4]>,
    /// Same for the mini window.
    pub mini_rect: Option<[f32; 4]>,
    /// Visual offset in ms by output device UID.
    pub visual_offsets: BTreeMap<String /*device uid*/, u32 /*ms*/>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            pattern: Pattern::default(),
            bpm: Bpm::new(120),
            mode: WindowMode::Full,
            full_rect: None,
            mini_rect: None,
            visual_offsets: BTreeMap::new(),
        }
    }
}

/// Where the app keeps its settings, if the platform has a data directory.
pub fn default_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("polyrhythmic-metronome"))
}

impl Settings {
    /// Reads `dir/settings.toml`. Never fails: a missing or unreadable file
    /// gives the defaults, and each field that is missing or invalid falls
    /// back to its default on its own. Loaded values are brought back within
    /// the model's limits (BPM range, pattern invariants, offsets up to
    /// [`MAX_VISUAL_OFFSET_MS`], window rects at least the mode's minimum size).
    pub fn load(dir: &Path) -> Settings {
        let table = std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok())
            .unwrap_or_default();
        fn field<T: serde::de::DeserializeOwned>(table: &toml::Table, key: &str) -> Option<T> {
            table.get(key).and_then(|v| v.clone().try_into().ok())
        }
        let defaults = Settings::default();
        let mut pattern: Pattern = field(&table, "pattern").unwrap_or(defaults.pattern);
        pattern.normalize();
        let visual_offsets = table
            .get("visual_offsets")
            .and_then(toml::Value::as_table)
            .map(|offsets| {
                offsets
                    .iter()
                    .filter_map(|(uid, ms)| {
                        let ms = u32::try_from(ms.as_integer()?).ok()?;
                        Some((uid.clone(), ms.min(MAX_VISUAL_OFFSET_MS)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Settings {
            pattern,
            bpm: field(&table, "bpm").unwrap_or(defaults.bpm),
            mode: field(&table, "mode").unwrap_or(defaults.mode),
            full_rect: field(&table, "full_rect").and_then(|r| valid_rect(r, FULL_MIN_SIZE)),
            mini_rect: field(&table, "mini_rect").and_then(|r| valid_rect(r, mini::MIN_SIZE)),
            visual_offsets,
        }
    }

    /// Writes `dir/settings.toml`, creating `dir` if needed. Writes a
    /// temporary file first and renames it over the old one, so a crash
    /// mid-write never leaves a half-written file.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let text = toml::to_string(self).map_err(io::Error::other)?;
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, dir.join(FILE))
    }
}

/// A stored `[x, y, w, h]` if every value is finite, grown to at least `min`.
fn valid_rect([x, y, w, h]: [f32; 4], min: Vec2) -> Option<[f32; 4]> {
    [x, y, w, h]
        .iter()
        .all(|v| v.is_finite())
        .then(|| [x, y, w.max(min.x), h.max(min.y)])
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use pm_core::{Intensity, MAX_PULSES};

    use super::*;

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            static N: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "pm-settings-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(dir: &Path, text: &str) {
        std::fs::write(dir.join(FILE), text).unwrap();
    }

    fn edited() -> Settings {
        let mut pattern = Pattern::default();
        let id = pattern.add_pulse().unwrap();
        pattern.set_beats(id, 7);
        pattern.cycle_intensity(id, 2);
        pattern.set_enabled(pattern.pulses[0].id, false);
        pattern.move_pulse(2, 0);
        Settings {
            pattern,
            bpm: Bpm::new(187),
            mode: WindowMode::Mini,
            full_rect: Some([10.0, 20.0, 900.0, 700.0]),
            mini_rect: Some([1200.0, 40.0, 260.0, 300.0]),
            visual_offsets: BTreeMap::from([
                (
                    "ExampleUSBInterface:1,2".to_owned(),
                    12,
                ),
                ("BuiltInSpeakerDevice".to_owned(), 0),
            ]),
        }
    }

    #[test]
    fn settings_roundtrip() {
        let dir = TempDir::new();
        let s = edited();
        s.save(&dir.0).unwrap();
        assert_eq!(Settings::load(&dir.0), s);
        // No temp file is left behind.
        let names: Vec<_> = std::fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(FILE)]);
    }

    #[test]
    fn corrupt_file_gives_defaults() {
        let dir = TempDir::new();
        assert_eq!(Settings::load(&dir.0), Settings::default(), "missing file");
        for text in [
            "this is = = not toml",
            "\u{0}\u{1}garbage",
            "",
            "bpm = [1, 2]",
        ] {
            write(&dir.0, text);
            assert_eq!(Settings::load(&dir.0), Settings::default(), "{text:?}");
        }
        std::fs::write(dir.0.join(FILE), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        assert_eq!(Settings::load(&dir.0), Settings::default(), "not UTF-8");
        assert_eq!(
            Settings::load(&dir.0.join("missing")),
            Settings::default(),
            "missing dir"
        );
    }

    #[test]
    fn missing_dir_created_on_save() {
        let dir = TempDir::new();
        let nested = dir.0.join("a").join("polyrhythmic-metronome");
        let s = edited();
        s.save(&nested).unwrap();
        assert!(nested.join(FILE).is_file());
        assert_eq!(Settings::load(&nested), s);
        // Saving again replaces the file.
        let mut t = s.clone();
        t.bpm = Bpm::new(60);
        t.save(&nested).unwrap();
        assert_eq!(Settings::load(&nested).bpm.get(), 60);
    }

    #[test]
    fn a_broken_field_falls_back_alone() {
        let dir = TempDir::new();
        let good = edited();
        good.save(&dir.0).unwrap();
        let text = std::fs::read_to_string(dir.0.join(FILE)).unwrap();
        let text = text.replace("bpm = 187", "bpm = \"fast\"");
        let text = text.replace("mode = \"Mini\"", "mode = \"Huge\"");
        write(&dir.0, &text);
        let loaded = Settings::load(&dir.0);
        assert_eq!(loaded.bpm, Bpm::new(120));
        assert_eq!(loaded.mode, WindowMode::Full);
        assert_eq!(loaded.pattern, good.pattern);
        assert_eq!(loaded.full_rect, good.full_rect);
        assert_eq!(loaded.visual_offsets, good.visual_offsets);
    }

    #[test]
    fn load_clamps_bpm_offsets_and_rects() {
        let dir = TempDir::new();
        write(
            &dir.0,
            "bpm = 999\n\
             full_rect = [0.0, 0.0, 100.0, 100.0]\n\
             mini_rect = [0.0, 0.0, nan, 300.0]\n\
             [visual_offsets]\n\
             a = 9000\n\
             b = -3\n\
             c = 40\n",
        );
        let s = Settings::load(&dir.0);
        assert_eq!(s.bpm.get(), 300);
        assert_eq!(s.visual_offsets.get("a"), Some(&500));
        assert_eq!(s.visual_offsets.get("b"), None, "not a u32: dropped");
        assert_eq!(s.visual_offsets.get("c"), Some(&40));
        // Too small for the full window: grown to its minimum size.
        assert_eq!(s.full_rect, Some([0.0, 0.0, 420.0, 640.0]));
        assert_eq!(s.mini_rect, None, "non-finite rect dropped");

        write(&dir.0, "bpm = 3\n");
        assert_eq!(Settings::load(&dir.0).bpm.get(), 40);
    }

    #[test]
    fn load_restores_pattern_invariants() {
        let dir = TempDir::new();
        let mut s = Settings::default();
        while s.pattern.add_pulse().is_some() {}
        s.save(&dir.0).unwrap();
        let mut text = std::fs::read_to_string(dir.0.join(FILE)).unwrap();
        // One pulse too many, a 0-beat pulse, a duplicate id and a stale next_id.
        let extra = text[text.find("[[pattern.pulses]]").unwrap()..]
            .split("[[pattern.pulses]]")
            .nth(1)
            .unwrap()
            .to_owned();
        text.push_str("[[pattern.pulses]]");
        text.push_str(&extra);
        let text = text.replacen("beats = 4", "beats = 0", 1);
        let text = text.replace("next_id = 16", "next_id = 1");
        write(&dir.0, &text);

        let mut p = Settings::load(&dir.0).pattern;
        assert_eq!(p.pulses.len(), MAX_PULSES);
        assert_eq!(p.pulses[0].beats, 1);
        assert_eq!(p.pulses[0].intensities, vec![Intensity::Hard]);
        for x in &p.pulses {
            assert_eq!(x.intensities.len(), x.beats as usize);
        }
        let mut ids: Vec<_> = p.pulses.iter().map(|x| x.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), MAX_PULSES, "unique ids");
        // next_id is past every id: a new pulse gets an unused one.
        p.remove(p.pulses[0].id);
        let fresh = p.add_pulse().unwrap();
        assert_eq!(p.pulses.iter().filter(|x| x.id == fresh).count(), 1);

        write(&dir.0, "[pattern]\nnext_id = 0\npulses = []\n");
        assert_eq!(Settings::load(&dir.0).pattern, Pattern::default());
    }
}
