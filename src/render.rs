//! Turning a [`Scene`] into pixels: panel backplate, modifier row, keycaps, mouse glyph, ripples.
//!
//! Pure — frame, scene, face and layout in; inked pixels out. Placement is a function of the
//! layout, so all of it is testable without a window.

use crate::input::MouseButton;
use crate::overlay::{Frame, Rect, Rgba};
use crate::scene::{Scene, Timing};
use crate::text::Face;

/// A palette. Colours are data, not code paths, so a theme is one value to pick and one test to
/// check: text must contrast with the cap it sits on, and the two accents must be tellable apart.
///
/// `ink` is the text drawn *on a lit cap*, so it opposes the cap colour; `shadow` is the halo
/// behind that text, so it opposes `ink` again (light theme: dark caps, light text, dark halo).
/// The two dark palettes are Catppuccin Mocha, the light ones Catppuccin Latte.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub name: &'static str,
    pub ink: Rgba,
    pub panel: Rgba,
    pub cap_idle: Rgba,
    pub label_idle: Rgba,
    pub cap_lit: Rgba,
    pub mod_lit: Rgba,
    pub shadow: Rgba,
    pub ripple: Rgba,
    pub mouse_lit: Rgba,
    pub mouse_idle: Rgba,
}

impl Theme {
    pub const DARK: Theme = Theme {
        name: "dark",
        ink: Rgba {
            r: 17,
            g: 17,
            b: 27,
            a: 255,
        },
        panel: Rgba {
            r: 30,
            g: 30,
            b: 46,
            a: 148,
        },
        cap_idle: Rgba {
            r: 49,
            g: 50,
            b: 68,
            a: 140,
        },
        label_idle: Rgba {
            r: 205,
            g: 214,
            b: 244,
            a: 184,
        },
        cap_lit: Rgba {
            r: 137,
            g: 180,
            b: 250,
            a: 245,
        },
        mod_lit: Rgba {
            r: 203,
            g: 166,
            b: 247,
            a: 245,
        },
        shadow: Rgba {
            r: 245,
            g: 245,
            b: 247,
            a: 90,
        },
        ripple: Rgba {
            r: 249,
            g: 226,
            b: 175,
            a: 220,
        },
        mouse_lit: Rgba {
            r: 250,
            g: 179,
            b: 135,
            a: 240,
        },
        mouse_idle: Rgba {
            r: 69,
            g: 71,
            b: 90,
            a: 217,
        },
    };

    /// For bright desks and screen-share backgrounds, where a dark strip disappears.
    pub const LIGHT: Theme = Theme {
        name: "light",
        ink: Rgba {
            r: 239,
            g: 241,
            b: 245,
            a: 255,
        },
        panel: Rgba {
            r: 239,
            g: 241,
            b: 245,
            a: 224,
        },
        cap_idle: Rgba {
            r: 204,
            g: 208,
            b: 218,
            a: 205,
        },
        label_idle: Rgba {
            r: 76,
            g: 79,
            b: 105,
            a: 199,
        },
        cap_lit: Rgba {
            r: 30,
            g: 102,
            b: 245,
            a: 245,
        },
        mod_lit: Rgba {
            r: 136,
            g: 57,
            b: 239,
            a: 245,
        },
        shadow: Rgba {
            r: 4,
            g: 4,
            b: 5,
            a: 77,
        },
        ripple: Rgba {
            r: 223,
            g: 142,
            b: 29,
            a: 220,
        },
        mouse_lit: Rgba {
            r: 254,
            g: 100,
            b: 11,
            a: 240,
        },
        mouse_idle: Rgba {
            r: 156,
            g: 160,
            b: 176,
            a: 217,
        },
    };

    pub const ALL: [&'static Theme; 2] = [&Self::DARK, &Self::LIGHT];

    pub fn named(name: &str) -> Option<&'static Theme> {
        Self::ALL.into_iter().find(|theme| theme.name == name)
    }
}

