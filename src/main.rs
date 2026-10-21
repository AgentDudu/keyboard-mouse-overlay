//! kmoverlay — keyboard and mouse input overlay (Windows).
//!
//! Current surface: capture only. `--dump` prints normalised input to stdout so
//! capture correctness is provable before any window exists; `--selftest` proves
//! the hook path end to end without touching a focused window.

mod capture;
mod input;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod render;
mod scene;
#[cfg(windows)]
mod text;

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
  --overlay         show the transparent overlay window (Ctrl+Alt+Q quits)
  --show-injected   with --overlay, also show synthetic input (demo/screenshot aid)
  --moves           with --dump, also print mouse-move events (noisy)
  --duration <ms>   stop after this many milliseconds (default: run until killed)
  --help            show this help

Nothing captured here is written to disk.";

#[derive(Debug)]
struct Args {
    dump: bool,
    selftest: bool,
    overlay: bool,
    show_injected: bool,
    moves: bool,
    duration: Option<Duration>,
    help: bool,
}

impl Args {
    fn parse(mut iter: impl Iterator<Item = String>) -> Result<Args, String> {
        let mut args = Args {
            dump: false,
            selftest: false,
            overlay: false,
            show_injected: false,
            moves: false,
            duration: None,
            help: false,
        };
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--dump" => args.dump = true,
                "--selftest" => args.selftest = true,
                "--overlay" => args.overlay = true,
                "--show-injected" => args.show_injected = true,
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
        if !(args.dump || args.selftest || args.overlay) {
            return Err("pick a mode: --dump, --selftest or --overlay".into());
        }
        let modes = [args.dump, args.selftest, args.overlay]
            .iter()
            .filter(|on| **on)
            .count();
        if modes > 1 {
            return Err("--dump, --selftest and --overlay are mutually exclusive".into());
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
    } else if args.overlay {
        overlay_mode(rx, capture, &args)
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

/// The overlay: capture events drive a [`Scene`], the scene is rendered into a small panel
/// canvas, and that is blitted onto a full-screen transparent frame so click ripples can appear
/// wherever the click happened. Repainting is skipped while the scene is idle.
#[cfg(windows)]
fn overlay_mode(
    rx: std::sync::mpsc::Receiver<RawEvent>,
    _capture: capture::Capture,
    args: &Args,
) -> ExitCode {
    use input::{Action, Event};
    use overlay::{Frame, Overlay};
    use render::Layout;
    use scene::Scene;
    use text::Face;

    let face = match Face::new(text::FONT_BYTES) {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("overlay: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Must happen before the first window exists, or the sizes below are guesses.
    let aware = overlay::make_dpi_aware();
    let scale = overlay::system_scale();
    let layout = Layout::new(scale);
    let screen_w = Overlay::primary_width();
    let screen_h = Overlay::primary_height();

    let mut window = match Overlay::create(0, 0, screen_w, screen_h) {
        Ok(created) => created,
        Err(err) => {
            eprintln!("overlay: {err}");
            return ExitCode::FAILURE;
        }
    };

    let panel_h = layout.row_height() + layout.pad * 2;
    let anchor_x = 0i32;
    let anchor_y = (screen_h - panel_h as i32 - (56.0 * scale) as i32).max(0);

    let mut panel = Frame::new(screen_w as u32, panel_h);
    let mut canvas = Frame::new(screen_w as u32, screen_h as u32);
    println!(
        "overlay {screen_w}x{screen_h} canvas, panel {screen_w}x{panel_h} at y {anchor_y} | scale {scale:.2} window {:.2} aware_set {aware}",
        window.scale()
    );
    println!("clicks pass through: hold keys, click anywhere for a ripple.");
    println!("Ctrl+Alt+C toggles click-through, Ctrl+Alt+Q quits.");

    let timing = scene::Timing::default();
    let deadline = args.duration.map(|elapsed| Instant::now() + elapsed);
    let started = Instant::now();
    let mut stream = InputStream::new();
    let mut scene = if args.show_injected {
        Scene::showing_injected()
    } else {
        Scene::new()
    };
    let mut frames = 0u64;
    let mut busy = std::time::Duration::ZERO;
    let mut quit = false;
    let mut visible = true;

    while !quit && window.pump() {
        while let Ok(raw) = rx.try_recv() {
            let time = started.elapsed().as_secs_f64();
            if let Some(event) = stream.on_raw(raw) {
                scene.apply(&event, stream.modifiers(), time, &timing);
                let pressed = |vk: u32| matches!(event, Event::Key { vk: v, action: Action::Down, .. } if v == vk);
                let held = stream.modifiers();
                if held.ctrl && held.alt {
                    if pressed(0x43) {
                        let on = !window.click_through();
                        window.set_click_through(on);
                        println!("click-through {}", if on { "ON" } else { "OFF" });
                    }
                    quit = quit || pressed(0x51);
                }
            }
        }

        let now = started.elapsed().as_secs_f64();
        let active = scene.active(now, &timing);
        if !active && !visible {
            std::thread::sleep(Duration::from_millis(8));
            if deadline.is_some_and(|when| Instant::now() >= when) {
                break;
            }
            continue;
        }

        let work = Instant::now();
        render::render_panel(&mut panel, &scene, &face, &layout, now, &timing);
        canvas.clear();
        canvas.blit(&panel, anchor_x, anchor_y);
        render::draw_ripples(&mut canvas, &scene, &layout, now, &timing);
        if let Err(err) = window.present(&canvas) {
            eprintln!("overlay: present failed: {err}");
            return ExitCode::FAILURE;
        }
        busy += work.elapsed();
        frames += 1;
        visible = active;

        if quit || deadline.is_some_and(|when| Instant::now() >= when) {
            break;
        }
        std::thread::sleep(Duration::from_millis(if active { 6 } else { 40 }));
    }

    let elapsed = started.elapsed();
    println!(
        "{frames} frames presented in {:.1}s, {:.2} ms of drawing+presenting per frame",
        elapsed.as_secs_f64(),
        busy.as_secs_f64() / frames.max(1) as f64 * 1000.0
    );
    if quit {
        println!("quit by Ctrl+Alt+Q");
    }
    ExitCode::SUCCESS
}

#[cfg(not(windows))]
fn overlay_mode(
    _rx: std::sync::mpsc::Receiver<RawEvent>,
    _capture: capture::Capture,
    _args: &Args,
) -> ExitCode {
    eprintln!("overlay is Windows-only");
    ExitCode::from(4)
}
