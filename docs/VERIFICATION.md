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
   clicks continue in the headphones within about 0.75 s (the service checks
   4 times a second and rebuilds once it sees the new default output on two
   checks in a row; cpal's default-output unit may already play there in
   between), with the 4-against-3 cycle intact: the next accent
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
   starting at the part of the output latency the beat times do not already
   include, which is the stream latency only (often 0 ms on wired outputs;
   see "Default visual offset" under Task 15). Move it to about 200 ms. Expected: the flashes
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
   headphones within about 0.75 s (the service ticks 4 times a second from
   a background thread even with no window shown, and wants the new default
   on two ticks in a row).
8. **Reopening.** Click the Dock icon. Expected: the window does **not**
   come back. eframe 0.37 / winit 0.30 give no hook for the macOS "reopen"
   event, so a hidden window is shown again from the menu bar item ("Abrir
   janela" or "Mini", Task 14) or with ⌃⌥M.
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

## Task 14: menu bar & hotkeys

### Blinking the menu bar icon on the "one": measured, not shipped

Status: PASS (2026-11-09, an Apple Silicon Mac, output
a USB audio interface). Decision: **static icon**.

Rule from the plan: blink only if the icon updates cost less than 1% CPU.
Measured with a temporary build (not committed) that started playing at
300 BPM (4 against 3, so a "one" every 0.8 s) with the window hidden,
blinking for 100 ms on each "one", versus the same build with the blink off.
CPU time from `ps -o time=` over 10 s, after 4 s of warm-up, three
alternating runs each:

| run | app, blinking | app, static | ControlCenter, blinking | ControlCenter, static | WindowServer, blinking | WindowServer, static |
|-----|---------------|-------------|-------------------------|-----------------------|------------------------|----------------------|
| 1   | 1.9%          | 1.1%        | 0.2%                    | 0.0%                  | 1.8%                   | 2.0%                 |
| 2   | 3.4%          | 2.4%        | 0.4%                    | 0.1%                  | 4.5%                   | 4.2%                 |
| 3   | 2.6%          | 1.2%        | 0.6%                    | 0.1%                  | 3.0%                   | 2.1%                 |

The blink costs about 1.1% in the app (0.8 to 1.4 points: each change
re-encodes the icon in `tray-icon` and the extra wake-ups run `App::logic`)
plus about 0.3% in ControlCenter, which draws the menu bar extras on macOS
26. WindowServer is within noise. Over 1%, so the icon stays static.

Also seen: with the window hidden, eframe wakes the app at most every
100 ms, so a blink would land up to about 100 ms after the click (bar
intervals between blinks measured 0.73 to 0.88 s for 0.8 s bars).

### Menu bar item and global hotkeys

Status: PENDING (owner). The agent only checked that the release build
creates the menu bar item (an `NSStatusItem`, visible, 38 px wide in the
menu bar) and that nothing is printed on stderr (no hotkey refused). It
could not click the menu, press the hotkeys, or listen.

```
cargo run --release -p polyrhythmic-metronome-app
```

The hotkeys are ⌃⌥Space (play/stop), ⌃⌥T (tap) and ⌃⌥M (full ⇄ mini). If
another app already holds one, the terminal shows
`hotkey ... unavailable` and the other two still work.

1. **Menu bar item.** Expected: a ring-with-pendulum icon in the menu bar,
   drawn in the menu bar's colour (dark on a light menu bar, light on a
   dark one). Click it. Expected menu: "Play", "BPM +1", "BPM −1",
   "Abrir janela", "Mini", "Sair".
2. **Play from the menu.** Choose "Play". Expected: clicks start, and the
   first item now reads "Stop". Click PLAY/STOP in the window: the menu
   item follows ("Play" again after stopping).
3. **Background.** While playing, close the window (red button). Open
   Safari (or any app) and click into it so it has the focus.
4. **Hotkey play/stop.** With Safari focused, press ⌃⌥Space. Expected:
   the clicks stop. Press it again: they start. Safari
   keeps the focus and receives no space character.
5. **BPM from the menu.** With the window still hidden, choose "BPM +1"
   ten times. Expected: the tempo is audibly a little faster (130 BPM).
   "BPM −1" ten times brings it back. At 300 or 40 BPM nothing changes.
6. **Hotkey tap.** With Safari focused, press ⌃⌥T four times at a steady
   pace (for example in time with a 100 BPM song). Expected: the clicks
   follow the tapped tempo.
