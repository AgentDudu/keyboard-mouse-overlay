//! OS-free input model: normalisation, autorepeat, modifiers, wheel accumulation.
//!
//! Nothing here imports `windows`. `capture` copies raw hook payloads into
//! [`RawEvent`] and this module turns them into [`Event`]s, so all the logic is
//! unit-testable without a desktop session.

use std::fmt;

// Kept local (not from the `windows` crate) so this module stays platform-neutral.
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_RBUTTONDBLCLK: u32 = 0x0206;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_MBUTTONDBLCLK: u32 = 0x0209;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_XBUTTONDOWN: u32 = 0x020B;
const WM_XBUTTONUP: u32 = 0x020C;
const WM_XBUTTONDBLCLK: u32 = 0x020D;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_MOUSEHWHEEL: u32 = 0x020E;

const LLKHF_EXTENDED: u32 = 0x01;
const LLKHF_ALTDOWN: u32 = 0x20;

const WHEEL_DELTA: i32 = 120;

/// Hook payload copied verbatim out of `KBDLLHOOKSTRUCT` / `MSLLHOOKSTRUCT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawEvent {
    pub msg: u32,
    pub vk: u32,
    pub flags: u32,
    pub mouse_data: u32,
    pub x: i32,
    pub y: i32,
    pub time: u32,
    pub injected: bool,
    /// `dwExtraInfo` matched our self-test tag.
    pub tagged: bool,
}

impl RawEvent {
    pub fn key(msg: u32, vk: u32, flags: u32, time: u32, injected: bool, tagged: bool) -> Self {
        Self {
            msg,
            vk,
            flags,
            mouse_data: 0,
            x: 0,
            y: 0,
            time,
            injected,
            tagged,
        }
    }

