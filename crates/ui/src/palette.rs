//! Color palettes in the Omarchy `colors.toml` format: an optional `mode` and
//! 25 optional `#rrggbb` / `#rgb` colors. Missing keys come from a base palette.

use egui::Color32;

/// Whether a palette is meant for a dark or a light window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

macro_rules! palette {
    ($($field:ident),* $(,)?) => {
        /// One color per Omarchy key.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Palette {
            pub mode: Mode,
            $(pub $field: Color32,)*
        }

        /// The color keys of a palette file, in the order of the Omarchy format.
        pub const KEYS: [&str; 25] = [$(stringify!($field)),*];

        impl Palette {
            fn slot(&mut self, key: &str) -> Option<&mut Color32> {
                match key {
                    $(stringify!($field) => Some(&mut self.$field),)*
                    _ => None,
                }
            }

            fn color(&self, key: &str) -> Color32 {
                match key {
                    $(stringify!($field) => self.$field,)*
                    _ => unreachable!("not a palette key"),
                }
            }
        }
    };
}

palette!(
    accent,
    selection,
    muted,
    background,
    dark_background,
    darker_background,
    lighter_background,
    foreground,
    dark_foreground,
    light_foreground,
    bright_foreground,
    red,
    yellow,
    orange,
    green,
    cyan,
    blue,
    magenta,
    brown,
    bright_red,
    bright_yellow,
    bright_green,
    bright_cyan,
    bright_blue,
    bright_magenta,
);

/// Themes shipped inside the binary.
pub const BUILTIN_THEMES: [&str; 2] = ["default", "light"];
/// Names a user theme file cannot take.
pub const RESERVED_THEMES: [&str; 3] = ["default", "light", "omarchy"];

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Palette {
    /// The v1 look (dark zinc, orange accent) in the Omarchy format.
    pub const DEFAULT: Palette = Palette {
        mode: Mode::Dark,
        background: hex(0x09090b),
        dark_background: hex(0x050506),
        darker_background: hex(0x000000),
        lighter_background: hex(0x18181b),
        selection: hex(0x27272a),
        muted: hex(0x3f3f46),
        foreground: hex(0xf4f4f5),
        dark_foreground: hex(0x71717a),
        light_foreground: hex(0xd4d4d8),
        bright_foreground: hex(0xffffff),
        accent: hex(0xf97316),
        orange: hex(0xf97316),
        blue: hex(0x3b82f6),
        green: hex(0x10b981),
        magenta: hex(0xa855f7),
        red: hex(0xf43f5e),
        yellow: hex(0xeab308),
        cyan: hex(0x06b6d4),
        brown: hex(0x78350f),
        bright_red: hex(0xfb7185),
        bright_yellow: hex(0xfacc15),
        bright_green: hex(0x34d399),
        bright_cyan: hex(0x22d3ee),
        bright_blue: hex(0x60a5fa),
        bright_magenta: hex(0xc084fc),
    };

    /// The built-in light palette.
    pub const LIGHT: Palette = Palette {
        mode: Mode::Light,
        background: hex(0xf4f4f5),
        dark_background: hex(0xe4e4e7),
        darker_background: hex(0xd4d4d8),
        lighter_background: hex(0xffffff),
        selection: hex(0xe4e4e7),
        muted: hex(0xd4d4d8),
        foreground: hex(0x18181b),
        dark_foreground: hex(0x71717a),
        light_foreground: hex(0x3f3f46),
        bright_foreground: hex(0x09090b),
        accent: hex(0xea580c),
        orange: hex(0xea580c),
        blue: hex(0x2563eb),
        green: hex(0x059669),
        magenta: hex(0x9333ea),
        red: hex(0xe11d48),
        yellow: hex(0xca8a04),
        cyan: hex(0x0891b2),
        brown: hex(0x92400e),
        bright_red: hex(0xf43f5e),
        bright_yellow: hex(0xeab308),
        bright_green: hex(0x10b981),
        bright_cyan: hex(0x06b6d4),
        bright_blue: hex(0x3b82f6),
        bright_magenta: hex(0xa855f7),
    };
}

