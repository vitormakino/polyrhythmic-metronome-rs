//! UI messages: embedded English and Brazilian Portuguese, with a per-key
//! overlay from a user locale file.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

use serde::{Deserialize, Serialize};

/// Language codes shipped inside the binary.
pub const BUILTIN: [&str; 2] = ["en", "pt-BR"];

const EN_TOML: &str = include_str!("../locales/en.toml");
const PT_BR_TOML: &str = include_str!("../locales/pt-BR.toml");

/// Every user-visible string of the interface. Field name = TOML key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Messages {
    pub app_heading: String,
    pub tempo_heading: String,
    pub bpm: String,
    pub bpm_decrease: String,
    pub bpm_increase: String,
    pub tap: String,
    pub pin_mini: String,
    pub expand: String,
    pub topology: String,
    pub coincidence: String,
    pub engine: String,
    pub active_pulses: String,
    pub add_pulse: String,
    pub play: String,
    pub stop: String,
    pub pulse: String,
    pub pulse_move: String,
    pub pulse_settings: String,
    pub pulse_remove: String,
    pub pulse_mute: String,
    pub pulse_more_beats: String,
    pub pulse_fewer_beats: String,
    pub pulse_beat: String,
    pub pulse_sound: String,
    pub sound_heading: String,
    pub sound_sine: String,
    pub sound_wood: String,
    pub sound_bell: String,
    pub sound_electronic: String,
    pub no_output: String,
    pub underruns: String,
    pub settings: String,
    pub visual_offset: String,
    pub tray_bpm_up: String,
    pub tray_bpm_down: String,
    pub tray_open: String,
    pub tray_mini: String,
    pub tray_quit: String,
}

impl Messages {
    /// The English messages (always available).
    pub fn english() -> Messages {
        toml::from_str(EN_TOML).expect("embedded en.toml is valid")
    }

    /// The embedded messages for `code`, if it is one of [`BUILTIN`].
    pub fn builtin(code: &str) -> Option<Messages> {
        if code.eq_ignore_ascii_case("en") {
            Some(Messages::english())
        } else if code.eq_ignore_ascii_case("pt-BR") {
            Some(toml::from_str(PT_BR_TOML).expect("embedded pt-BR.toml is valid"))
        } else {
            None
        }
    }
}

/// Messages plus the problems found while building them.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub messages: Messages,
    pub warnings: Vec<String>,
    /// The user's locale file did not parse, so `messages` is only the base.
    pub syntax_error: bool,
}

/// Flat key -> text view of the messages.
fn table(m: &Messages) -> BTreeMap<String, String> {
    match serde_json::to_value(m).expect("Messages serializes") {
        serde_json::Value::Object(map) => map
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_owned())))
            .collect(),
        _ => unreachable!("Messages is a struct"),
    }
}

/// Names of the `{name}` placeholders in `text`.
fn placeholders(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                out.insert(after[..close].to_owned());
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    out
}

/// Messages for `code`, overlaid key by key with `user_file` (TOML text).
///
/// The base is the built-in language, or English for any other code. Never
/// fails: problems become entries in `warnings` and the base text is kept.
pub fn resolve(code: &str, user_file: Option<&str>) -> Resolved {
    let builtin = Messages::builtin(code);
    let mut warnings = Vec::new();
    if builtin.is_none() && user_file.is_none() {
        warnings.push(format!(
            "unknown language \"{code}\": no built-in messages and no locale file, using English"
        ));
    }
    let base = builtin.unwrap_or_else(Messages::english);
    let Some(text) = user_file else {
        return Resolved {
            messages: base,
            warnings,
            syntax_error: false,
        };
    };

    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let overlay: toml::Table = match toml::from_str(text) {
        Ok(t) => t,
        Err(e) => {
            let line = e
                .span()
                .map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
            let at = line.map(|l| format!("line {l}: ")).unwrap_or_default();
            warnings.push(format!("locale file syntax error, {at}{}", e.message()));
            return Resolved {
                messages: base,
                warnings,
                syntax_error: true,
            };
        }
    };

    let english = table(&Messages::english());
    let mut merged = table(&base);
    for (key, value) in overlay {
        let Some(slot) = merged.get_mut(&key) else {
            warnings.push(format!("unknown key \"{key}\" ignored"));
            continue;
        };
        let Some(s) = value.as_str() else {
            warnings.push(format!("key \"{key}\" must be a string, ignored"));
            continue;
        };
        if placeholders(s) != placeholders(&english[&key]) {
            warnings.push(format!(
                "key \"{key}\" uses different placeholders than the English text"
            ));
        }
        *slot = s.to_owned();
    }
    let messages = serde_json::from_value(serde_json::to_value(&merged).expect("map serializes"))
        .expect("merged table has exactly the Messages keys");
    Resolved {
        messages,
        warnings,
        syntax_error: false,
    }
}

