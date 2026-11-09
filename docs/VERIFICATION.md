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