/// `#rrggbb` or `#rgb`, either case.
fn parse_color(s: &str) -> Option<Color32> {
    let digits = s.strip_prefix('#')?;
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let nibble = |i: usize| u8::from_str_radix(&digits[i..=i], 16).ok();
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    match digits.len() {
        3 => Some(Color32::from_rgb(
            nibble(0)? * 17,
            nibble(1)? * 17,
            nibble(2)? * 17,
        )),
        6 => Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        _ => None,
    }
}

/// 1-based line of a top-level `key =`, or 1 when it cannot be found.
fn key_line(text: &str, key: &str) -> usize {
    text.lines()
        .position(|l| {
            l.trim_start()
                .strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
        .map_or(1, |i| i + 1)
}

/// Parses a palette file over `base`. Returns the palette and one warning per
/// unknown key; a syntax error or an invalid value rejects the whole file.
pub fn parse(text: &str, base: &Palette) -> Result<(Palette, Vec<String>), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let table: toml::Table = toml::from_str(text).map_err(|e| {
        let line = e
            .span()
            .map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
        let at = line.map(|l| format!("line {l}: ")).unwrap_or_default();
        format!("theme file syntax error, {at}{}", e.message())
    })?;

    let mut palette = *base;
    let mut warnings = Vec::new();
    for (key, value) in table {
        if key == "mode" {
            palette.mode = match value.as_str() {
                Some("dark") => Mode::Dark,
                Some("light") => Mode::Light,
                _ => {
                    return Err(format!(
                        "line {}: mode = {value}: expected \"dark\" or \"light\"",
                        key_line(text, "mode")
                    ));
                }
            };
        } else if let Some(slot) = palette.slot(&key) {
            let color = value.as_str().and_then(parse_color);
            *slot = color.ok_or_else(|| {
                format!(
                    "line {}: {key} = {value}: expected a color like \"#rrggbb\" or \"#rgb\"",
                    key_line(text, &key)
                )
            })?;
        } else {
            warnings.push(format!("unknown theme key \"{key}\" ignored"));
        }
    }
    Ok((palette, warnings))
}

/// Role of each key, for the schema descriptions.
fn role(key: &str) -> &'static str {
    match key {
        "accent" => "accent color (Play button, selection, top accent)",
        "selection" => "card and widget borders",
        "muted" => "inactive beat dots and pressed widgets",
        "background" => "window background",
        "dark_background" => "not used by the app (Omarchy key)",
        "darker_background" => "not used by the app (Omarchy key)",
        "lighter_background" => "card background",
        "foreground" => "primary text",
        "dark_foreground" => "labels and idle icons",
        "light_foreground" => "not used by the app (Omarchy key)",
        "bright_foreground" => "strongest text and highlights",
        "orange" => "pulse color 1",
        "blue" => "pulse color 2",
        "green" => "pulse color 3",
        "magenta" => "pulse color 4",
        "red" => "pulse color 5",
        "yellow" => "pulse color 6",
        "cyan" => "cyan terminal color",
        "brown" => "brown terminal color",
        "bright_red" => "bright red terminal color",
        "bright_yellow" => "bright yellow terminal color",
        "bright_green" => "bright green terminal color",
        "bright_cyan" => "bright cyan terminal color",
        "bright_blue" => "bright blue terminal color",
        "bright_magenta" => "bright magenta terminal color",
        _ => "",
    }
}

