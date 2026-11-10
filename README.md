# polyrhythmic-metronome-rs

A native macOS polyrhythmic metronome written in Rust (egui + CoreAudio).
Stack several pulses with different beat counts (4 against 3, 7 against 11,
up to 16 pulses), set the tempo from 40 to 300 BPM, and shape every click:
sound, pitch and intensity per beat.

Its main promise is that the clicks never drift or slip, even when the Mac is
under heavy load. Click positions are computed with exact integer math from a
fixed anchor, and the audio callback never allocates, locks or does I/O.

This is the native port of the web app:
<https://vitormakino.github.io/advanced-polyrhythmic-metronome/>

## Requirements

- macOS 11 or later, Apple Silicon or Intel.
- Xcode Command Line Tools (`xcode-select --install`).
- Rust stable (`rust-toolchain.toml` selects the stable channel with rustfmt
  and clippy).

## Run

```sh
cargo run --release -p polyrhythmic-metronome-app
```

Settings (pattern, BPM, window positions, visual offset per output device) are
saved in `~/Library/Application Support/polyrhythmic-metronome/settings.toml`.

### Build a local .app

```sh
scripts/bundle-macos.sh
open target/bundle/PolyrhythmicMetronome.app
```

The script builds the release binary, wraps it in an `.app` bundle
(`io.github.vitormakino.polyrhythmic-metronome`) and signs it ad hoc
(`codesign -s -`). The result is for your own machine; there is no DMG,
notarization or App Store build.

## Using it

- **Tempo:** −/+ buttons, the slider, or **TAP**. The first pulse in the list
  defines the measure: its beats fill one measure at the chosen BPM, and every
  other pulse divides that same measure evenly.
- **Pulses:** "Add pulse" adds one; each card changes its beat count,
  mutes it, removes it, or is dragged to reorder. The settings button on a card
  opens per-beat intensities (click a bar to cycle mute → soft → medium → hard)
  and the sound (Sine, Wood, Bell, Electronic).
- **Edits while playing:** tempo, sound and intensity changes apply at once.
  Changes to the first pulse (its beat count, reordering, removing it) and new
  pulses start at the next "one".
- **Mini window:** the pin button ("Pin mini") shrinks the app to a small
  window that stays on top of other apps, with the rhythm topology, BPM and
  Play/Stop. "Expand" goes back to the full window.
- **Visual offset:** "Settings" in the footer has "Visual offset (ms)", a
  per-device delay for the flashes so they match what you hear (useful with
  Bluetooth headphones).
- **Device changes:** the app follows the macOS default output. Plugging or
  unplugging headphones continues from the same point in the measure.

## Keyboard shortcuts and menu bar

Global hotkeys work while another app has the focus:

| Shortcut | Action |
|----------|--------|
| Ctrl+Option+Space | Play / stop |
| Ctrl+Option+T | Tap tempo |
| Ctrl+Option+M | Switch between the full and mini window |

If another app or macOS already holds one of them, the terminal prints
`hotkey ... unavailable` and the other two keep working. Ctrl+Option+Space is
also the macOS shortcut for "Select next source in Input menu" when more than
one keyboard input source is enabled.

The menu bar item offers Play/Stop, BPM +1, BPM −1, "Open window", "Mini" and
"Quit". Closing the window leaves the metronome playing; reopen it from the
menu bar item (or with Ctrl+Option+M). Quit with "Quit" or ⌘Q.

## Configuration

The app keeps its files in `~/.config/polyrhythmic-metronome/` (or
`$XDG_CONFIG_HOME/polyrhythmic-metronome/` when that is set to an absolute
path), also on macOS. On the first run it creates `config.toml` there and never
rewrites it afterwards.

```toml
language = "en"   # "en", "pt-BR", or the code of a file in locales/
theme = "default" # "default", "light", or the name of a file in themes/
```

The interface is in English by default; `pt-BR` is built in. Saving
`config.toml` applies a new language at once, in the window and in the menu
bar, without restarting and without touching the audio. A file with an error
never stops the app: it keeps the last valid state and prints the file, line
and problem on stderr.

To translate, or to reword a built-in language, create
`locales/<code>.toml` (for example `locales/de.toml` plus `language = "de"`).
Start it with the directive `#:schema ../schema/locale.schema.json` and
override only the keys you want; the rest comes from the built-in language
(English for a code that is not built in). Keep the `{name}` placeholders of
the English text.

### Themes

`default` (dark) and `light` are built in. To use your own colors, create
`themes/<name>.toml` (for example `themes/nord.toml` plus `theme = "nord"`).
Start it with the directive `#:schema ../schema/theme.schema.json`. The keys
are the ones in an Omarchy `colors.toml` (`accent`, `background`, `foreground`,
`red`, `orange`, ... and `mode = "dark"` or `"light"`), so any Omarchy
`colors.toml` works as a theme file. Colors are `"#rrggbb"` or `"#rgb"`. Every
key is optional: a missing one comes from the `default` theme, and an unknown
key only prints a warning. A light theme needs `mode = "light"`: the mode is
not inferred from the colors. A copied Omarchy `colors.toml` may show editor
underlines for keys the app ignores (the schema has `additionalProperties:
false`); the app itself only warns.

