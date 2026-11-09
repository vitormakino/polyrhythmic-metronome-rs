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

The interface is in Portuguese, like the labels quoted below.

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
- **Pulses:** "Adicionar pulso" adds one; each card changes its beat count,
  mutes it, removes it, or is dragged to reorder. The settings button on a card
  opens per-beat intensities (click a bar to cycle mute → soft → medium → hard)
  and the sound (Sine, Wood, Bell, Electronic).
- **Edits while playing:** tempo, sound and intensity changes apply at once.
  Changes to the first pulse (its beat count, reordering, removing it) and new
  pulses start at the next "one".
- **Mini window:** the pin button ("Fixar mini") shrinks the app to a small
  window that stays on top of other apps, with the rhythm topology, BPM and
  Play/Stop. "Expandir" goes back to the full window.
- **Visual offset:** "Configurações" in the footer has "Offset visual (ms)", a
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

The menu bar item offers Play/Stop, BPM +1, BPM −1, "Abrir janela", "Mini" and
"Sair". Closing the window leaves the metronome playing; reopen it from the
menu bar item (or with Ctrl+Option+M). Quit with "Sair" or ⌘Q.

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

## License

GPL-3.0-or-later. See `LICENSE`. The embedded fonts (Inter, JetBrains Mono)
are under the SIL OFL 1.1; their licenses are in `crates/ui/assets/fonts/`.
