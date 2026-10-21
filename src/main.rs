//! kmoverlay — keyboard and mouse input overlay (Windows).
//!
//! Current surface: capture only. `--dump` prints normalised input to stdout so
//! capture correctness is provable before any window exists; `--selftest` proves
//! the hook path end to end without touching a focused window.

mod capture;
mod input;

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use input::{Event, InputStream, RawEvent};

#[cfg(test)]
mod tests {
    use super::Args;

    fn parse(args: &[&str]) -> Result<Args, String> {
        Args::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_mode_is_rejected_with_a_usable_message() {
        let err = parse(&[]).unwrap_err();
        assert!(err.contains("--dump"), "{err}");
    }

    #[test]
    fn help_does_not_need_a_mode() {
        assert!(parse(&["--help"]).map(|a| a.help).unwrap());
    }

    #[test]
    fn modes_are_exclusive_and_duration_needs_a_number() {
        assert!(parse(&["--dump", "--selftest"]).is_err());
        assert!(parse(&["--dump", "--duration"]).is_err());
        assert!(parse(&["--dump", "--duration", "abc"]).is_err());
        assert_eq!(
            parse(&["--dump", "--duration", "250"])
                .unwrap()
                .duration
                .unwrap()
                .as_millis(),
            250
        );
        assert!(parse(&["--dump", "--nope"]).is_err());
    }
}

const USAGE: &str = "\
kmoverlay — keyboard and mouse input overlay

  --dump            print normalised input events to stdout
  --selftest        inject tagged synthetic keys and assert the hook path works
  --moves           with --dump, also print mouse-move events (noisy)
  --duration <ms>   stop after this many milliseconds (default: run until killed)
  --help            show this help

Nothing captured here is written to disk.";

#[derive(Debug)]
struct Args {
    dump: bool,
    selftest: bool,
    moves: bool,
    duration: Option<Duration>,
    help: bool,
}

impl Args {
    fn parse(mut iter: impl Iterator<Item = String>) -> Result<Args, String> {
        let mut args = Args {
            dump: false,
            selftest: false,
            moves: false,
            duration: None,
            help: false,
        };
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--dump" => args.dump = true,
                "--selftest" => args.selftest = true,
                "--moves" => args.moves = true,
                "--help" | "-h" => args.help = true,
                "--duration" => {
                    let value = iter.next().ok_or("--duration needs a millisecond value")?;
                    let ms = value
                        .parse::<u64>()
                        .map_err(|_| format!("--duration: not a number: {value}"))?;
                    args.duration = Some(Duration::from_millis(ms));
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        if args.help {
            return Ok(args);
        }
        if !(args.dump || args.selftest) {
            return Err("pick a mode: --dump or --selftest".into());
        }
        if args.dump && args.selftest {
            return Err("--dump and --selftest are mutually exclusive".into());
        }
        Ok(args)
    }
}

fn main() -> ExitCode {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("{err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let (rx, capture) = match capture::start() {
        Ok(started) => started,
        Err(err) => {
            eprintln!("capture unavailable: {err}");
            return ExitCode::from(3);
        }
    };

    if args.selftest {
        selftest(rx, capture)
    } else {
        dump(rx, capture, &args)
    }
}

fn dump(
    rx: std::sync::mpsc::Receiver<RawEvent>,
    capture: capture::Capture,
    args: &Args,
) -> ExitCode {
    let mut stream = InputStream::new();
    let started = Instant::now();
    let deadline = args.duration.map(|d| started + d);
    let stdout = io::stdout();
    let mut out = stdout.lock();

    // shortcut: no elevation probe yet; this is the honest static caveat until the
    // integrity-level check lands with the overlay UI.
    println!("capturing input — Ctrl+C to stop");
    println!(
        "note: input destined for elevated windows is invisible unless this process runs elevated"
    );
    let _ = out.flush();

    loop {
        let event = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok(event) => event,
                    Err(_) => break,
                }
            }
            None => match rx.recv() {
                Ok(event) => event,
                Err(_) => break,
            },
        };

        let Some(event) = stream.on_raw(event) else {
            continue;
        };
        if matches!(event, Event::Move { .. }) && !args.moves {
            continue;
        }

        let modifiers = stream.modifiers();
        let t = started.elapsed().as_secs_f64();
        if modifiers.is_empty() {
            let _ = writeln!(out, "{t:>9.3} {event}");
        } else {
            let _ = writeln!(out, "{t:>9.3} {event}   [{modifiers}]");
        }
    }

    let dropped = capture::DROPPED.load(std::sync::atomic::Ordering::Relaxed);
    println!("stopped. dropped events: {dropped}");
    capture.stop();
    ExitCode::SUCCESS
}

/// Prove: hooks installed → tagged injected keys arrive → autorepeat-free
/// down/up pairs → hooks removable.
///
/// The synthetic keys carry [`capture::TEST_TAG`] and are swallowed by our own
/// hook, so no window under the user's focus ever sees F13/F14.
fn selftest(rx: std::sync::mpsc::Receiver<RawEvent>, capture: capture::Capture) -> ExitCode {
    capture::set_swallow_tagged(true);
    let mut stream = InputStream::new();
    let mut seen: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for (vk, name) in [(0x7Cu16, "F13"), (0x7Du16, "F14")] {
        if !capture::inject_key(vk, false) {
            failures.push(format!("SendInput rejected {name} down"));
        }
        std::thread::sleep(Duration::from_millis(20));
        if !capture::inject_key(vk, true) {
            failures.push(format!("SendInput rejected {name} up"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Collect for a beat so late-arriving hook callbacks are still counted.
    let collect = Instant::now() + Duration::from_millis(300);
    while Instant::now() < collect {
        if let Ok(raw) = rx.recv_timeout(Duration::from_millis(50)) {
            if !(raw.tagged && raw.injected) {
                continue; // the user's own typing is not our business here
            }
            if let Some(event) = stream.on_raw(raw) {
                seen.push(event.to_string());
            }
        }
    }

    capture::set_swallow_tagged(false);
    capture.stop();

    let lines = seen;
    for line in &lines {
        println!("observed: {line}");
    }

    let expected = ["F13", "F13", "F14", "F14"];
    let got_labels: Vec<&str> = lines
        .iter()
        .map(|l| l.split_whitespace().nth(2).unwrap_or("?"))
        .collect();
    if got_labels != expected {
        failures.push(format!("expected {expected:?} down/up, got {got_labels:?}"));
    }
    if lines.len() != 4 {
        failures.push(format!("expected 4 events, saw {}", lines.len()));
    }
    if lines.iter().any(|l| !l.contains("[injected]")) {
        failures.push("an event was not flagged injected".into());
    }
    if lines.iter().any(|l| l.contains("Repeat")) {
        failures.push("autorepeat dedupe mislabelled an event".into());
    }

    if failures.is_empty() {
        println!(
            "selftest PASS: hook path delivered {} tagged events and unhooked cleanly",
            lines.len()
        );
        ExitCode::SUCCESS
    } else {
        for failure in failures {
            eprintln!("selftest FAIL: {failure}");
        }
        ExitCode::FAILURE
    }
}