    pub fn mouse(
        msg: u32,
        mouse_data: u32,
        x: i32,
        y: i32,
        time: u32,
        injected: bool,
        tagged: bool,
    ) -> Self {
        Self {
            msg,
            vk: 0,
            flags: 0,
            mouse_data,
            x,
            y,
            time,
            injected,
            tagged,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    /// OS key autorepeat: same key down again without an intervening release.
    Repeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key {
        vk: u32,
        label: Option<&'static str>,
        action: Action,
        extended: bool,
        alt_down: bool,
        injected: bool,
        time: u32,
    },
    Button {
        button: MouseButton,
        action: Action,
        x: i32,
        y: i32,
        injected: bool,
        time: u32,
    },
    Wheel {
        axis: Axis,
        notches: i32,
        x: i32,
        y: i32,
        time: u32,
    },
    Move {
        x: i32,
        y: i32,
        time: u32,
    },
}

/// Signed high word of `MSLLHOOKSTRUCT::mouseData` (wheel delta) or the X button id.
fn high_word(value: u32) -> i32 {
    ((value >> 16) as u16) as i16 as i32
}

const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
const NUMPAD_DIGITS: [&str; 10] = [
    "1(num)", "2(num)", "3(num)", "4(num)", "5(num)", "6(num)", "7(num)", "8(num)", "9(num)",
    "0(num)",
];
const LETTERS: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];

/// Map a virtual-key code to a display label. `None` means the caller should
/// fall back to `vk{HH}` in hex (covers unmapped OEM keys and layout-specific codes).
pub fn key_label(vk: u32, extended: bool) -> Option<&'static str> {
    // Shared codes that need the extended bit to say which side/key they are.
    match vk {
        0x0D => return Some(if extended { "Enter(num)" } else { "Enter" }),
        0x11 => return Some(if extended { "Ctrl(R)" } else { "Ctrl" }),
        0x12 => return Some(if extended { "Alt(R)" } else { "Alt" }),
        0x30..=0x39 => return Some(DIGITS[vk as usize - 0x30]),
        0x41..=0x5A => return Some(LETTERS[vk as usize - 0x41]),
        0x60..=0x69 => return Some(NUMPAD_DIGITS[vk as usize - 0x60]),
        _ => {}
    }
    let label = match vk {
        0x08 => "Backspace",
        0x09 => "Tab",
        0x10 => "Shift",
        0x13 => "Pause",
        0x14 => "CapsLock",
        0x1B => "Esc",
        0x20 => "Space",
        0x21 => "PageUp",
        0x22 => "PageDown",
        0x23 => "End",
        0x24 => "Home",
        0x25 => "Left",
        0x26 => "Up",
        0x27 => "Right",
        0x28 => "Down",
        0x2D => "Insert",
        0x2E => "Delete",
        0x2F => "Help",
        0x5B => "Win",
        0x5C => "Win(R)",
        0x5D => "Menu",
        0x5F => "Sleep",
        0x6A => "*",
        0x6B => "+(num)",
        0x6C => ".(num)",
        0x6D => "-(num)",
        0x6E => "/(num)",
        0x6F => "NumLock",
        0x70 => "F1",
        0x71 => "F2",
        0x72 => "F3",
        0x73 => "F4",
        0x74 => "F5",
        0x75 => "F6",
        0x76 => "F7",
        0x77 => "F8",
        0x78 => "F9",
        0x79 => "F10",
        0x7A => "F11",
        0x7B => "F12",
        0x7C => "F13",
        0x7D => "F14",
        0x7E => "F15",
        0x7F => "F16",
        0x80 => "F17",
        0x81 => "F18",
        0x82 => "F19",
        0x83 => "F20",
        0x84 => "F21",
        0x85 => "F22",
        0x86 => "F23",
        0x87 => "F24",
        0x90 => "NumLock",
        0x91 => "ScrollLock",
        0xA0 => "Shift(L)",
        0xA1 => "Shift(R)",
        0xA2 => "Ctrl(L)",
        0xA3 => "Ctrl(R)",
        0xA4 => "Alt(L)",
        0xA5 => "Alt(R)",
        0xA6 => "AppMail",
        0xA7 => "AppMedia",
        0xA8 => "AppStart1",
        0xA9 => "AppStart2",
        0xAB => "VolumeMute",
        0xAC => "VolumeDown",
        0xAD => "VolumeUp",
        0xAE => "MediaNext",
        0xAF => "MediaPrev",
        0xB0 => "MediaStop",
        0xB3 => "MediaPause",
        0xB5 => "MediaSelect",
        0xB6 => "LaunchMail",
        0xBA => ";",
        0xBB => "=",
        0xBC => ",",
        0xBD => "-",
        0xBE => ".",
        0xBF => "/",
        0xC0 => "`",
        0xDB => "[",
        0xDC => "\\",
        0xDD => "]",
        0xDE => "'",
        0xDF => "|",
        _ => return None,
    };
    Some(label)
}

/// Held modifier mask derived from our own pressed-key table. `GetAsyncKeyState`
/// is focus- and timing-dependent, so we never ask it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Modifiers {
    pub fn is_empty(&self) -> bool {
        !(self.ctrl || self.alt || self.shift || self.win)
    }
}

impl fmt::Display for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (bit, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.win, "Win"),
        ] {
            if bit {
                if !first {
                    write!(f, "+")?;
                }
                write!(f, "{name}")?;
                first = false;
            }
        }
        Ok(())
    }
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Down => "Down",
            Action::Up => "Up",
            Action::Repeat => "Repeat",
        }
    }
}

/// Accumulate a raw wheel delta and return whole notches, if any.
///
/// A sign change discards the leftover remainder instead of carrying it, which
/// is what users expect from high-resolution wheels that reverse direction.
fn accumulate_wheel(remainder: &mut i32, delta: i32) -> Option<i32> {
    if delta == 0 {
        return None;
    }
    if delta.signum() != remainder.signum() {
        *remainder = 0;
    }
    *remainder += delta;
    let notches = *remainder / WHEEL_DELTA;
    *remainder %= WHEEL_DELTA;
    (notches != 0).then_some(notches)
}

/// Tracks held keys so autorepeat and modifier state are facts we compute, not
/// facts we ask the OS about.
pub struct InputStream {
    key_down: [bool; 256],
    wheel_v: i32,
    wheel_h: i32,
}

// Manual: `[bool; 256]` has no `Default`.
impl Default for InputStream {
    fn default() -> Self {
        Self {
            key_down: [false; 256],
            wheel_v: 0,
            wheel_h: 0,
        }
    }
}