/// JSON Schema (Draft 4) for a theme file, for editor completion and checks.
pub fn theme_schema() -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "mode".into(),
        serde_json::json!({
            "enum": ["dark", "light"],
            "description": "dark or light window; default dark",
        }),
    );
    for key in KEYS {
        let d = Palette::DEFAULT.color(key);
        properties.insert(
            key.into(),
            serde_json::json!({
                "type": "string",
                "pattern": "^#([0-9A-Fa-f]{3}|[0-9A-Fa-f]{6})$",
                "description": format!(
                    "{}; default #{:02x}{:02x}{:02x}",
                    role(key),
                    d.r(),
                    d.g(),
                    d.b()
                ),
            }),
        );
    }
    serde_json::json!({
        "$schema": "http://json-schema.org/draft-04/schema#",
        "title": "Polyrhythmic Metronome theme",
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
    })
}

/// `^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$`: a theme name never names a path.
pub fn valid_theme_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_omarchy_colors_toml_parses_with_a_warning_for_unknown_keys() {
        let text = include_str!("../tests/fixtures/tokyo-night.toml");
        let (p, w) = parse(text, &Palette::DEFAULT).unwrap();
        assert_eq!(p.background, Color32::from_rgb(0x1a, 0x1b, 0x26));
        assert_eq!(p.mode, Mode::Dark);
        assert_eq!(w.len(), 1, "theme_type is unknown: {w:?}");
    }

    #[test]
    fn missing_keys_come_from_the_base() {
        let (p, w) = parse("accent = \"#123\"\n", &Palette::DEFAULT).unwrap();
        assert_eq!(p.accent, Color32::from_rgb(0x11, 0x22, 0x33));
        assert_eq!(p.background, Palette::DEFAULT.background);
        assert!(w.is_empty());
    }

    #[test]
    fn invalid_colors_and_modes_reject_the_file_naming_key_and_value() {
        for (text, needle) in [
            ("accent = \"orange\"", "orange"),
            ("accent = \"#12345\"", "#12345"),
            ("accent = \"rgb(1,2,3)\"", "rgb(1,2,3)"),
            ("accent = 3", "accent"),
            ("mode = \"dim\"", "dim"),
        ] {
            // The offending key is on line 3.
            let text = format!("# c\nmuted = \"#123\"\n{text}\n");
            let e = parse(&text, &Palette::DEFAULT).unwrap_err();
            assert!(
                e.contains(needle) && (e.contains("accent") || e.contains("mode")),
                "{text}: {e}"
            );
            assert!(e.contains("line 3"), "{text}: {e}");
        }
        assert!(
            parse("accent = \"#ABCDEF\"\r\n", &Palette::DEFAULT).is_ok(),
            "uppercase + CRLF"
        );
        assert!(
            parse("\u{feff}accent = \"#abc\"", &Palette::DEFAULT).is_ok(),
            "BOM"
        );
        assert!(
            parse("accent = \"#abc", &Palette::DEFAULT)
                .unwrap_err()
                .contains("line 1")
        );
    }

    #[test]
    fn theme_names_cannot_name_paths() {
        for ok in ["tokyo-night", "my_theme", "a", "Nord2"] {
            assert!(valid_theme_name(ok), "{ok}");
        }
        for bad in [
            "",
            "-x",
            "../x",
            "a/b",
            "a.toml",
            "with space",
            &"x".repeat(65),
        ] {
            assert!(!valid_theme_name(bad), "{bad}");
        }
        assert!(valid_theme_name(&"x".repeat(64)));
    }

    #[test]
    fn theme_schema_is_draft_4_and_lists_every_key() {
        let s = theme_schema();
        let text = s.to_string();
        for banned in ["\"$defs\"", "\"$ref\"", "\"const\""] {
            assert!(!text.contains(banned));
        }
        assert_eq!(s["$schema"], "http://json-schema.org/draft-04/schema#");
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["properties"].as_object().unwrap().len(), KEYS.len() + 1);
        assert_eq!(
            s["properties"]["mode"]["enum"],
            serde_json::json!(["dark", "light"])
        );
        assert_eq!(
            s["properties"]["accent"]["description"],
            "accent color (Play button, selection, top accent); default #f97316"
        );
    }
}
