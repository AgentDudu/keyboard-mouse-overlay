//! What to draw and how visible it is: held keys, fading keycaps, mouse state, click ripples.
//!
//! Pure state over `time` (seconds from an arbitrary origin). No pixels, no Win32, so the fade
//! curve, the cap limit and the ripple lifetimes are all unit-testable.

use crate::input::{Action, Event, Modifiers, MouseButton};

/// Never show more than this many keycaps at once — bounds the per-frame drawing cost.
pub const MAX_CAPS: usize = 10;

/// Timings for the fade and the ripple.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// How long a released keycap takes to become invisible.
    pub fade: f64,
    /// How long a click ripple lives.
    pub ripple: f64,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            fade: 0.9,
            ripple: 0.45,
        }
    }
}

/// Exponential decay: 1.0 while held, ~0.05 at the end of `fade`. Exponential rather than
/// linear because a linear ramp ends with a visible snap.
fn decay(age: f64, fade: f64) -> f32 {
    if fade <= 0.0 {
        return 0.0;
    }
    (-3.0 * (age / fade).clamp(0.0, 1.0)).exp() as f32
}

/// Ctrl, Shift, Alt and Win already have their own chips in the modifier row. Without this the
/// same key reads twice — once as a modifier chip, once as a keycap — which the two-accent
/// palette makes obvious instead of merely redundant.
fn is_modifier_vk(vk: u32) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0 | 0xA1 | 0xA2)
}

/// Unknown VK codes fall back to hex, matching `Event`'s display format.
fn cap_label(label: Option<&'static str>, vk: u32) -> String {
    label
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("vk{vk:02X}"))
}

#[derive(Clone, Debug)]
struct Cap {
    vk: u32,
    label: String,
    /// `None` while the key is still held.
    released: Option<f64>,
}

