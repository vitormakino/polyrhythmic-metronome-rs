//! `config.toml` in `~/.config/polyrhythmic-metronome`, plus the Draft 4 JSON
//! Schemas that give editors (Taplo) autocomplete for it and for `locales/`.
//!
//! The app creates `config.toml` once and never writes to it again; only
//! `schema/` is rewritten, and only when its content differs.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;

/// First-run content of `config.toml`.
pub const TEMPLATE: &str = include_str!("config_template.toml");

const DRAFT_4: &str = "http://json-schema.org/draft-04/schema#";
const DIR_NAME: &str = "polyrhythmic-metronome";

/// Config folder: `$XDG_CONFIG_HOME` when absolute, else `~/.config` (also on
/// macOS, where `dirs::config_dir()` would give `Library/Application Support`).
pub fn config_dir_from(xdg: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = xdg
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".config")))?;
    Some(base.join(DIR_NAME))
}

/// [`config_dir_from`] with the real environment.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(std::env::var_os("XDG_CONFIG_HOME"), dirs::home_dir())
}

/// The settings in `config.toml`.
#[derive(Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct Config {
    /// Interface language: "en" (default), "pt-BR", or the code of a file in locales/.
    #[serde(default = "default_language")]
    #[schemars(schema_with = "language_schema")]
    pub language: String,
}

fn default_language() -> String {
    "en".to_owned()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            language: default_language(),
        }
    }
}

const LANGUAGE_PATTERN: &str = "^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$";

fn language_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "description": "Interface language: \"en\" (default), \"pt-BR\", or the code of a file in locales/.",
        "default": "en",
        "anyOf": [
            { "enum": pm_ui::i18n::BUILTIN },
            { "type": "string", "pattern": LANGUAGE_PATTERN }
        ]
    })
}

/// Hand-written equivalent of `^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$`. Rejects
/// anything that could name a path.
pub fn valid_language(code: &str) -> bool {
    let mut parts = code.split('-');
    let first = parts.next().unwrap_or("");
    (2..=3).contains(&first.len())
        && first.bytes().all(|b| b.is_ascii_alphabetic())
        && parts.all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// Parses `config.toml`. `Ok` carries warnings (unknown keys); `Err` carries a
/// message with the line for bad syntax or an invalid `language`.
pub fn parse(text: &str) -> Result<(Config, Vec<String>), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut config = Config::default();
    let mut warnings = Vec::new();
    for (key, value) in &table {
        if key == "language" {
            let line = key_line(text, "language");
            match value.as_str() {
                Some(code) if valid_language(code) => config.language = code.to_owned(),
                Some(code) => {
                    return Err(format!("line {line}: invalid language code {code:?}"));
                }
                None => {
                    return Err(format!(
                        "line {line}: language must be a string, got {value}"
                    ));
                }
            }
        } else {
            warnings.push(format!("unknown key {key:?} ignored"));
        }
    }
    Ok((config, warnings))
}

