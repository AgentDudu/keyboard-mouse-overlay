# Keyboard & Mouse Overlay — Implementation Plan

Status: **decisions locked** (2026-10-21) — see section 9. Rest of the document is the plan
those decisions imply.

---

## 1. What we are building (decided)

A desktop app that watches real keyboard + mouse input system-wide and paints it on a
transparent, always-on-top, **click-through** overlay: keycaps for pressed keys, modifier
state, mouse button + wheel indicators, with fade-out after release. Think Keyviz /
Keycues / NohBoard. Read-only: no suppression, no injection. Nothing typed is written to disk.

Decided out of scope: screen recording, key remapping / macro execution, input replay, OBS
plugin, and the full-screen "ghost layer that follows the cursor" variant (a fixed keycap
panel is the v1 shape — the ghost layer needs a full-screen layered surface and a heavier
render path for no extra information).

## 2. Requirements draft

Functional
- F1 Capture keyboard press/release, modifier state, OS key autorepeat (distinguish real
  repeats from re-presses).
- F2 Capture mouse button up/down, wheel ticks, cursor position.
- F3 Render overlay: keycap row/grid, per-button mouse glyph, click ripple, timed fade.
- F4 Overlay never steals focus, never blocks clicks to the app underneath.
- F5 User can move/anchor the overlay, pick theme/size/opacity, quit with a hotkey.
- F6 Config persisted between runs.

Non-functional
- N1 Idle CPU ≤ 1% single core, ≤ 80 MB RSS while showing a few keycaps.
- N2 Input → pixel ≤ 30 ms p95, measured not guessed.
- N3 Per-monitor DPI correct on mixed-scale monitors; declare DPI awareness in a manifest.
- N4 Portable single `.exe`, no install, no network, no telemetry.
- N5 Zero keystroke persistence. Only the currently-displayed glyphs live in memory.

## 3. Architecture

```
Win32 low-level hooks (WH_KEYBOARD_LL / WH_MOUSE_LL)   [capture thread, own message loop]
        │  copy Event {kind, vk, is_e0/e1, transitions, wdelta, pt, time}
        ▼
lock-free queue (or crossbeam channel, bounded, drop-oldest)
        ▼
InputState  ── press/release, autorepeat dedupe, modifier mask, expiry timers
        ▼                                    ▲
   RenderModel (keycaps, ripples, alpha)     │ Config (themes, layout, bounds)
        ▼
Overlay surface (one painter: draw → present), plus tray/hotkey/settings path
```

Invariants
- The hook callback only copies into the queue and calls `CallNextHookEx`. A slow low-level
  hook is silently removed by Windows, so no allocation-heavy work, no logging, no locks in
  that callback.
- The overlay window is never activated. All hotkeys come from the hook stream, not from
  window focus.
- One render path. If we add a second backend later it hides behind one `Overlay` trait, and
  only if a real need appears.

## 4. Technology options (evidence, not vibes)

Input capture

