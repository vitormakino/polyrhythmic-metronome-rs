//! Stress harness: runs the real `AudioService` (7 against 11 at 300 BPM)
//! while busy threads load the CPU, records what the audio callback rendered
//! through its output tap, writes a WAV and checks every click against the
//! `Timeline` grid.
//!
//! ```text
//! cargo run -p stress --release -- --minutes 10 --load 12
//! ```
//!
//! Exit status 0 only if the recording equals the offline reference render
//! sample for sample (so every click, merged or not, is on the grid), no
//! frame of the recording was lost, and the stream was not rebuilt.

mod analysis;
mod onset;
mod wav;

use std::fs::File;
use std::io::BufWriter;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use pm_audio::{AudioService, EngineInit};
use pm_core::{Bpm, MusicalPosition};

// Debug builds abort on an allocation inside the audio callback, as the app.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const BPM: i32 = 300;
/// Output tap: about 21 s at 48 kHz, so a stalled main thread loses nothing.
const TAP_SAMPLES: usize = 1 << 20;

struct Args {
    minutes: f64,
    load: usize,
    out: String,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        minutes: 10.0,
        load: thread::available_parallelism().map_or(4, |n| n.get()),
        out: "stress.wav".to_owned(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--minutes" => {
                args.minutes = value("--minutes")?
                    .parse()
                    .map_err(|e| format!("--minutes: {e}"))?
            }
            "--load" => {
                args.load = value("--load")?
                    .parse()
                    .map_err(|e| format!("--load: {e}"))?
            }
            "--out" => args.out = value("--out")?,
            "-h" | "--help" => {
                return Err("usage: stress [--minutes M] [--load N] [--out FILE.wav]".to_owned());
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if !args.minutes.is_finite() || args.minutes <= 0.0 {
        return Err("--minutes must be positive".to_owned());
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &Args) -> Result<bool, String> {
    let pattern = analysis::seven_eleven();
    let bpm = Bpm::new(BPM);
    let init = EngineInit {
        pattern: pattern.clone(),
        bpm,
        // Plays from the first frame, so engine frame N is recording frame N.
        resume: Some(MusicalPosition::start_of(0)),
    };

    let stop = Arc::new(AtomicBool::new(false));
    let load: Vec<_> = (0..args.load)
        .map(|i| {
            let stop = Arc::clone(&stop);
            thread::spawn(move || busy(i as u64, &stop))
        })
        .collect();

    let mut service = AudioService::start_with_tap(init, TAP_SAMPLES).map_err(|e| e.to_string())?;
    let device = service.device_info().ok_or("no audio output device")?;
    let sr = device.sample_rate;
    println!(
        "{} min, {} load threads, 7:11 at {BPM} BPM on \"{}\" at {sr} Hz",
        args.minutes, args.load, device.name
    );

    let total = (args.minutes * 60.0 * f64::from(sr)) as usize;
    let mut samples: Vec<f32> = Vec::with_capacity(total + TAP_SAMPLES);
    let started = Instant::now();
    let run_for = Duration::from_secs_f64(args.minutes * 60.0);
    let mut next_tick = started;
    let mut next_report = started + Duration::from_secs(30);
    let mut decided = 0;
    while started.elapsed() < run_for {
        let now = Instant::now();
        if now >= next_tick {
            service.tick();
            next_tick = now + Duration::from_millis(250);
            if service.rebuild_reasons().total() != decided {
                decided = service.rebuild_reasons().total();
                let reason = service
                    .last_rebuild_reason()
                    .map_or_else(|| "?".to_owned(), ToString::to_string);
                println!(
                    "  {:>5.1} s, rebuild: {reason}",
                    started.elapsed().as_secs_f64()
                );
            }
        }
        if let Some(tap) = service.tap() {
            while let Ok(s) = tap.samples.pop() {
                samples.push(s);
            }
        }
        if now >= next_report {
            let u = service.handle().underruns.load(Ordering::Relaxed);
            println!(
                "  {:>5.0} s, {} frames, underruns {u}",
                started.elapsed().as_secs_f64(),
                samples.len()
            );
            next_report += Duration::from_secs(30);
        }
        thread::sleep(Duration::from_millis(20));
    }
    let underruns = service.handle().underruns.load(Ordering::Relaxed);
    let rebuilds = service.rebuilds();
    let reasons = service.rebuild_reasons();
    let last_reason = service.last_rebuild_reason().cloned();
    let last_error = service.last_error().map(ToString::to_string);
    let dropped = service.tap().map_or(u64::MAX, |t| t.dropped());
    if let Some(tap) = service.tap() {
        while let Ok(s) = tap.samples.pop() {
            samples.push(s);
        }
    }
    drop(service);
    stop.store(true, Ordering::Relaxed);
    for t in load {
        let _ = t.join();
    }

    let file = File::create(&args.out).map_err(|e| format!("{}: {e}", args.out))?;
    wav::write_mono_f32(&mut BufWriter::new(file), sr, &samples).map_err(|e| e.to_string())?;

    let report = analysis::analyse(&samples, sr, bpm, &pattern);
    println!("wrote {} ({} frames)", args.out, samples.len());
    println!(
        "clicks on the grid:       {} ({} inside another click's sound)",
        report.hits, report.merged_hits
    );
    println!(
        "sounds expected/detected: {} / {} (matched {})",
        report.expected, report.detected, report.matched
    );
    println!(
        "first-onset offset:       {} frames (must be 0)",
        report.shift
    );
    println!("max onset deviation:      {} frames", report.max_deviation);
    println!(
        "samples vs reference:     {} compared, {} differ by more than {:e}, max difference {:e}",
        report.samples,
        report.differing_samples,
        analysis::TOLERANCE,
        report.max_sample_diff
    );
    println!("underruns:                {underruns}");
    println!("stream rebuilds:          {rebuilds}");
    println!(
        "rebuild reasons:          slept {}, stalled {}, stream error {}, device changed {}",
        reasons.slept, reasons.stalled, reasons.stream_error, reasons.device_changed
    );
    if let Some(reason) = last_reason {
        println!("last rebuild reason:      {reason}");
    }
    if let Some(e) = last_error {
        println!("last open error:          {e}");
    }
    println!("tap frames dropped:       {dropped}");
    // Dropped tap frames or a rebuild void the recording.
    let valid = rebuilds == 0 && reasons.total() == 0 && dropped == 0;
    let verdict = if !valid {
        "INVALID (recording is not contiguous)"
    } else if !report.is_exact() {
        "FAIL"
    } else if underruns > 0 {
        "PASS (output exact) BUT UNDERRUNS OCCURRED: not glitch-free"
    } else {
        "PASS"
    };
    println!("{verdict}");
    Ok(valid && report.is_exact())
}

/// Burns a core at normal priority until `stop` is set.
fn busy(seed: u64, stop: &AtomicBool) {
    let mut x = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut buf = vec![0u8; 8 << 20]; // also pressure the caches and memory
    while !stop.load(Ordering::Relaxed) {
        for i in 0..100_000usize {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let idx = (x >> 40) as usize % buf.len();
            buf[idx] = buf[idx].wrapping_add(i as u8);
        }
        std::hint::black_box((&buf, x));
    }
}