/// Sizes in physical pixels plus the palette to draw with. Everything derives from one scale, so
/// a 150 % display is one number.
#[derive(Clone, Copy, Debug)]
pub struct Layout<'theme> {
    pub theme: &'theme Theme,
    pub pad: u32,
    pub gap: u32,
    pub cap_height: u32,
    pub inner_pad: u32,
    pub radius: u32,
    pub font_px: f32,
    pub mouse_width: u32,
    pub mouse_height: u32,
}

impl<'theme> Layout<'theme> {
    pub fn themed(scale: f32, theme: &'theme Theme) -> Self {
        let unit = |value: f32| (value * scale).round().max(1.0) as u32;
        Self {
            theme,
            pad: unit(14.0),
            gap: unit(8.0),
            cap_height: unit(46.0),
            inner_pad: unit(12.0),
            radius: unit(9.0),
            font_px: 26.0 * scale,
            mouse_width: unit(38.0),
            mouse_height: unit(58.0),
        }
    }

    pub fn row_height(&self) -> u32 {
        self.cap_height.max(self.mouse_height)
    }
}

/// How many leading slots are the modifiers.
const MOD_SLOTS: usize = 4;

/// One keycap slot. `None` alpha means an idle modifier: outlined, labelled, never bright.
/// `modifier` picks the second accent, so a shortcut reads as one at a glance.
struct Slot<'a> {
    label: &'a str,
    alpha: Option<f32>,
    modifier: bool,
    x: u32,
    width: u32,
}

fn dim(colour: Rgba, alpha: f32) -> Rgba {
    Rgba {
        a: (colour.a as f32 * alpha).clamp(0.0, 255.0) as u8,
        ..colour
    }
}

/// Draw the panel into `frame` (which this clears), top-left anchored, and return the backplate
/// rectangle. `None` means nothing to show, so the caller can leave the screen untouched.
///
/// Ripples are separate: they sit at the click position on the caller's full-size canvas, while
/// the panel is small and cheap to repaint.
pub fn render_panel(
    frame: &mut Frame,
    scene: &Scene,
    face: &Face<'_>,
    layout: &Layout<'_>,
    time: f64,
    timing: &Timing,
) -> Option<Rect> {
    frame.clear();
    if !scene.active(time, timing) {
        return None;
    }

    let mods = scene.modifiers();
    let mut slots = Vec::new();
    let mut x = layout.pad;
    // The first MOD_SLOTS entries are the modifiers, so the accent choice cannot drift from the
    // label text.
    for (index, (label, alpha)) in [
        ("Ctrl", mods.ctrl.then_some(1.0)),
        ("Shift", mods.shift.then_some(1.0)),
        ("Alt", mods.alt.then_some(1.0)),
        ("Win", mods.win.then_some(1.0)),
    ]
    .into_iter()
    .chain(
        scene
            .caps(time, timing)
            .into_iter()
            .map(|(label, alpha)| (label, Some(alpha))),
    )
    .enumerate()
    {
        let text_width = face.width(label, layout.font_px).ceil() as u32;
        let width = (text_width + layout.inner_pad * 2).max(layout.cap_height);
        slots.push(Slot {
            label,
            alpha,
            modifier: index < MOD_SLOTS,
            x,
            width,
        });
        x += width + layout.gap;
    }

    let mouse_x = x;
    x += layout.mouse_width;
    let panel = Rect {
        x: 0,
        y: 0,
        w: (x + layout.pad).min(frame.w),
        h: (layout.row_height() + layout.pad * 2).min(frame.h),
    };

    let t = layout.theme;
    frame.fill_round_rect(panel, layout.radius * 2, t.panel);

    let cap_top = layout.pad + (layout.row_height() - layout.cap_height) / 2;
    for slot in &slots {
        let baseline = face.baseline_for(layout.font_px, cap_top, layout.cap_height, slot.label);
        let box_colour = slot.alpha.map_or(t.cap_idle, |alpha| {
            let lit = if slot.modifier { t.mod_lit } else { t.cap_lit };
            dim(lit, alpha)
        });
        frame.fill_round_rect(
            Rect {
                x: slot.x,
                y: cap_top,
                w: slot.width,
                h: layout.cap_height,
            },
            layout.radius,
            box_colour,
        );
        let left = (slot.x
            + (slot.width - face.width(slot.label, layout.font_px).ceil() as u32) / 2)
            as i32;
        match slot.alpha {
            // Offset shadow keeps white glyphs readable over a bright desktop.
            Some(alpha) => {
                face.draw(
                    frame,
                    left + 2,
                    baseline + 2,
                    layout.font_px,
                    slot.label,
                    dim(t.shadow, alpha),
                );
                face.draw(
                    frame,
                    left,
                    baseline,
                    layout.font_px,
                    slot.label,
                    dim(t.ink, alpha),
                );
            }
            None => {
                face.draw(
                    frame,
                    left,
                    baseline,
                    layout.font_px,
                    slot.label,
                    t.label_idle,
                );
            }
        }
    }

    draw_mouse(frame, scene, layout, mouse_x, layout.pad);

    Some(panel)
}

