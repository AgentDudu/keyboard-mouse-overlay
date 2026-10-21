//! Windows low-level hook capture.
//!
//! The hook callback must stay near-weightless: Windows silently removes a
//! low-level hook that overruns its time budget. So the callback only reads the
//! struct it was handed and does a `try_send` — no formatting, no locks, no
//! blocking, no allocation.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_INJECTED,
    LLKHF_LOWER_IL_INJECTED, MSG, MSLLHOOKSTRUCT, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_QUIT,
};

use crate::input::RawEvent;

/// `dwExtraInfo` sentinel for our own synthetic self-test input. The hook swallows
/// only tagged injected events, so the self-test cannot disturb a focused window.
pub const TEST_TAG: usize = 0x4B4D_4F56_5345_4C46; // "KMOVSELF"

/// Self-test only: drop tagged injected events instead of forwarding them.
static SWALLOW_TAGGED: AtomicBool = AtomicBool::new(false);

/// Events discarded because the consumer had not drained the channel.
pub static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Bounded so a stalled consumer cannot grow the heap without limit. A bounded
/// `SyncSender` is the only queue handle safe to reach from the hook callback.
const QUEUE_CAPACITY: usize = 4096;
static SINK: OnceLock<SyncSender<RawEvent>> = OnceLock::new();

fn emit(event: RawEvent) {
    // shortcut: drops the newest event when the consumer stalls, oldest-first
    // eviction needs a ring buffer; upgrade only if DROPPED ever climbs in practice.
    let sent = SINK
        .get()
        .map(|tx| tx.try_send(event).is_ok())
        .unwrap_or(false);
    if !sent {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let tagged = kb.dwExtraInfo == TEST_TAG;
        let injected =
            kb.flags.contains(LLKHF_INJECTED) || kb.flags.contains(LLKHF_LOWER_IL_INJECTED);
        // Record first, swallow second: the self-test must see what it injected.
        emit(RawEvent::key(
            wparam.0 as u32,
            kb.vkCode,
            kb.flags.0,
            kb.time,
            injected,
            tagged,
        ));
        if tagged && SWALLOW_TAGGED.load(Ordering::Relaxed) {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let ms = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let tagged = ms.dwExtraInfo == TEST_TAG;
        // shortcut: MSLLHOOKSTRUCT has no injected flag (only XBUTTON bits), so
        // synthetic mouse input is only detectable via a dwExtraInfo convention.
        emit(RawEvent::mouse(
            wparam.0 as u32,
            ms.mouseData,
            ms.pt.x,
            ms.pt.y,
            ms.time,
            false,
            tagged,
        ));
        if tagged && SWALLOW_TAGGED.load(Ordering::Relaxed) {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Running capture. Dropping without [`Capture::stop`] leaves the hook installed
/// until this process exits, so always stop explicitly.
pub struct Capture {
    thread_id: u32,
    handle: Option<JoinHandle<()>>,
}

impl Capture {
    pub fn stop(mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            let _ = handle.join();
        }
    }
}

/// Install both low-level hooks on a dedicated thread with its own message loop.
/// Both hooks are installed or neither is: a half-installed pair is a confusing
/// "why is there no mouse input" bug.
pub fn start() -> Result<(Receiver<RawEvent>, Capture), String> {
    let (tx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
    let (ready_tx, ready_rx) = mpsc::channel();

    let handle = std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || {
            let _ = SINK.set(tx);
            // hMod may be NULL for the two low-level hooks, so no module handle needed.
            let keyboard =
                match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) } {
                    Ok(hook) => hook,
                    Err(err) => {
                        let _ = ready_tx.send(Err(format!("keyboard hook failed: {err}")));
                        return;
                    }
                };
            let mouse = match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) } {
                Ok(hook) => hook,
                Err(err) => {
                    unsafe {
                        let _ = UnhookWindowsHookEx(keyboard);
                    }
                    let _ = ready_tx.send(Err(format!("mouse hook failed: {err}")));
                    return;
                }
            };

            let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));

            let mut message = MSG::default();
            // GetMessageW returns false on WM_QUIT or error: stop, then unhook.
            while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            unsafe {
                let _ = UnhookWindowsHookEx(mouse);
                let _ = UnhookWindowsHookEx(keyboard);
            }
        })
        .map_err(|err| format!("failed to spawn capture thread: {err}"))?;

    let thread_id = ready_rx
        .recv()
        .map_err(|err| format!("capture thread died before reporting: {err}"))??;
    Ok((
        rx,
        Capture {
            thread_id,
            handle: Some(handle),
        },
    ))
}

pub fn set_swallow_tagged(on: bool) {
    SWALLOW_TAGGED.store(on, Ordering::SeqCst);
}

/// Inject one keyboard event carrying [`TEST_TAG`]. Returns false if Windows
/// refused it.
pub fn inject_key(vk: u16, up: bool) -> bool {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: TEST_TAG,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    sent == 1
}