/// Everything the overlay shows, derived from input events.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    /// Real input only by default: a macro or a bot should never light the overlay. Turned on
    /// by `--show-injected` so a scripted `SendInput` sequence can drive a screenshot.
    show_injected: bool,
    caps: Vec<Cap>,
    ripples: Vec<(Ripple, f64)>,
    /// Held buttons. A list, not a set: `MouseButton` has no `Hash` and there are at most five.
    buttons: Vec<MouseButton>,
    modifiers: Modifiers,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Ripple {
    x: i32,
    y: i32,
    /// Right-button ripples are drawn larger, so left and right stay distinguishable.
    large: bool,
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    /// A scene that also shows injected input, for scripted captures and GIFs.
    pub fn showing_injected() -> Self {
        Self {
            show_injected: true,
            ..Self::default()
        }
    }

    /// Fold one normalised event into the scene. `time` is seconds from the caller's origin,
    /// `modifiers` comes from `InputStream` rather than being re-derived here.
    ///
    /// `Repeat` is ignored on purpose: autorepeat must not re-light a keycap.
    pub fn apply(&mut self, event: &Event, modifiers: Modifiers, time: f64, timing: &Timing) {
        self.modifiers = modifiers;
        match event {
            Event::Key {
                vk,
                label,
                action,
                injected,
                ..
            } => {
                if *injected && !self.show_injected {
                    return;
                }
                match action {
                    Action::Down if !is_modifier_vk(*vk) => {
                        self.press(*vk, cap_label(*label, *vk), time)
                    }
                    Action::Down => {}
                    Action::Up => self.release(*vk, time),
                    Action::Repeat => {}
                }
            }
            Event::Button {
                button,
                action,
                x,
                y,
                injected,
                ..
            } => {
                if *injected {
                    return;
                }
                match action {
                    Action::Down => {
                        if !self.buttons.contains(button) {
                            self.buttons.push(*button);
                        }
                        let large = matches!(button, MouseButton::Right);
                        self.ripples.push((
                            Ripple {
                                x: *x,
                                y: *y,
                                large,
                            },
                            time,
                        ));
                    }
                    Action::Up => {
                        self.buttons.retain(|held| held != button);
                    }
                    Action::Repeat => {}
                }
            }
            _ => {}
        }
        self.trim(time, timing);
    }

    fn press(&mut self, vk: u32, label: String, time: f64) {
        self.caps.retain(|cap| cap.vk != vk);
        self.caps.push(Cap {
            vk,
            label,
            released: None,
        });
        let _ = time;
    }

    fn release(&mut self, vk: u32, time: f64) {
        if let Some(cap) = self.caps.iter_mut().find(|cap| cap.vk == vk) {
            cap.released = Some(time);
        }
    }

    fn trim(&mut self, time: f64, timing: &Timing) {
        self.caps
            .retain(|cap| cap.released.is_none_or(|at| time - at <= timing.fade));
        self.ripples.retain(|(_, at)| time - *at <= timing.ripple);
    }

    /// Keycaps that still have visible life left at `time`. The view is the truth about what is
    /// on screen; `trim` only bounds memory.
    fn live_caps(&self, time: f64, timing: &Timing) -> impl Iterator<Item = &Cap> {
        self.caps
            .iter()
            .filter(move |cap| cap.released.is_none_or(|at| time - at <= timing.fade))
    }

    fn live_ripples(&self, time: f64, timing: &Timing) -> impl Iterator<Item = &(Ripple, f64)> {
        self.ripples
            .iter()
            .filter(move |(_, at)| time - *at <= timing.ripple)
    }

    /// Visible keycaps with their alpha, oldest first, newest `MAX_CAPS` only.
    pub fn caps(&self, time: f64, timing: &Timing) -> Vec<(&str, f32)> {
        let live = self.live_caps(time, timing).collect::<Vec<_>>();
        let start = live.len().saturating_sub(MAX_CAPS);
        live[start..]
            .iter()
            .map(|cap| {
                let alpha = cap.released.map_or(1.0, |at| decay(time - at, timing.fade));
                (cap.label.as_str(), alpha)
            })
            .collect()
    }

    /// Visible ripples as (x, y, alpha, radius), in the order they happened.
    pub fn ripples(&self, time: f64, timing: &Timing) -> Vec<(i32, i32, f32, f32)> {
        self.live_ripples(time, timing)
            .map(|(ripple, at)| {
                let age = ((time - at) / timing.ripple).clamp(0.0, 1.0) as f32;
                let radius = (14.0 + 46.0 * age) * if ripple.large { 1.35 } else { 1.0 };
                (ripple.x, ripple.y, 1.0 - age, radius)
            })
            .collect()
    }

    pub fn button_held(&self, button: MouseButton) -> bool {
        self.buttons.contains(&button)
    }

    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// True when anything would be drawn — lets the caller skip repainting an idle screen.
    pub fn active(&self, time: f64, timing: &Timing) -> bool {
        !self.modifiers.is_empty()
            || !self.buttons.is_empty()
            || self.live_caps(time, timing).next().is_some()
            || self.live_ripples(time, timing).next().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(vk: u32, action: Action, label: Option<&'static str>) -> Event {
        Event::Key {
            vk,
            label,
            action,
            extended: false,
            alt_down: false,
            injected: false,
            time: 0,
        }
    }

    fn button(button: MouseButton, action: Action) -> Event {
        Event::Button {
            button,
            action,
            x: 10,
            y: 20,
            injected: false,
            time: 0,
        }
    }

    fn apply(scene: &mut Scene, event: &Event, time: f64, timing: &Timing) {
        scene.apply(event, Modifiers::default(), time, timing);
    }

    #[test]
    fn a_held_modifier_is_not_also_a_keycap() {
        let timing = Timing::default();
        let mut scene = Scene::new();
        let mods = Modifiers {
            ctrl: true,
            alt: false,
            shift: false,
            win: false,
        };
        scene.apply(&key(0x11, Action::Down, Some("Ctrl")), mods, 0.0, &timing);
        assert!(scene.modifiers().ctrl, "the modifier row knows about it");
        assert!(
            scene.caps(0.0, &timing).is_empty(),
            "the keycap row must not repeat it: {:?}",
            scene.caps(0.0, &timing)
        );
        // A normal key alongside it is still a keycap.
        scene.apply(&key(0x41, Action::Down, Some("A")), mods, 0.1, &timing);
        assert_eq!(scene.caps(0.1, &timing).len(), 1);
    }

    #[test]
    fn held_key_is_fully_visible_then_fades_and_disappears() {
        let timing = Timing::default();
        let mut scene = Scene::new();
        apply(
            &mut scene,
            &key(0x41, Action::Down, Some("A")),
            0.0,
            &timing,
        );
        apply(&mut scene, &key(0x41, Action::Up, Some("A")), 0.0, &timing);
        assert_eq!(scene.caps(0.0, &timing)[0].1, 1.0);
        let mid = scene.caps(0.45, &timing)[0].1;
        let late = scene.caps(0.85, &timing)[0].1;
        assert!(
            mid > late && late > 0.0 && late < 0.1,
            "monotonic exponential fade: {mid} {late}"
        );
        assert!(
            scene.caps(5.0, &timing).is_empty(),
            "expired caps are dropped, not hidden"
        );
    }

    #[test]
    fn autorepeat_does_not_relight_a_released_key() {
        let timing = Timing::default();
        let mut scene = Scene::new();
        apply(
            &mut scene,
            &key(0x41, Action::Down, Some("A")),
            0.0,
            &timing,
        );
        apply(
            &mut scene,
            &key(0x41, Action::Repeat, Some("A")),
            0.0,
            &timing,
        );
        apply(&mut scene, &key(0x41, Action::Up, Some("A")), 0.0, &timing);
        assert_eq!(scene.caps(0.0, &timing).len(), 1);
        assert_eq!(scene.caps(0.0, &timing)[0].1, 1.0);
    }

    #[test]
    fn same_key_pressed_twice_shows_one_cap() {
        let timing = Timing::default();
        let mut scene = Scene::new();
        apply(
            &mut scene,
            &key(0x41, Action::Down, Some("A")),
            0.0,
            &timing,
        );
        apply(&mut scene, &key(0x41, Action::Up, Some("A")), 0.1, &timing);
        apply(
            &mut scene,
            &key(0x41, Action::Down, Some("A")),
            0.2,
            &timing,
        );
        assert_eq!(scene.caps(0.2, &timing).len(), 1);
        assert_eq!(
            scene.caps(0.2, &timing)[0].1,
            1.0,
            "repress makes it opaque again"
        );
    }

    #[test]
    fn caps_are_capped_and_oldest_dropped() {
        let timing = Timing::default();
        let mut scene = Scene::new();
        for offset in 0..(MAX_CAPS as u32 + 5) {
            apply(
                &mut scene,
                &key(0x30 + offset, Action::Down, Some("K")),
                0.0,
                &timing,
            );
        }
        let caps = scene.caps(0.0, &timing);
        assert_eq!(caps.len(), MAX_CAPS);
        assert_eq!(caps.last().unwrap().0, "K");
        assert_eq!(
            scene.caps.len(),
            MAX_CAPS + 5,
            "state is kept, only the view is capped"
        );
    }

    #[test]
    fn unknown_vk_falls_back_to_hex() {
        assert_eq!(cap_label(None, 0xC7), "vkC7");
        assert_eq!(cap_label(Some("F13"), 0x7C), "F13");
    }

    #[test]
    fn ripple_grows_fades_and_expires() {
        let timing = Timing::default();
        let mut scene = Scene::new();
        apply(
            &mut scene,
            &button(MouseButton::Left, Action::Down),
            0.0,
            &timing,
        );
        let start = scene.ripples(0.0, &timing);
        let later = scene.ripples(0.3, &timing);
        assert_eq!(start.len(), 1);
        assert!(later[0].3 > start[0].3, "ripple expands");
        assert!(later[0].2 < start[0].2, "ripple fades");
        assert!(scene.ripples(5.0, &timing).is_empty());
    }

    #[test]
    fn right_button_ripple_is_larger() {
        let timing = Timing::default();
        let mut scene = Scene::new();
        apply(
            &mut scene,
            &button(MouseButton::Left, Action::Down),
            0.0,
            &timing,
        );
        apply(
            &mut scene,
            &button(MouseButton::Right, Action::Down),
            0.0,
            &timing,
        );
        let ripples = scene.ripples(0.1, &timing);
        assert!(ripples[1].3 > ripples[0].3);
    }

    #[test]
    fn button_state_tracks_press_and_release() {
        let timing = Timing::default();
        let mut scene = Scene::new();
        assert!(!scene.button_held(MouseButton::Left));
        apply(
            &mut scene,
            &button(MouseButton::Left, Action::Down),
            0.0,
            &timing,
        );
        assert!(scene.button_held(MouseButton::Left));
        apply(
            &mut scene,
            &button(MouseButton::Left, Action::Up),
            0.0,
            &timing,
        );
        assert!(!scene.button_held(MouseButton::Left));
    }

    #[test]
    fn showing_injected_is_opt_in() {
        let timing = Timing::default();
        let mut scene = Scene::showing_injected();
        let pressed = Event::Key {
            vk: 0x41,
            label: Some("A"),
            action: Action::Down,
            extended: false,
            alt_down: false,
            injected: true,
            time: 0,
        };
        scene.apply(&pressed, Modifiers::default(), 0.0, &timing);
        assert_eq!(
            scene.caps(0.0, &timing).len(),
            1,
            "demo mode renders scripted input"
        );
    }

    #[test]
    fn injected_input_is_ignored() {
        let timing = Timing::default();
        let mut scene = Scene::new();
        let pressed = Event::Key {
            vk: 0x41,
            label: Some("A"),
            action: Action::Down,
            extended: false,
            alt_down: false,
            injected: true,
            time: 0,
        };
        scene.apply(&pressed, Modifiers::default(), 0.0, &timing);
        let click = Event::Button {
            button: MouseButton::Left,
            action: Action::Down,
            x: 0,
            y: 0,
            injected: true,
            time: 0,
        };
        scene.apply(&click, Modifiers::default(), 0.0, &timing);
        assert!(scene.caps(0.0, &timing).is_empty());
        assert!(scene.ripples(0.0, &timing).is_empty());
    }

    #[test]
    fn active_tracks_whether_a_repaint_is_worthwhile() {
        let timing = Timing::default();
        let mut scene = Scene::new();
        assert!(!scene.active(0.0, &timing));
        apply(
            &mut scene,
            &key(0x41, Action::Down, Some("A")),
            0.0,
            &timing,
        );
        assert!(scene.active(0.0, &timing));
        apply(&mut scene, &key(0x41, Action::Up, Some("A")), 0.0, &timing);
        assert!(scene.active(0.5, &timing), "still fading");
        assert!(!scene.active(5.0, &timing), "nothing left to draw");
    }

    #[test]
    fn modifiers_are_carried_through() {
        let timing = Timing::default();
        let mut scene = Scene::new();
        let mods = Modifiers {
            ctrl: true,
            alt: false,
            shift: false,
            win: false,
        };
        scene.apply(&key(0x41, Action::Down, Some("A")), mods, 0.0, &timing);
        assert!(scene.modifiers().ctrl);
        assert!(
            scene.active(0.0, &timing),
            "a held modifier alone is worth drawing"
        );
    }

    #[test]
    fn decay_endpoints() {
        assert_eq!(decay(0.0, 1.0), 1.0);
        assert!(decay(1.0, 1.0) < 0.06);
        assert_eq!(decay(0.5, 0.0), 0.0, "zero fade means no cap linger");
    }
}