/// 1-based line of a top-level `key =`, or 1 when it cannot be found.
fn key_line(text: &str, key: &str) -> usize {
    text.lines()
        .position(|l| {
            let l = l.trim_start();
            l.strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
        .map_or(1, |i| i + 1)
}

/// JSON Schema (Draft 4) for `config.toml`.
pub fn config_schema() -> serde_json::Value {
    let generator = schemars::generate::SchemaSettings::draft07()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    let mut schema = serde_json::to_value(generator.into_root_schema_for::<Config>())
        .expect("schema serializes");
    let root = schema.as_object_mut().expect("object schema");
    root.insert("$schema".into(), DRAFT_4.into());
    root.insert(
        "title".into(),
        "Polyrhythmic Metronome configuration".into(),
    );
    root.insert("additionalProperties".into(), false.into());
    root.remove("required");
    schema
}

fn pretty(value: &serde_json::Value) -> String {
    let mut s = serde_json::to_string_pretty(value).expect("json serializes");
    s.push('\n');
    s
}

/// Creates the folder, `config.toml` (only if absent), `locales/` and the
/// schemas (only when their content differs).
pub fn ensure_files(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir.join("schema"))?;
    fs::create_dir_all(dir.join("locales"))?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join("config.toml"))
    {
        Ok(mut f) => f.write_all(TEMPLATE.as_bytes())?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    write_if_changed(
        &dir.join("schema/config.schema.json"),
        &pretty(&config_schema()),
    )?;
    write_if_changed(
        &dir.join("schema/locale.schema.json"),
        &pretty(&pm_ui::i18n::locale_schema()),
    )
}

fn write_if_changed(path: &Path, content: &str) -> io::Result<()> {
    if fs::read_to_string(path).is_ok_and(|current| current == content) {
        return Ok(());
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            static N: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "pm-config-{}-{}",
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
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn check_or_update(rel: &str, value: &serde_json::Value) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel);
        let want = pretty(value);
        if std::env::var_os("UPDATE_SCHEMAS").is_some() {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &want).unwrap();
        }
        assert_eq!(
            fs::read_to_string(&path).unwrap_or_default(),
            want,
            "{rel} is stale; run with UPDATE_SCHEMAS=1"
        );
    }

    #[test]
    fn config_dir_prefers_absolute_xdg_then_home_dot_config() {
        let home = Some(PathBuf::from("/h"));
        assert_eq!(
            config_dir_from(Some("/x".into()), home.clone()),
            Some("/x/polyrhythmic-metronome".into())
        );
        assert_eq!(
            config_dir_from(Some("rel".into()), home.clone()),
            Some("/h/.config/polyrhythmic-metronome".into())
        );
        assert_eq!(
            config_dir_from(Some("".into()), home.clone()),
            Some("/h/.config/polyrhythmic-metronome".into())
        );
        assert_eq!(config_dir_from(None, None), None);
    }

    #[test]
    fn template_parses_to_the_defaults_and_starts_with_the_directive() {
        assert!(TEMPLATE.starts_with("#:schema ./schema/config.schema.json\n"));
        assert_eq!(parse(TEMPLATE).unwrap(), (Config::default(), vec![]));
    }

    #[test]
    fn parse_handles_missing_unknown_invalid_and_bom() {
        assert_eq!(parse("").unwrap().0.language, "en");
        let (c, w) = parse("\u{feff}language = \"pt-BR\"\r\ncolour = 1\r\n").unwrap();
        assert_eq!((c.language.as_str(), w.len()), ("pt-BR", 1));
        let err = parse("language = 3\n").unwrap_err();
        assert!(err.contains("line 1") && err.contains('3'), "{err}");
        let err = parse("# c\nlanguage = \"a/b\"\n").unwrap_err();
        assert!(err.contains("line 2") && err.contains("a/b"), "{err}");
        assert!(parse("language = \"pt-BR\n").is_err());
    }

    #[test]
    fn language_codes_cannot_name_paths() {
        for ok in ["en", "pt-BR", "zh-Hant-TW", "fil"] {
            assert!(valid_language(ok), "{ok}");
        }
        for bad in [
            "",
            "e",
            "../../etc/x",
            "pt/BR",
            "pt_BR",
            "en-",
            "toolonglang",
            "pt-BR.toml",
        ] {
            assert!(!valid_language(bad), "{bad}");
            assert!(parse(&format!("language = {bad:?}\n")).is_err(), "{bad}");
        }
    }

    #[test]
    fn schemas_are_draft_4_compatible() {
        for s in [config_schema(), pm_ui::i18n::locale_schema()] {
            let text = s.to_string();
            for banned in ["\"$defs\"", "\"$ref\"", "\"const\"", "\"definitions\""] {
                assert!(!text.contains(banned), "{banned}: {text}");
            }
            assert_eq!(s["$schema"], DRAFT_4);
        }
        let s = config_schema();
        assert_eq!(s["additionalProperties"], false);
        assert!(s.get("required").is_none());
        assert_eq!(s["properties"]["language"]["anyOf"][0]["enum"][1], "pt-BR");
    }

    #[test]
    fn committed_schemas_match_the_code() {
        // UPDATE_SCHEMAS=1 rewrites schema/*.json from the code, like UPDATE_SNAPSHOTS.
        check_or_update("schema/config.schema.json", &config_schema());
        check_or_update("schema/locale.schema.json", &pm_ui::i18n::locale_schema());
    }

    #[test]
    fn ensure_files_creates_once_and_never_rewrites_the_config() {
        let t = TempDir::new();
        let dir = &t.0;
        ensure_files(dir).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("config.toml")).unwrap(),
            TEMPLATE
        );
        fs::write(dir.join("config.toml"), "language = \"pt-BR\"\n").unwrap();
        fs::write(dir.join("schema/config.schema.json"), "stale").unwrap();
        ensure_files(dir).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("config.toml")).unwrap(),
            "language = \"pt-BR\"\n"
        );
        assert_ne!(
            fs::read_to_string(dir.join("schema/config.schema.json")).unwrap(),
            "stale"
        );
        assert!(dir.join("schema/locale.schema.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn ensure_files_in_a_read_only_dir_is_an_error_not_a_panic() {
        use std::os::unix::fs::PermissionsExt;
        let t = TempDir::new();
        fs::set_permissions(&t.0, fs::Permissions::from_mode(0o555)).unwrap();
        let result = ensure_files(&t.0.join("polyrhythmic-metronome"));
        fs::set_permissions(&t.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err());
    }
}
