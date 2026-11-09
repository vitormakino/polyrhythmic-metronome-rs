# Manual verification

Checks that need a person, real hardware, or both. Each entry says who owns
it and its status: PENDING, PASS (date, machine) or FAIL (date, what
happened).

## Task 8: device changes

### Smoke test on the default output

Status: automated part PASS (2026-11-09, an Apple Silicon Mac, USB audio interface at
44100 Hz, reported latency 0.59 ms, 89600 frames, 9 events, 0 underruns).
Audible part PENDING (owner): the agent ran it but cannot hear the output.

```
cargo test -p polyrhythmic-metronome-audio --test stream -- --ignored --nocapture
```

Expected: about 2 s of clicks (accent on the "one", 4 against 3 at 120 BPM),
the test passes, `underruns 0` is printed.

### Following the default output device

Status: PENDING (owner). Needs wired headphones and a Bluetooth headset.

There is no app UI yet (Tasks 9 to 13), so use the smoke test above with a
longer run. Change the `2` in `while started.elapsed() <
Duration::from_secs(2)` in `crates/audio/tests/stream.rs` to `60`, locally and
without committing, then run the command above. During the 60 s:

1. Start with the built-in speakers (or the current default) as the output.
   Clicks are audible.
2. Plug in wired headphones. macOS makes them the default output. Expected:
   clicks continue in the headphones within about 0.5 s (the service checks
   4 times a second), with the 4-against-3 cycle intact: the next accent
   still falls on the bar's "one", no extra or missing click beyond a short
   gap at the switch.
3. Unplug the headphones. Expected: same, back on the speakers.
4. Connect a Bluetooth headset and select it (menu bar > Sound, or it
   becomes the default when it connects). Expected: same, in the headset.
   Bluetooth adds latency, so the gap at the switch can be longer, and the
   sample rate may differ (often 48000 or 16000 Hz in headset mode); the
   bar position must still continue.
5. Disconnect the headset. Expected: back on the speakers, no crash.
6. Optional: during a run, choose a device in Audio MIDI Setup with another
   sample rate (for example 96 kHz on an interface). Expected: same.

Pass: no crash or hang, and by ear the bar position is continuous across
each switch. If the test fails only on the underruns assertion, note the
printed count: a click lost during a switch is a finding, not a crash.
Revert the `60` afterwards.

Record the result here: date, devices used, any gap or glitch heard.

## Task 13: window modes

Status: PENDING (owner). The agent only checked that the release build
launches: the full window renders, with the output device name at the top
and the gear at the bottom right; the settings file is written about 1 s
after a change; started in mini mode, the window is 260 x 300, floats above
other windows, and the rounded card shows transparent corners. It could not
click or listen.

Settings live in `~/Library/Application Support/polyrhythmic-metronome/settings.toml`.
To start from defaults, move that file away first.

```
cargo run --release -p polyrhythmic-metronome-app
```

1. **Full window.** It opens at 900 x 700 (or the last saved size). Drag a
   corner inward. Expected: it stops at 420 x 640. The window has no title
   bar, only the traffic lights. Drag an empty spot to move it.
2. **Play.** Click PLAY. Expected: 4-against-3 clicks, and the topology and
   cards light in time.
3. **Visual offset.** Click the gear at the bottom right ("Configurações").
   Expected: a popover with "OFFSET VISUAL (MS)" and a slider (0 to 500 ms),
   starting at the latency CoreAudio reports for this device (often 0 to
   a few ms on wired outputs). Move it to about 200 ms. Expected: the flashes
   now trail the clicks visibly. Set it back to taste. Click outside to
   close the popover.
4. **Mini, always on top.** Open a PDF in Preview. Back in the metronome,
   click the small window icon next to TAP ("Fixar mini"). Expected: the
   window shrinks to the mini card (260 x 300 the first time), keeps playing,
   and stays above Preview when you click on Preview. The green zoom button
   is greyed out. Corners outside the rounded card are transparent. Drag
   the card to a corner of the screen. Resize it. Expected: it does not go
   below 220 x 260.
5. **Back to full.** Click the icon at the card's top right ("Expandir").
   Expected: the full window comes back at its previous size and place,
   at normal level (Preview can cover it again). Toggle mini again.
   Expected: the mini card comes back where you left it.
6. **Background.** Click the red close button (in either mode). Expected:
   the window disappears and the clicks keep going, steady, for at least a
   minute.
7. **Hidden window follows devices.** While the window is hidden and
   playing, plug in wired headphones. Expected: the clicks move to the
   headphones within about 0.5 s (the service ticks 4 times a second from
   a background thread even with no window shown).
8. **Reopening.** Click the Dock icon. Expected for now: the window does
   **not** come back. eframe 0.37 / winit 0.30 give no hook for the macOS
   "reopen" event, so a hidden window can only be shown again from the menu
   bar item that Task 14 adds. Until then, quit and relaunch.
9. **Quit and restore.** Edit the pattern (add a pulse, change beats, a
   sound, BPM 150), pick mini or full, then press ⌘Q (click the Dock icon
   first if no window is shown, so the app is active). Expected: the app
   quits. Relaunch. Expected: same pattern, BPM, mode and window places, and
   the visual offset you set for this device.
10. **No output.** Optional: with no output device at all, the footer shows
    "sem saída de áudio" with a red dot, and the app keeps running. Audio
    underruns, if any, show in the footer as "Falhas de áudio: n", also with
    a red dot.

Record the result here: date, macOS version, what differed.
