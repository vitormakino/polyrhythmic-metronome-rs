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
- Rust stable (the repo pins the toolchain in `rust-toolchain.toml`).

## Run

```sh
cargo run --release -p polyrhythmic-metronome-app
```

Settings are saved in
`~/Library/Application Support/polyrhythmic-metronome/settings.toml`.

### Build a local .app

```sh
scripts/bundle-macos.sh
open target/bundle/PolyrhythmicMetronome.app
```

The script builds the release binary, wraps it in an `.app` bundle
(`io.github.vitormakino.polyrhythmic-metronome`) and signs it ad hoc
(`codesign -s -`). The result is for your own machine; there is no DMG,
notarization or App Store build.

## Keyboard shortcuts and menu bar

Global hotkeys work while another app has the focus:

| Shortcut | Action |
|----------|--------|
| Ctrl+Option+Space | Play / stop |
| Ctrl+Option+T | Tap tempo |
| Ctrl+Option+M | Switch between the full and mini window |

If another app already holds one of them, the terminal prints
`hotkey ... unavailable` and the other two keep working.

The menu bar item offers Play/Stop, BPM +1, BPM -1, open the window, Mini and
Quit. Closing the window leaves the metronome playing from the menu bar.

## Verification

Timing is checked by automated tests (`cargo test --workspace`), by the stress
harness (`cargo run -p stress --release -- --minutes 10 --load <number of cores>`, which
records the real audio callback under CPU load and measures every click
against the timeline), and by manual checks that need hardware. Procedures
and results are in [docs/VERIFICATION.md](docs/VERIFICATION.md).

## License

GPL-3.0-or-later. See `LICENSE`. The embedded fonts (Inter, JetBrains Mono)
are under the SIL OFL 1.1; their licenses are in `crates/ui/assets/fonts/`.