/// Replaces each `{name}` found in `args`; everything else stays literal.
pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = template.to_owned();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

/// JSON Schema (draft 4) for locale files, for editor completion.
pub fn locale_schema() -> serde_json::Value {
    let properties: serde_json::Map<String, serde_json::Value> = table(&Messages::english())
        .into_iter()
        .map(|(k, text)| {
            (
                k,
                serde_json::json!({ "type": "string", "description": text }),
            )
        })
        .collect();
    serde_json::json!({
        "$schema": "http://json-schema.org/draft-04/schema#",
        "title": "Polyrhythmic Metronome locale",
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_codes_match_ignoring_case() {
        let r = resolve("pt-br", None);
        assert_eq!(r.messages, Messages::builtin("pt-BR").unwrap());
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn builtins_parse_strictly_and_have_the_same_placeholders() {
        // deny_unknown_fields + every field required: a missing or extra key fails here.
        let en = Messages::english();
        let pt = Messages::builtin("pt-BR").unwrap();
        for (key, en_text) in table(&en) {
            assert_eq!(
                placeholders(&table(&pt)[&key]),
                placeholders(&en_text),
                "{key}"
            );
        }
    }

    #[test]
    fn pt_br_keeps_the_v1_texts() {
        let pt = Messages::builtin("pt-BR").unwrap();
        assert_eq!(pt.add_pulse, "Adicionar pulso");
        assert_eq!(pt.topology, "Topologia rítmica");
        assert_eq!(fill(&pt.underruns, &[("n", &3)]), "Falhas de áudio: 3");
        assert_eq!(pt.visual_offset, "Offset visual (ms)");
    }

    #[test]
    fn unknown_code_without_file_falls_back_to_english_with_a_warning() {
        let r = resolve("de", None);
        assert_eq!(r.messages, Messages::english());
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn user_file_overlays_key_by_key_and_missing_keys_come_from_the_base() {
        let r = resolve("pt-BR", Some("add_pulse = \"Novo pulso\"\n"));
        assert_eq!(r.messages.add_pulse, "Novo pulso");
        assert_eq!(r.messages.topology, "Topologia rítmica");
        let r = resolve("de", Some("add_pulse = \"Puls hinzufügen\"\n"));
        assert_eq!(r.messages.add_pulse, "Puls hinzufügen");
        assert_eq!(
            r.messages.topology, "Rhythmic topology",
            "new language: English base"
        );
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn unknown_keys_and_non_strings_are_warnings_not_errors() {
        let r = resolve("en", Some("nope = \"x\"\nplay = 3\nstop = \"Halt\"\n"));
        assert_eq!(r.messages.stop, "Halt");
        assert_eq!(r.messages.play, "Play");
        assert_eq!(r.warnings.len(), 2);
    }

    #[test]
    fn syntax_error_keeps_the_base_and_names_the_line() {
        let r = resolve("pt-BR", Some("play = \"ok\"\nstop = \n"));
        assert_eq!(r.messages, Messages::builtin("pt-BR").unwrap());
        assert!(r.warnings[0].contains("line 2"), "{:?}", r.warnings);
    }

    #[test]
    fn bom_and_crlf_are_accepted() {
        let r = resolve("en", Some("\u{feff}play = \"Go\"\r\nstop = \"Halt\"\r\n"));
        assert_eq!(
            (r.messages.play.as_str(), r.messages.stop.as_str()),
            ("Go", "Halt")
        );
    }

    #[test]
    fn placeholder_mismatch_warns_and_fill_never_panics() {
        let r = resolve("en", Some("underruns = \"Dropouts: {x}\"\n"));
        assert_eq!(r.warnings.len(), 1);
        assert_eq!(fill(&r.messages.underruns, &[("n", &3)]), "Dropouts: {x}");
        assert_eq!(fill("{n} and {n}", &[("n", &1)]), "1 and 1");
        assert_eq!(fill("{", &[]), "{");
    }

    #[test]
    fn locale_schema_lists_every_key_with_its_english_text() {
        let s = locale_schema();
        assert_eq!(s["additionalProperties"], false);
        assert!(s.get("required").is_none());
        assert_eq!(s["properties"]["add_pulse"]["type"], "string");
        assert_eq!(s["properties"]["add_pulse"]["description"], "Add pulse");
        assert_eq!(
            s["properties"].as_object().unwrap().len(),
            table(&Messages::english()).len()
        );
    }
}
