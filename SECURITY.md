# Security Policy

## Reporting a vulnerability

Please **do not open a public issue** for security problems. Report them
through a [private security advisory](https://github.com/vitormakino/polyrhythmic-metronome-rs/security/advisories/new)
instead. You should get an answer within 14 days.

## Scope

polyrhythmic-metronome-rs is a local desktop app for macOS:

- It makes no network connections and has no accounts or telemetry.
- It reads and writes only its own settings file
  (`~/Library/Application Support/polyrhythmic-metronome/settings.toml`);
  a malformed file is ignored and replaced by defaults, never trusted.
- It registers three global hotkeys (Carbon `RegisterEventHotKey`), which
  needs no Accessibility permission and does not read other keystrokes.
- It is distributed as source only; builds you make yourself are signed ad
  hoc.

Relevant reports include memory-safety problems in the single `unsafe`
CoreAudio call (`crates/audio/src/stream.rs`), crashes from crafted settings
files, and vulnerable or wrongly licensed dependencies (CI checks these with
`cargo-deny`).