impl InputStream {
    pub fn new() -> Self {
        Self::default()
    }

    /// At most one [`Event`] per raw event; partial wheel ticks return `None`.
    pub fn on_raw(&mut self, raw: RawEvent) -> Option<Event> {
        match raw.msg {
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
                let up = raw.msg == WM_KEYUP || raw.msg == WM_SYSKEYUP;
                let slot = (raw.vk & 0xFF) as usize;
                let was_down = self.key_down[slot];
                self.key_down[slot] = !up;
                let action = if up {
                    Action::Up
                } else if was_down {
                    Action::Repeat
                } else {
                    Action::Down
                };
                Some(Event::Key {
                    vk: raw.vk,
                    label: key_label(raw.vk, raw.flags & LLKHF_EXTENDED != 0),
                    action,
                    extended: raw.flags & LLKHF_EXTENDED != 0,
                    alt_down: raw.flags & LLKHF_ALTDOWN != 0,
                    injected: raw.injected,
                    time: raw.time,
                })
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let axis = if raw.msg == WM_MOUSEWHEEL {
                    Axis::Vertical
                } else {
                    Axis::Horizontal
                };
                let remainder = if axis == Axis::Vertical {
                    &mut self.wheel_v
                } else {
                    &mut self.wheel_h
                };
                accumulate_wheel(remainder, high_word(raw.mouse_data)).map(|notches| Event::Wheel {
                    axis,
                    notches,
                    x: raw.x,
                    y: raw.y,
                    time: raw.time,
                })
            }
            WM_MOUSEMOVE => Some(Event::Move {
                x: raw.x,
                y: raw.y,
                time: raw.time,
            }),
            msg => mouse_button_event(msg, raw).map(|(button, action)| Event::Button {
                button,
                action,
                x: raw.x,
                y: raw.y,
                injected: raw.injected,
                time: raw.time,
            }),
        }
    }

    pub fn modifiers(&self) -> Modifiers {
        let down = |vk: u16| self.key_down[vk as usize];
        Modifiers {
            ctrl: down(0xA2) || down(0xA3),
            alt: down(0xA4) || down(0xA5),
            shift: down(0xA0) || down(0xA1),
            win: down(0x5B) || down(0x5C),
        }
    }

    /// Held non-modifier keys in vk order — what the overlay draws as keycaps.
    pub fn held_keys(&self) -> Vec<u32> {
        (0u32..0x100)
            .filter(|vk| self.key_down[*vk as usize] && !is_modifier_vk(*vk))
            .collect()
    }
}

/// Double-click messages are treated as a press: Windows sends down/up/dblclk/up
/// for a real double click and the extra press is the display we want.
fn mouse_button_event(msg: u32, raw: RawEvent) -> Option<(MouseButton, Action)> {
    let (button, action) = match msg {
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => (MouseButton::Left, Action::Down),
        WM_LBUTTONUP => (MouseButton::Left, Action::Up),
        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => (MouseButton::Right, Action::Down),
        WM_RBUTTONUP => (MouseButton::Right, Action::Up),
        WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => (MouseButton::Middle, Action::Down),
        WM_MBUTTONUP => (MouseButton::Middle, Action::Up),
        WM_XBUTTONDOWN | WM_XBUTTONDBLCLK | WM_XBUTTONUP => {
            let id = (raw.mouse_data >> 16) as u16;
            let button = match id {
                1 => MouseButton::X1,
                2 => MouseButton::X2,
                _ => return None,
            };
            (
                button,
                if msg == WM_XBUTTONUP {
                    Action::Up
                } else {
                    Action::Down
                },
            )
        }
        _ => return None,
    };
    Some((button, action))
}