| Option | State | Notes |
| --- | --- | --- |
| `windows` crate, direct `SetWindowsHookExW(WH_KEYBOARD_LL/WH_MOUSE_LL)` | recommended | ~150 lines, full control of `KBDLLHOOKSTRUCT` flags (`LLKHF_ALTDOWN`, extended-key bit), no third-party input dependency. |
| `rdev` 0.5.3 | risk | Last publish 2023-06-26 ("over 2 years ago"); maintenance question open as [Narsil/rdev#151](https://github.com/Narsil/rdev/issues/151). Popular (833k downloads) but stale. |
| `inputs-rs` (`inputs` 0.1.1) | immature | Published 2026-08-24, 171 total downloads. Claims suppression + cross-platform. Watch, do not depend. |
| `willhook-rs`, `raw-input` | niche | 12 and 8 stars. Too young to trust with a raw input path. |

Overlay window
- Need: transparent, borderless, topmost, absent from taskbar/Alt-Tab, hit-test transparent.
- winit exposes transparency/topmost but not the extra ex-styles we want; the documented
  route is still `SetWindowLongPtrW(GWL_EXSTYLE, WS_EX_LAYERED | WS_EX_TRANSPARENT |
  WS_EX_TOOLWINDOW)` after window creation ([winit#2059](https://github.com/rust-windowing/winit/issues/2059),
  [winit#1434](https://github.com/rust-windowing/winit/issues/1434)).
- egui/eframe can do transparent + always-on-top
  ([egui discussion #3898](https://github.com/emilk/egui/discussions/3898)), but input
  passthrough is OS-specific and needs the same ex-style hack
  ([egui#1677](https://github.com/emilk/egui/issues/1677)); users also report black-instead-of-transparent
  windows depending on backend ([egui discussion #4446](https://github.com/emilk/egui/discussions/4446))
  and winit transparency regressions ([winit#2502](https://github.com/rust-windowing/winit/issues/2502)).

Rendering

| Option | Cost | Fit |
| --- | --- | --- |
| CPU paint into a `WS_EX_LAYERED` window (`UpdateLayeredWindow`) via `softbuffer` + `tiny-skia` + `ab_glyph` | ~300–500 lines total | Best fit: no GPU/driver variance, per-pixel alpha is native, cheap at 60 Hz for a small surface, works over most content. |
| Direct2D/DirectComposition via `windows` crate | more Win32 code | Best for a full-screen overlay over hardware-accelerated content; more platform code to own. |
| `eframe`/`egui` | least UI code | Fastest settings UI, but transparency + passthrough are the exact two things it is weakest at. Use only if the settings UI gets big. |
| `wgpu`/`pixels` | heaviest | Overkill for keycaps. |

**Chosen:** `windows`-crate hooks + `softbuffer`/`tiny-skia`/`ab_glyph` overlay, with the
Phase 2 spike proving transparency and passthrough before any keycap art exists.

Capture seam (decided): one `Capture` trait — `start(sink)`, `stop()`, event type is our own
normalised enum, never a Win32 struct. Windows-only implementation in v1; the trait exists so
a Linux/macOS backend later cannot leak `KBDLLHOOKSTRUCT` into the renderer. No trait for the
renderer — one paint path only.

## 5. Phases

Each phase ends runnable and merged to `develop` via PR. Exit criteria are testable.

**Phase 0 — skeleton (0.5 day)**
`cargo init` binary crate, `clippy`/`fmt` config, `windows` + `windows-core` deps pinned,
CI job (`fmt --check`, `clippy -D warnings`, `test`, `build --release`), README.
Exit: `cargo run` opens a window. CI green.

**Phase 1 — capture (1–2 days)**
Hook install/teardown on a dedicated thread with its own message loop; VK → logical key map
(including numpad, arrows, media keys); modifier mask from hook state, not from
`GetKeyState` (focus-dependent, lies); autorepeat detection; wheel delta normalisation;
bounded queue.
Add `--dump` mode: print normalised events to stdout, no window. This is the cheap way to
validate capture without any GUI, and it stays as a diagnostic forever.
Exit: `--dump` shows correct names/modifiers/wheel for a manual 2-minute torture test
(held key autorepeat, Ctrl+Shift combos, IME editor, game window); unit tests for the VK map
and the press/release/autorepeat state machine.

Done, with three findings for later steps:
- `MSLLHOOKSTRUCT` exposes no injected flag (only XBUTTON bits), so synthetic **mouse** input
  is only distinguishable by a `dwExtraInfo` convention. Keyboard keeps `LLKHF_INJECTED`.
- The hook callback holds a bounded `SyncSender` handle in a process-wide `OnceLock` and
  `try_send`s: drop-newest on a stalled consumer, counter at `capture::DROPPED`. Upgrade to
  drop-oldest only if that counter ever climbs.
- `--selftest` (tagged + swallowed F13/F14 via `SendInput`) is the proof the hook path works,
  because it needs a desktop session and cannot run in CI. It already caught one real bug:
  the swallow branch returned before recording, so its own probe input vanished.

**Phase 2 — overlay window spike (1–2 days)** ← highest risk, do it early, before Phase 3 art
Transparent topmost click-through window, hidden from taskbar/Alt-Tab, DPI-aware manifest,
bounds follow a configurable anchor/monitor, quit hotkey + escape hatch, drag mode toggled by
hotkey (because passthrough means no dragging by default).
Exit: clicks land on a window underneath; overlay still paints over a video and over a
fullscreen-borderless game; no Alt-Tab flicker; `cargo run` on 100%/150% mixed DPI.
Kill criterion: if `softbuffer` + layered window cannot stay transparent here, switch this
phase's render backend to DirectComposition instead of fighting it.

Built on this branch (`src/overlay.rs`): raw `WS_EX_LAYERED | WS_EX_TRANSPARENT |
WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST` painted with
`UpdateLayeredWindow(ULW_ALPHA, AC_SRC_ALPHA)` from a `CreateDIBSection` backed by a
premultiplied-BGRA `Frame`. `softbuffer` proved unnecessary — a DIB section is the same
CPU-buffer model with one less dependency, so the DirectComposition fallback stays unspent.
- 60.4 fps held for 6 s (363 frames) at 480×96 on 1920×1080 @100 %, `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` before the window exists; 304 KB exe; clean exit.
- Ctrl+Alt+Q quits, Ctrl+Alt+C toggles passthrough (the future drag mode), `pump()` reports a dead window instead of spinning.
- Still needs a human: paint over a video and a fullscreen-borderless game, 100 %/150 % mixed DPI, no Alt-Tab flicker. Run `cargo run -- --overlay`.

**Phase 3 — visuals (1–2 days)**
Keycap widget (glyph + label + shadow), modifier row, mouse glyph with per-button state,
click ripple, exponential fade, layout (bottom-centre and corner presets), font loading and
fallback for the media/symbol keys.
Exit: layout tests plus a screenshot from a real desktop; idle CPU inside N1.

**Status (phase 3 visuals, in progress — 2026-10-21):** `ab_glyph` + embedded IBM Plex Mono
Bold (`assets/OFL.txt`, 134 KB). Pure modules: `scene.rs` (what is visible and at what alpha),
`text.rs` (glyphs onto a CPU buffer), `render.rs` (layout: panel, caps, mouse indicator,
ripple). `overlay.rs` gained `blit`, so only the strip and the rings are painted onto a
full-screen canvas, and `main.rs` skips repainting while the scene is idle. `--show-injected`
lets a scripted `SendInput` sequence drive the overlay for a capture. Measured on this desktop
(1920x1080 @100 %, release):

```
idle 3.0 s, nothing pressed        -> 1 frame presented            (idle skip works)
active run                         -> 167 frames, 4.86 ms draw+present per frame
cargo test 45 passed               (15 scene, 9 render, 3 text)
screenshot diff vs no-overlay shot -> panel x 0..932 y 938..1024, caps at x 14-100 /
  109-210 / 219-289 (Ctrl Shift Alt lit), idle Win cap with legible glyph strokes, F13 lit,
  A and F15 mid-fade, mouse indicator x 802-872, caps band y 952..1010
                                       -> docs/phase3-panel.png
```

Layout anchor is bottom-centre: the strip is only as wide as its content and is blitted centred
per frame, so the row slides sideways as caps come and go (keyviz behaviour; corner presets are a
phase 4 setting). Labels are centred by measured ink, not font ascent — `Face::ink` reads outline
bounds and `baseline_for` centres that box in the keycap.

```
caps x 507..1411 of 1920   -> left margin 507, right margin 508 (centred)
glyph ink rows 973..987    -> centre 980 vs cap box centre 981
```

Still open for phase 3: a screenshot on a display actually set to 150 % (the `--scale` flag
proves the geometry, not the OS DPI path), corner/corner-preset anchoring (phase 4 config), and
the optional hold-to-confirm ring, which belongs with the phase 5 macros it would serve.

Added with the polish pass — palettes are data (`render::Theme`, `--theme dark|light`) and the
size factor is a flag (`--scale`), plus `tools/capture-gif.ps1`, which drives harmless synthetic
F-key input behind a plain backdrop and encodes the demos in the README (ffmpeg, 36 frames):

```
scale 1.0, caps band    -> y 34..79  = 46 px  = Layout::cap_height(1.0)
scale 1.0, panel height -> y 14..99  = 86 px  = mouse_height(58) + 2 * pad(14)
scale 1.5, caps band    -> y 17..85  = 69 px  = 46 * 1.5
scale 1.5, strip centre -> 720 of 1440 (centred in the capture)
before the first press  -> 0 pixels above the backdrop, so idle really hides
theme test              -> every palette must keep text contrast against its own cap
```

**Phase 4 — config & ergonomics (1 day)**
`config.toml` in the platform config dir (theme, scale, opacity, anchor, monitor, fade ms,
excluded keys), theme presets, reload-on-save, tray icon with quit/toggle, first-run
behaviour when config is missing/corrupt (start with defaults, never crash).
Exit: kill the process, relaunch, same look. Corrupt config test.

**Phase 5 — ship (1 day)**
Release profile (`lto`, `strip`, `panic=abort`), version via `CARGO_PKG_VERSION`, portable zip
artifact from CI, optional Inno Setup/MSIX later, local error log with rotation and **no input
content in logs**, manual test matrix (Win10/Win11, admin app, game, browser, IME, RDP),
privacy note in README.
Exit: a stranger can download the zip and run it without a terminal.

Deferred until someone asks: macOS (`CGEventTap` + Accessibility permission), Linux (evdev on
X11; Wayland global input needs a portal/`libei` — treat as its own project), OBS browser
source, per-app profiles.

## 6. Testing

- Unit: VK → key name table, modifier mask transitions, autorepeat dedupe, fade maths,
  config merge/corrupt fallback.
- `--dump` golden-event test fed from a scripted `SendInput` sequence (recorded fixture).
- Latency harness: synthetic `SendInput` stamps a timestamp, overlay logs present-time, assert
  p95 ≤ 30 ms over 1000 events.
- Manual matrix kept as a checklist in the repo so reviewers can run it.
- Optional later: frame snapshot test by reading back the software buffer (no window needed).

## 7. Risks

| Risk | Impact | Mitigation |
| --- | --- | --- |
| UIPI / integrity level: a non-elevated process cannot see input going to an elevated window | overlay silently goes dead over admin apps | detect and show a badge; optional `--elevated` relaunch documented, not forced |
| Fullscreen **exclusive** games composite above/under layered windows | no overlay in some games | document borderless-windowed; test DirectComposition path only if needed |
| Windows silently unhooks a slow low-level hook | input stops, no error | zero-work hook callback, watchdog that re-installs the hook, `--dump` proves liveness |
| Unsigned exe triggers SmartScreen/AV heuristics (global hooks look keylogger-ish) | users cannot run it | documented build instructions, source on GitHub, optional Authenticode later; ship no network code at all |
| Keyboard layout / IME: hooks give scan/virtual codes, not characters | wrong labels on non-US layouts | label from `MapVirtualKeyW`/`ToUnicode` with layout-change polling, fall back to physical name |
| Transparency + passthrough is OS glue, not a crate feature | Phase 2 rework | spike it in Phase 2 with an explicit kill criterion and a named fallback |
| Privacy: the tool literally reads every keystroke | reputational damage | never log or send key data, in-memory only, state it in README and privacy note |
| Mixed-DPI multi-monitor | blurry/clipped overlay | per-monitor v2 DPI manifest, tests at 100/150% |

## 8. Non-goals for v1

macOS/Linux, remapping or macro execution, input injection/replay, cloud sync, plugin API,
per-app profiles, theming engine beyond presets, full-screen cursor-following ghost layer.

## 9. Decisions

Locked 2026-10-21:
1. **Purpose** — keystroke/click display for demos and streaming. Read-only capture.
2. **Platform** — Windows only, behind a `Capture` trait so other OSes stay possible.
3. **Stack** — `windows`-crate low-level hooks; `softbuffer` + `tiny-skia` + `ab_glyph` painted
   into a layered window. No egui, no wgpu, no `rdev`.
4. **Shape** — fixed keycap panel with presets. Ghost-layer-following-cursor deferred.

Still open, low stakes, answer when the relevant phase starts:
- Elevation: show a "cannot see elevated apps" badge only, or offer an opt-in elevated
  relaunch? (plan: badge in Phase 3, relaunch flag in Phase 5 if you want it)
- Tray icon vs hotkey-only in v1. (plan: tray in Phase 4, hotkey-only until then)
- Config location/format if `config.toml` in the platform config dir bothers you.
- Font: bundled `ab_glyph`-compatible font, or system Segoe UI via direct write. (plan: bundle
  one OFL/MIT font so behaviour is identical on every machine)

## 10. Branching & hygiene

- `main` = released, `develop` = integration. `feature/<short-name>` → PR → `develop`.
  `release/x.y.z` → PR to `main` **and** back to `develop`. `hotfix/<name>` from `main`.
- Conventional Commits, intent only: `feat: add low-level keyboard hook`, `fix: normalise wheel delta`.
  No step/phase numbering, no ticket-only messages.
- Never commit: `.env`, tokens, signing certs, private registry URLs, machine paths containing
  usernames, `--dump` captures, or any recorded input/session data. `dist/`, `*.log`, `.env*`
  are already ignored.
- PRs to `main` and every push need your approval.

## Sources

- rdev 0.5.3 metadata and last publish: https://crates.io/crates/rdev
- rdev maintenance status: https://github.com/Narsil/rdev/issues/151
- `inputs` 0.1.1 (2026-08-24, 171 downloads): https://crates.io/crates/inputs
- willhook-rs: https://github.com/myood/willhook-rs
- winit ex-styles (WS_EX_LAYERED/TRANSPARENT/TOOLWINDOW): https://github.com/rust-windowing/winit/issues/2059
- winit click-through request: https://github.com/rust-windowing/winit/issues/1434
- winit transparency regression: https://github.com/rust-windowing/winit/issues/2502
- egui always-on-top: https://github.com/emilk/egui/discussions/3898
- egui input passthrough: https://github.com/emilk/egui/issues/1677
- egui transparency not working report: https://github.com/emilk/egui/discussions/4446