Saving `config.toml` or the theme file applies the change at once, in the
window, the settings popover and the mini window, without restarting and
without touching the audio. A theme with an invalid color (`accent = "orange"`)
is rejected as a whole: the app keeps the previous theme and prints the file,
the key and the value on stderr. The names `default`, `light` and `omarchy` are
reserved, in any letter case: a file with one of those names is ignored (with a
warning when that name is the selected theme), and `theme = "omarchy"` uses
`default` (the Omarchy integration comes with the Linux version).

### Practice sessions

A session is a plan of stages for practice: a stage can jump or ramp the tempo,
mute bars on a cycle or at random, and turn single pulses off. Each session is
a file in `sessions/` (for example `sessions/warmup.toml`); the app creates the
folder but never writes a session file. Start the file with
`#:schema ../schema/session.schema.json`. The file name (letters, digits, `_`
and `-`) is the default name of the session.

```toml
#:schema ../schema/session.schema.json
name = "Warm-up 3:4"
on_finish = "stop"           # "stop" (default) | "continue" | "repeat"
show_muted_visuals = false   # default: the screen holds still in muted stretches

[[stage]]
bars = 8                     # length: bars = N or seconds = N

[[stage]]
bars = 32
tempo = { change = 2, every_bars = 4, max = 140 }   # +2 BPM every 4 bars, up to 140

[[stage]]
bars = 16
mute = { play_bars = 4, mute_bars = 4 }             # 4 bars play, 4 are muted

[[stage]]
seconds = 60
pulses = [ { pulse = 2, state = "off" } ]           # pulse 2 off (counting from 1)

[[stage]]
bars = 16
random_mute = { percent = 20, increase = 5, every_bars = 4, max = 60 }
```

Other `tempo` keys: `jump = 10` (relative) or `set = 120` (absolute), `min`
for a falling ramp, `every_seconds`, and `smooth = true`. A mute cycle can use
`play_seconds` and `mute_seconds` instead. A pulse state is `"off"`, `"muted"`
or `"on"`. The schema documents every key; an unknown key or a value out of
range makes the whole file invalid.

Saving a session file updates the list at once. A broken file keeps its last
valid version (or is not listed if it never was valid) and its file, line and
problem go to stderr; an edit takes effect the next time the session starts,
never in one that is running.

Pick a session in the list above the pulses and press Play (the button, the
menu bar item or the hotkey): it starts on the "one". While it runs, the
screen shows the session's tempo, and the tempo controls and the menu bar's
BPM items move it by the same amount they would move your own. A session never
changes your setup: on Stop, or when it ends with `on_finish = "stop"`, the
tempo and pulses go back to yours, plus any change you made during the
session. The selected
session is remembered across restarts. Manual checks for sessions are in
[docs/VERIFICATION.md](docs/VERIFICATION.md#practice-sessions).

The app writes JSON Schemas to `schema/` (only when their content changes) and
the files point to them with `#:schema` directives. Autocomplete and
validation need an editor with the Taplo TOML language server (Even Better
TOML in VS Code, Helix, Neovim with `taplo`). To check that Taplo is active, type
`lang` in `config.toml`: `language` should complete, and `language = "de/x"`
should be underlined.

## Project layout

| Path | What |
|------|------|
| `crates/core` | Domain model, exact timeline (scheduler), live edits, click synthesis, tap tempo. No I/O. |
| `crates/audio` | Real-time engine and the CoreAudio stream (cpal) that follows the default output. |
| `crates/ui` | egui views: full window, rhythm topology, mini window, theme. |
| `crates/app` | The `polyrhythmic-metronome` binary: windows, settings, menu bar, hotkeys. |
| `tools/stress` | Stress harness (not a default workspace member). |
| `docs/VERIFICATION.md` | Manual checks and stress results. |

## Development

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

UI tests drive the views headless through their accessibility labels and
compare snapshot images in `crates/ui/tests/snapshots/`. After an intended
visual change, regenerate them with `UPDATE_SNAPSHOTS=1 cargo test -p
polyrhythmic-metronome-ui` and look at the new PNGs before committing. Without
a GPU adapter the snapshot tests are skipped with a notice.

The JSON Schemas in `schema/` are generated from the code; after changing the
config or the messages, regenerate them with `UPDATE_SCHEMAS=1 cargo test -p
polyrhythmic-metronome-app`.

## Verification

Timing is checked by automated tests (including a simulated 24 h run of 7:11
at 300 BPM), by the stress harness, and by manual checks that need hardware.
The stress harness plays 7:11 at 300 BPM through the real audio output while
busy threads load the CPU, records what the audio callback rendered, and
compares it sample by sample with an offline reference render:

```sh
cargo run -p stress --release -- --minutes 10 --load <number of cores>
```

Procedures and results are in [docs/VERIFICATION.md](docs/VERIFICATION.md).

## Security

CI pins every GitHub Action to a commit SHA, runs with a read-only token, and
scans commits for leaked secrets with gitleaks (the full history weekly).
Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

## License

GPL-3.0-or-later. See `LICENSE`. The embedded fonts (Inter, JetBrains Mono)
are under the SIL OFL 1.1; their licenses are in `crates/ui/assets/fonts/`.