/// Keys shown as modifier glyphs rather than as their own keycap.
const fn is_modifier_vk(vk: u32) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::Key {
                vk,
                label,
                action,
                injected,
                alt_down,
                ..
            } => {
                let name = label.map_or_else(|| format!("vk{vk:02X}"), ToString::to_string);
                write!(f, "kbd  {:<6} {name}", action.name())?;
                if *alt_down {
                    write!(f, " [alt]")?;
                }
                if *injected {
                    write!(f, " [injected]")?;
                }
                Ok(())
            }
            Event::Button {
                button,
                action,
                x,
                y,
                injected,
                ..
            } => {
                write!(f, "mouse {:<6} {button:?} @ {x},{y}", action.name())?;
                if *injected {
                    write!(f, " [injected]")?;
                }
                Ok(())
            }
            Event::Wheel {
                axis,
                notches,
                x,
                y,
                ..
            } => {
                let dir = match (axis, notches.signum()) {
                    (Axis::Vertical, 1) => "up",
                    (Axis::Vertical, _) => "down",
                    (Axis::Horizontal, 1) => "right",
                    (Axis::Horizontal, _) => "left",
                };
                write!(f, "mouse wheel {dir} {} notch(es) @ {x},{y}", notches.abs())
            }
            Event::Move { x, y, .. } => write!(f, "mouse move  @ {x},{y}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(msg: u32, vk: u32) -> RawEvent {
        RawEvent::key(msg, vk, 0, 1, false, false)
    }

    #[test]
    fn autorepeat_is_not_a_fresh_press() {
        let mut s = InputStream::new();
        let first = s.on_raw(key(WM_KEYDOWN, 0x41)).unwrap();
        let repeat = s.on_raw(key(WM_KEYDOWN, 0x41)).unwrap();
        let up = s.on_raw(key(WM_KEYUP, 0x41)).unwrap();
        let again = s.on_raw(key(WM_KEYDOWN, 0x41)).unwrap();
        assert_eq!(first.action_of(), Some(Action::Down));
        assert_eq!(repeat.action_of(), Some(Action::Repeat));
        assert_eq!(up.action_of(), Some(Action::Up));
        assert_eq!(again.action_of(), Some(Action::Down));
    }

    #[test]
    fn modifiers_come_from_held_keys_only() {
        let mut s = InputStream::new();
        s.on_raw(key(WM_KEYDOWN, 0xA2));
        s.on_raw(key(WM_KEYDOWN, 0xA1));
        assert_eq!(
            s.modifiers(),
            Modifiers {
                ctrl: true,
                alt: false,
                shift: true,
                win: false
            }
        );
        assert_eq!(s.modifiers().to_string(), "Ctrl+Shift");
        s.on_raw(key(WM_KEYUP, 0xA2));
        assert_eq!(s.modifiers().to_string(), "Shift");
        s.on_raw(key(WM_KEYUP, 0xA1));
        assert!(s.modifiers().is_empty());
    }

    #[test]
    fn wheel_accumulates_and_resets_on_reversal() {
        let mut r = 0;
        assert_eq!(accumulate_wheel(&mut r, 30), None);
        assert_eq!(accumulate_wheel(&mut r, 30), None);
        assert_eq!(accumulate_wheel(&mut r, 60), Some(1));
        assert_eq!(accumulate_wheel(&mut r, -30), None); // sign flip drops the 0 remainder
        assert_eq!(accumulate_wheel(&mut r, -120), Some(-1));
        assert_eq!(accumulate_wheel(&mut r, 120), Some(1));
        assert_eq!(accumulate_wheel(&mut r, 0), None);
    }

    #[test]
    fn partial_wheel_events_are_suppressed() {
        let mut s = InputStream::new();
        let partial = s.on_raw(RawEvent::mouse(
            WM_MOUSEWHEEL,
            30 << 16,
            5,
            5,
            1,
            false,
            false,
        ));
        assert!(partial.is_none(), "partial tick must not render");
        let full = s.on_raw(RawEvent::mouse(
            WM_MOUSEWHEEL,
            (-120i32 as u32) << 16,
            5,
            5,
            2,
            false,
            false,
        ));
        assert_eq!(
            full.map(|e| match e {
                Event::Wheel { notches, .. } => Some(notches),
                _ => None,
            }),
            Some(Some(-1))
        );
    }

    #[test]
    fn xbutton_id_selects_the_button() {
        let mut s = InputStream::new();
        let back = s
            .on_raw(RawEvent::mouse(
                WM_XBUTTONDOWN,
                1 << 16,
                0,
                0,
                1,
                false,
                false,
            ))
            .unwrap();
        let fwd = s
            .on_raw(RawEvent::mouse(
                WM_XBUTTONUP,
                2 << 16,
                0,
                0,
                2,
                false,
                false,
            ))
            .unwrap();
        assert_eq!(
            back,
            Event::Button {
                button: MouseButton::X1,
                action: Action::Down,
                x: 0,
                y: 0,
                injected: false,
                time: 1
            }
        );
        assert_eq!(
            fwd,
            Event::Button {
                button: MouseButton::X2,
                action: Action::Up,
                x: 0,
                y: 0,
                injected: false,
                time: 2
            }
        );
    }

    #[test]
    fn unknown_xbutton_id_is_ignored() {
        let mut s = InputStream::new();
        assert!(
            s.on_raw(RawEvent::mouse(
                WM_XBUTTONDOWN,
                7 << 16,
                0,
                0,
                1,
                false,
                false
            ))
            .is_none()
        );
    }

    #[test]
    fn double_click_reads_as_a_press() {
        let mut s = InputStream::new();
        let e = s
            .on_raw(RawEvent::mouse(WM_LBUTTONDBLCLK, 0, 3, 4, 1, false, false))
            .unwrap();
        assert_eq!(
            e,
            Event::Button {
                button: MouseButton::Left,
                action: Action::Down,
                x: 3,
                y: 4,
                injected: false,
                time: 1
            }
        );
    }

    #[test]
    fn labels_and_fallbacks() {
        assert_eq!(key_label(0x0D, false), Some("Enter"));
        assert_eq!(key_label(0x0D, true), Some("Enter(num)"));
        assert_eq!(key_label(0xA3, false), Some("Ctrl(R)"));
        assert_eq!(key_label(0x41, false), Some("A"));
        assert_eq!(key_label(0x7C, false), Some("F13"));
        assert_eq!(key_label(0x6E, true), Some("/(num)"));
        assert_eq!(key_label(0xF5, false), None);
    }

    #[test]
    fn display_is_stable_and_readable() {
        let mut s = InputStream::new();
        let e = s
            .on_raw(RawEvent::key(WM_KEYDOWN, 0x41, 0, 7, true, false))
            .unwrap();
        assert_eq!(e.to_string(), "kbd  Down   A [injected]");
        let alt = s
            .on_raw(RawEvent::key(
                WM_KEYDOWN,
                0x7A,
                LLKHF_ALTDOWN,
                8,
                false,
                false,
            ))
            .unwrap();
        assert_eq!(alt.to_string(), "kbd  Down   F11 [alt]");
        let unmapped = s.on_raw(key(WM_KEYDOWN, 0xF5)).unwrap();
        assert_eq!(unmapped.to_string(), "kbd  Down   vkF5");
        let w = s
            .on_raw(RawEvent::mouse(
                WM_MOUSEHWHEEL,
                120 << 16,
                1,
                2,
                9,
                false,
                false,
            ))
            .unwrap();
        assert_eq!(w.to_string(), "mouse wheel right 1 notch(es) @ 1,2");
    }

    #[test]
    fn held_keys_are_non_modifiers_in_vk_order() {
        let mut input = InputStream::new();
        input.on_raw(key(WM_KEYDOWN, 0xA2));
        input.on_raw(key(WM_KEYDOWN, 0x41));
        input.on_raw(key(WM_KEYDOWN, 0x32));
        assert_eq!(input.held_keys(), vec![0x32, 0x41]);
        input.on_raw(key(WM_KEYUP, 0x41));
        assert_eq!(input.held_keys(), vec![0x32]);
        input.on_raw(key(WM_KEYUP, 0x32));
        assert!(
            input.held_keys().is_empty(),
            "modifiers alone are not keycaps"
        );
    }

    trait ActionOf {
        fn action_of(&self) -> Option<Action>;
    }
    impl ActionOf for Event {
        fn action_of(&self) -> Option<Action> {
            match self {
                Event::Key { action, .. } | Event::Button { action, .. } => Some(*action),
                _ => None,
            }
        }
    }
}