/// Draw the click ripples onto a full-size canvas at their screen coordinates.
pub fn draw_ripples(
    frame: &mut Frame,
    scene: &Scene,
    layout: &Layout<'_>,
    time: f64,
    timing: &Timing,
) {
    let scale = layout.mouse_width as f32 / 38.0;
    for (cx, cy, alpha, radius) in scene.ripples(time, timing) {
        frame.stroke_circle(cx, cy, radius * scale, 3.0, dim(layout.theme.ripple, alpha));
    }
}

/// Mouse silhouette whose buttons light up: left half, right half, wheel.
fn draw_mouse(frame: &mut Frame, scene: &Scene, layout: &Layout<'_>, x: u32, y: u32) {
    let (w, h) = (layout.mouse_width, layout.mouse_height);
    let state = |held: bool| {
        if held {
            layout.theme.mouse_lit
        } else {
            layout.theme.mouse_idle
        }
    };
    let half = w / 2;
    let top = h / 2;

    frame.fill_round_rect(Rect { x, y, w, h }, layout.radius, layout.theme.mouse_idle);
    frame.fill_round_rect(
        Rect {
            x: x + 2,
            y: y + 2,
            w: half - 3,
            h: top - 4,
        },
        4,
        state(scene.button_held(MouseButton::Left)),
    );
    frame.fill_round_rect(
        Rect {
            x: x + half + 1,
            y: y + 2,
            w: half - 3,
            h: top - 4,
        },
        4,
        state(scene.button_held(MouseButton::Right)),
    );
    frame.fill_round_rect(
        Rect {
            x: x + half - 2,
            y: y + 4,
            w: 5,
            h: top / 2,
        },
        2,
        state(scene.button_held(MouseButton::Middle)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Action, Event, Modifiers};
    use crate::text::FONT_BYTES;

    fn dark(scale: f32) -> Layout<'static> {
        Layout::themed(scale, &Theme::DARK)
    }

    fn face() -> Face<'static> {
        Face::new(FONT_BYTES).unwrap()
    }

    fn inked(frame: &Frame) -> usize {
        frame
            .px
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] != 0)
            .count()
    }

    fn alpha_at(frame: &Frame, x: u32, y: u32) -> u8 {
        frame.px[(y as usize * frame.w as usize + x as usize) * 4 + 3]
    }

    fn key(vk: u32, action: Action, label: &'static str) -> Event {
        Event::Key {
            vk,
            label: Some(label),
            action,
            extended: false,
            alt_down: false,
            injected: false,
            time: 0,
        }
    }

    fn click(button: MouseButton, action: Action) -> Event {
        Event::Button {
            button,
            action,
            x: 300,
            y: 200,
            injected: false,
            time: 0,
        }
    }

    fn scene_from(events: &[(&Event, f64)], timing: &Timing) -> Scene {
        let mut scene = Scene::new();
        for (event, time) in events {
            scene.apply(event, Modifiers::default(), *time, timing);
        }
        scene
    }

    #[test]
    fn empty_scene_draws_nothing() {
        let timing = Timing::default();
        let layout = dark(1.0);
        let mut frame = Frame::new(600, 120);
        assert!(render_panel(&mut frame, &Scene::new(), &face(), &layout, 0.0, &timing).is_none());
        assert_eq!(
            inked(&frame),
            0,
            "an idle overlay must be fully transparent"
        );
    }

    #[test]
    fn held_key_is_inked_inside_the_frame() {
        let timing = Timing::default();
        let layout = dark(1.0);
        let press = key(0x41, Action::Down, "A");
        let mut scene = Scene::new();
        let mods = Modifiers {
            ctrl: true,
            alt: false,
            shift: false,
            win: false,
        };
        scene.apply(&press, mods, 0.0, &timing);
        let mut frame = Frame::new(900, 120);
        let panel = render_panel(&mut frame, &scene, &face(), &layout, 0.0, &timing)
            .expect("something drawn");
        assert!(inked(&frame) > 500);
        assert!(panel.w < frame.w && panel.h <= frame.h, "fits: {panel:?}");
    }

    /// Average x of every pixel painted in exactly this colour, and how many. The frame is
    /// premultiplied, so compare against what a solid fill of `colour` leaves behind.
    fn tint(frame: &Frame, colour: Rgba) -> (f64, usize) {
        let want = crate::overlay::premultiply(colour);
        let mut total = 0i64;
        let mut count = 0usize;
        for (index, px) in frame.px.as_chunks::<4>().0.iter().enumerate() {
            if px[..3] == want[..3] && px[3] == want[3] {
                total += (index % frame.w as usize) as i64;
                count += 1;
            }
        }
        (total as f64 / count.max(1) as f64, count)
    }

    #[test]
    fn a_held_modifier_and_a_held_key_get_different_accents() {
        let timing = Timing::default();
        let layout = dark(1.0);
        let press = key(0x41, Action::Down, "A");
        let mut scene = Scene::new();
        scene.apply(
            &press,
            Modifiers {
                ctrl: true,
                alt: false,
                shift: false,
                win: false,
            },
            0.0,
            &timing,
        );
        let mut frame = Frame::new(900, 120);
        render_panel(&mut frame, &scene, &face(), &layout, 0.0, &timing).expect("drawn");
        let (ctrl_x, ctrl_pixels) = tint(&frame, Theme::DARK.mod_lit);
        let (key_x, key_pixels) = tint(&frame, Theme::DARK.cap_lit);
        assert!(
            ctrl_pixels > 100 && key_pixels > 100,
            "{ctrl_pixels} {key_pixels}"
        );
        assert!(
            ctrl_x < key_x,
            "modifiers are the left group: {ctrl_x} {key_x}"
        );
    }

    #[test]
    fn ripple_draws_at_the_click_position_after_the_key_faded() {
        let timing = Timing {
            fade: 0.1,
            ripple: 0.5,
        };
        let layout = dark(1.0);
        let press = key(0x41, Action::Down, "A");
        let release = key(0x41, Action::Up, "A");
        let down = click(MouseButton::Left, Action::Down);
        let scene = scene_from(&[(&press, 0.0), (&release, 0.02), (&down, 0.4)], &timing);
        let mut frame = Frame::new(400, 260);
        draw_ripples(&mut frame, &scene, &layout, 0.5, &timing);
        // The ring is hollow, so sample out at its own radius rather than at the click point.
        let radius = scene.ripples(0.5, &timing)[0].3 as i32;
        assert!(
            alpha_at(&frame, (300 + radius) as u32, 200) > 0,
            "ring drawn around the click position"
        );
        let mut empty = Frame::new(60, 60);
        draw_ripples(&mut empty, &Scene::new(), &layout, 0.0, &timing);
        assert_eq!(inked(&empty), 0, "no clicks, no rings");
    }

    #[test]
    fn long_labels_and_tiny_frames_do_not_panic() {
        let timing = Timing::default();
        let layout = dark(1.0);
        let press = key(0x41, Action::Down, "PrintScreen");
        let scene = scene_from(&[(&press, 0.0)], &timing);
        let mut frame = Frame::new(24, 24);
        render_panel(&mut frame, &scene, &face(), &layout, 0.0, &timing);
        assert!(inked(&frame) > 0, "clipped but not empty");
    }

    #[test]
    fn every_theme_contrasts_text_with_its_cap() {
        let brightness = |c: Rgba| c.r as i32 * 3 + c.g as i32 * 5 + c.b as i32 * 2;
        let farthest = |a: Rgba, b: Rgba| {
            (a.r as i32 - b.r as i32)
                .abs()
                .max((a.g as i32 - b.g as i32).abs())
                .max((a.b as i32 - b.b as i32).abs())
        };
        for theme in Theme::ALL {
            let name = theme.name;
            assert!(
                (brightness(theme.ink) - brightness(theme.cap_lit)).abs() > 300,
                "{name}: lit cap and its text too close"
            );
            assert!(
                (brightness(theme.ink) - brightness(theme.mod_lit)).abs() > 300,
                "{name}: lit modifier and its text too close"
            );
            assert!(
                (brightness(theme.label_idle) - brightness(theme.cap_idle)).abs() > 120,
                "{name}: idle row unreadable"
            );
            // The point of two accents: a modifier has to look different from a key.
            assert!(
                farthest(theme.cap_lit, theme.mod_lit) > 50,
                "{name}: the two accents are indistinguishable"
            );
            // The halo behind the text must oppose the text, or it blurs it.
            assert!(
                (brightness(theme.ink) - brightness(theme.shadow)).abs() > 400,
                "{name}: text halo does not contrast with the text"
            );
            // Backplate opacity: below this the label competes with the desktop behind it.
            assert!(
                theme.panel.a >= 128,
                "{name}: panel too translucent to read"
            );
        }
        assert!(Theme::named("DARK").is_none(), "names are exact");
        assert_eq!(Theme::named("light").unwrap().name, "light");
    }

    #[test]
    fn rendering_at_150_percent_matches_the_metrics() {
        let timing = Timing::default();
        let press = key(0x41, Action::Down, "A");
        let mut scene = Scene::new();
        scene.apply(&press, Modifiers::default(), 0.0, &timing);
        let mut small = Frame::new(1200, 200);
        let mut large = Frame::new(1800, 300);
        let one =
            render_panel(&mut small, &scene, &face(), &dark(1.0), 0.0, &timing).expect("drawn");
        let big = render_panel(
            &mut large,
            &scene,
            &face(),
            &Layout::themed(1.5, &Theme::DARK),
            0.0,
            &timing,
        )
        .expect("drawn");
        assert!(
            (big.h as f32 - one.h as f32 * 1.5).abs() <= 2.0,
            "height {} vs {}",
            big.h,
            one.h
        );
        assert!(
            (big.w as f32 - one.w as f32 * 1.5).abs() <= 10.0,
            "width {} vs {}",
            big.w,
            one.w
        );
        assert!(inked(&large) > inked(&small), "1.5x paints more ink");
    }

    #[test]
    fn layout_scales_linearly() {
        let small = dark(1.0);
        let large = dark(1.5);
        assert_eq!(large.cap_height, (small.cap_height as f32 * 1.5) as u32);
        assert!((large.font_px - small.font_px * 1.5).abs() < 0.01);
    }

    #[test]
    fn mouse_buttons_light_individually() {
        let timing = Timing::default();
        let layout = dark(1.0);
        let down = click(MouseButton::Right, Action::Down);
        let scene = scene_from(&[(&down, 0.0)], &timing);
        let mut frame = Frame::new(400, 120);
        render_panel(&mut frame, &scene, &face(), &layout, 0.0, &timing);
        // Stored premultiplied BGRA, so orange (255,150,96) lands as [96, 150, 255, a].
        let orange = frame
            .px
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] < 160 && px[2] > 200 && px[3] > 200)
            .count();
        assert!(orange > 30, "right button should be lit, found {orange}");
    }
}
