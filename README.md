# keyboard-mouse-overlay

Always-on-top, click-through overlay that shows live keyboard and mouse input on Windows.

Rust, raw Win32 (`WS_EX_LAYERED` + `UpdateLayeredWindow`), CPU-rendered frames with `ab_glyph`.
Roadmap and the phase-by-phase plan are in [PLAN.md](PLAN.md).

## Run

Requires the MSVC toolchain and the Windows SDK (`vcbuildtools` workload).

```
cargo run -- --dump              # print global input as NDJSON
cargo run -- --selftest          # inject tagged input and check the hook sees it
cargo run -- --overlay           # the transparent overlay (needs an interactive desktop)
cargo run -- --overlay --moves   # also forward mouse-move events
```

```
usage: kmoverlay [--dump|--selftest] [--duration SECONDS] [--quiet] [--moves] [--overlay]

Global input capture for the overlay. Ctrl+Alt+Q quits everywhere, Ctrl+Alt+C toggles
click-through while the overlay is up.
  --duration SECONDS  stop after this much elapsed time
  --dump              print one NDJSON event per line and a summary at exit (default)
  --selftest          inject tagged F13/F14 input, then report what the hook saw
  --quiet             run silently; the exit code carries the result
  --moves             also report mouse-move events (they arrive at the coalesced rate)
  --overlay           show the transparent overlay window (Ctrl+Alt+Q quits)
  --show-injected     with --overlay, also show synthetic input (capture aid)
```

No arguments needs no elevation; the low-level hook follows the UIPI rule explained in
[PLAN.md](PLAN.md). `--selftest` injects tagged input and swallows only its own events, so
F13/F14 never reach whatever has focus.

**Status:** phases 1-2 run on this machine (input capture, 60 Hz transparent click-through
window), and phase 3 draws keycaps, fading history, modifier states, a mouse indicator and
click ripples (IBM Plex Mono embedded, `ab_glyph` on the CPU):

![phase 3 panel](docs/phase3-panel.png)