7. **Abrir janela.** Choose "Abrir janela". Expected: the full window
   comes back in front, focused, at its previous place, showing the
   current BPM (the one set in steps 5 and 6); while playing, no burst of
   old flashes, the lights just carry on in time.
8. **Mini.** Close the window again, then choose "Mini". Expected: the
   mini card comes back on top of Safari, without taking the focus from
   Safari. "Abrir janela" from mini switches to the full window.
9. **Hotkey mini.** With Safari focused, press ⌃⌥M. Expected: full ⇄
   mini each time; if the window was closed, it comes back (mini without
   focus, full with focus). A minimized window (yellow button) also comes
   back from the menu or ⌃⌥M.
10. **Sair.** Change the BPM, then choose "Sair" (also with the window
    hidden). Expected: the app quits (menu bar icon gone, no sound).
    Relaunch. Expected: the BPM and pattern you left.
11. **Hidden tick.** While the window is hidden and playing, plug in wired
    headphones. Expected: the sound moves within about 0.75 s (the service
    is now ticked on every ticker wake-up, 4 times a second, even when a
    wake-up comes a little early).

Record the result here: date, macOS version, what differed.

## Task 15: stress harness

`tools/stress` runs the real `AudioService` (7 against 11 at 300 BPM, both
wood blocks, playing from frame 0) while N busy threads at normal priority
burn the CPU and touch 8 MiB of memory each. The audio callback copies what it
rendered (first channel) into a preallocated ring (an optional "tap", off in
the app), the harness writes it to a WAV and then:

1. builds the reference signal from the `Timeline` grid and the `SampleBank`
   clips (summed, then the engine's soft clip), with no engine in between;
2. compares the whole recording with it sample by sample. The engine is
   deterministic, so a click that is early, late, missing or extra anywhere,
   including one that starts inside another click's sound, changes samples.
   Differences up to 1e-6 count as equal: the engine sums overlapping voices
   in slot order and the reference in hit order, which can differ by float
   rounding (about 1e-8) with three or more overlapping clicks;
3. also finds the sounds in both with the same detector (a sample at or above
   0.01 after at least 10 ms of quieter samples starts a sound) and reports
   the largest onset deviation in frames. This looks only at the leading
   click of a sound, so it is secondary; the report says how many grid clicks
   start inside another click's sound.

Pass: no sample differs by more than 1e-6, first-onset offset 0, the onsets
agree, no tap frames dropped and no stream rebuild (either would void the
recording). Underruns are printed; if any occurred the verdict says so and is
not "glitch-free".

```
cargo run -p stress --release -- --minutes 2 --load <cores> --out stress.wav
```

### 2-minute run, 12 load threads

Status: PASS (2026-11-09, an Apple Silicon Mac, 12 cores, output a
USB audio interface at 44100 Hz; nothing else deliberately running).
The agent ran it (audible on the interface).

```
2 min, 12 load threads, 7:11 at 300 BPM on a USB audio interface at 44100 Hz
wrote stress.wav (5292032 frames)
clicks on the grid:       1544 (600 inside another click's sound)
sounds expected/detected: 943 / 943 (matched 943)
first-onset offset:       0 frames (must be 0)
max onset deviation:      0 frames
samples vs reference:     5292032 compared, 0 differ by more than 1e-6, max difference 0e0
underruns:                0
stream rebuilds:          0
tap frames dropped:       0
PASS
```

The maximum difference was exactly 0: the recording is bit-identical to the
reference. Whether the run sounded even was not judged (the agent cannot
hear it).

Finding (fixed in the final review wave, see below): in one of three runs
with this build, the stream was rebuilt once
during the run (`stream rebuilds: 1`, verdict INVALID, 5290496 frames, onset
deviation 1024 frames afterwards because the new engine's tap restarts).
Underruns were 0. The cause was not identified (the harness does not record
which condition in `AudioService::tick` fired). The harness reports such a run
as INVALID rather than hiding it. Worth watching in the 10-minute run.

### Final review fix wave: rebuild triggers confirmed and recorded

The rebuild above had 0 underruns and lost only 1536 frames (about 35 ms):
the audio was healthy and the rebuild itself made the gap. Any single
odd sample could trigger one. Now (`crates/audio/src/watch.rs`, unit tested):

- a missing default output (including a failed UID query) rebuilds only
  after 1 s of ticks; a different default output must be seen on two ticks
  in a row (a flap A -> B -> A does nothing);
- a wall clock that jumped (sleep, NTP, the user setting the time) counts as
  sleep only if the data callback was not called since the previous tick;
- the callback counter (no callback for over 1 s) is the only stall
  detector: the data callback no longer flags a failure from CoreAudio's
  host-time gap (a bogus `mHostTime` could fake one); late callbacks still
  count as underruns;
- every rebuild decision is recorded with its reason (`slept`, `stalled`,
  `stream error <kind>`, `device changed <from> -> <to>`): the app prints it
  on stderr and the harness prints it as it happens plus per-reason counts.
  Any rebuild decision voids a harness run.

Debug builds of the app and the harness also run the data callback under
`assert_no_alloc` with `AllocDisabler` as the global allocator: an
allocation in the callback aborts (checked by adding one on purpose: the
debug harness aborted with "memory allocation of 1 bytes failed"; reverted).
A 15 s debug harness run (2 load threads) passed sample-exact, and the debug
app ran 12 s with the stream open without aborting. Release builds are
unchanged.

Three 2-minute runs, 12 load threads (`sysctl -n hw.ncpu` = 12), 2026-11-09,
an Apple Silicon Mac (USB audio interface at 44100 Hz):

```
cargo run -p stress --release -- --minutes 2 --load <cores> --out stress.wav
```

| run | frames  | sounds (expected/detected/matched) | samples differing | max deviation | underruns | rebuilds | rebuild reasons | dropped | verdict |
|-----|---------|------------------------------------|-------------------|---------------|-----------|----------|-----------------|---------|---------|
| 1   | 5292544 | 943 / 943 / 943                    | 0                 | 0 frames      | 0         | 0        | none            | 0       | PASS    |
| 2   | 5293056 | 944 / 944 / 944                    | 0                 | 0 frames      | 0         | 0        | none            | 0       | PASS    |
| 3   | 5293056 | 944 / 944 / 944                    | 0                 | 0 frames      | 0         | 0        | none            | 0       | PASS    |

Max sample difference 0 and first-onset offset 0 in all three. No rebuild
was decided in any run, so the earlier one did not recur; its cause stays
unconfirmed (the debounce does not log the samples it ignores). If one
happens in the 10-minute run, the printed reason says which check fired.

### Default visual offset

cpal 0.18.2 (`host/coreaudio/macos/device.rs`, output render callback) sets
`playback = callback + (device buffer frames + kAudioDevicePropertyLatency
+ kAudioDevicePropertySafetyOffset)`, and the engine stamps beat events
with that time. Measured here: 538 frames ahead for a 512-frame buffer,
i.e. device latency + safety offset = 26 frames (0.59 ms at 44.1 kHz). The
old default visual offset added device + safety + stream latency again on
top. It is now only `kAudioStreamPropertyLatency`, the part cpal leaves
out (`OutputLatency::uncompensated`); on the USB interface tested it is 0 ms (stream
latency 0, device + safety 0.59 ms, from the device smoke test). Offsets
the user saved per device are kept as they are.

### 10-minute run with a browser playing video

Status: PENDING (owner). Needs a person to start the video.

1. Open a browser and play a video (1080p or higher, full screen on a second
   display if there is one). Leave it playing.
2. Run (replace 12 with the number of cores, `sysctl -n hw.ncpu`):

   ```
   cargo run -p stress --release -- --minutes 10 --load <cores> --out stress.wav
   ```
3. Pass: `PASS`, max deviation 0 frames, same sound counts. Note the underruns
   and whether you heard any glitch. Listening to `stress.wav` is optional;
   it is the callback's own output.

Record the result here: date, machine, numbers.

### Unplug every output, sleep and wake

Status: PENDING (owner). The agent cannot unplug hardware or sleep the Mac.

With the app playing (`cargo run --release -p polyrhythmic-metronome-app`):

1. Unplug or disconnect every audio output, including the built-in one if the
   Mac allows it (Bluetooth off, USB interface out, headphones out; on a
   laptop the built-in speakers stay, so this part needs a Mac mini or an
   external-only setup). Expected: the footer shows "sem saída de áudio" with
   a red dot within about a second (at once if cpal reports the device gone;
   a default output that is merely missing for a moment is ignored for 1 s)
   and the app keeps running. The terminal shows `audio stream rebuilt:`
   with the reason; hovering the footer label shows the open error, if any.
2. Plug an output back in. Expected: playing resumes within about a second
   with the bar position continuous; the footer shows the device name.
3. While playing, put the Mac to sleep (Apple menu > Sleep) and wake it after
   a few seconds. Expected: the app is still there, the stream is rebuilt
   within about a second of waking and the clicks resume without a burst of
   old flashes; no crash.

Record the result here: date, macOS version, outputs used, what differed.
