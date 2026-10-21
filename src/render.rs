//! Turning a [`Scene`] into pixels: panel backplate, modifier row, keycaps, mouse glyph, ripples.
//!
//! Pure — frame, scene, face and layout in; inked pixels out. Placement is a function of the
//! layout, so all of it is testable without a window.

use crate::input::MouseButton;
use crate::overlay::{Frame, Rect, Rgba};
use crate::scene::{Scene, Timing};
use crate::text::Face;

const INK: Rgba = Rgba {
    r: 240,
    g: 244,
    b: 255,
    a: 255,
};
const PANEL: Rgba = Rgba {
    r: 16,
    g: 18,
    b: 26,
    a: 96,
};
const CAP_IDLE: Rgba = Rgba {
    r: 120,
    g: 132,
    b: 156,
    a: 46,
};
const LABEL_IDLE: Rgba = Rgba {
    r: 176,
    g: 186,
    b: 206,
    a: 110,
};
const CAP_LIT: Rgba = Rgba {
    r: 96,
    g: 164,
    b: 255,
    a: 235,
};
const SHADOW: Rgba = Rgba {
    r: 0,
    g: 0,
    b: 0,
    a: 110,
};
const RIPPLE: Rgba = Rgba {
    r: 255,
    g: 214,
    b: 120,
    a: 220,
};
const MOUSE_LIT: Rgba = Rgba {
    r: 255,
    g: 150,
    b: 96,
    a: 240,
};
const MOUSE_IDLE: Rgba = Rgba {
    r: 120,
    g: 132,
    b: 156,
    a: 70,
};

/// Sizes in physical pixels. Everything derives from one scale, so a 150 % display is one number.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub pad: u32,
    pub gap: u32,
    pub cap_height: u32,
    pub inner_pad: u32,
    pub radius: u32,
    pub font_px: f32,
    pub mouse_width: u32,
    pub mouse_height: u32,
}

impl Layout {
    pub fn new(scale: f32) -> Self {
        let unit = |value: f32| (value * scale).round().max(1.0) as u32;
        Self {
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

/// One keycap slot. `None` alpha means an idle modifier: outlined, labelled, never bright.
struct Slot<'a> {
    label: &'a str,
    alpha: Option<f32>,
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
    layout: &Layout,
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
    for (label, alpha) in [
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
    ) {
        let text_width = face.width(label, layout.font_px).ceil() as u32;
        let width = (text_width + layout.inner_pad * 2).max(layout.cap_height);
        slots.push(Slot {
            label,
            alpha,
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

    frame.fill_round_rect(panel, layout.radius * 2, PANEL);

    let cap_top = layout.pad + (layout.row_height() - layout.cap_height) / 2;
    let baseline =
        (cap_top as f32 + (layout.cap_height as f32 + face.ascent(layout.font_px)) / 2.0) as i32;
    for slot in &slots {
        let box_colour = slot.alpha.map_or(CAP_IDLE, |alpha| dim(CAP_LIT, alpha));
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
                    dim(SHADOW, alpha),
                );
                face.draw(
                    frame,
                    left,
                    baseline,
                    layout.font_px,
                    slot.label,
                    dim(INK, alpha),
                );
            }
            None => {
                face.draw(
                    frame,
                    left,
                    baseline,
                    layout.font_px,
                    slot.label,
                    LABEL_IDLE,
                );
            }
        }
    }

    draw_mouse(frame, scene, layout, mouse_x, layout.pad);

    Some(panel)
}

/// Draw the click ripples onto a full-size canvas at their screen coordinates.
pub fn draw_ripples(frame: &mut Frame, scene: &Scene, layout: &Layout, time: f64, timing: &Timing) {
    let scale = layout.mouse_width as f32 / 38.0;
    for (cx, cy, alpha, radius) in scene.ripples(time, timing) {
        frame.stroke_circle(cx, cy, radius * scale, 3.0, dim(RIPPLE, alpha));
    }
}

/// Mouse silhouette whose buttons light up: left half, right half, wheel.
fn draw_mouse(frame: &mut Frame, scene: &Scene, layout: &Layout, x: u32, y: u32) {
    let (w, h) = (layout.mouse_width, layout.mouse_height);
    let state = |held: bool| if held { MOUSE_LIT } else { MOUSE_IDLE };
    let half = w / 2;
    let top = h / 2;

    frame.fill_round_rect(Rect { x, y, w, h }, layout.radius, MOUSE_IDLE);
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
        let layout = Layout::new(1.0);
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
        let layout = Layout::new(1.0);
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

    #[test]
    fn ripple_draws_at_the_click_position_after_the_key_faded() {
        let timing = Timing {
            fade: 0.1,
            ripple: 0.5,
        };
        let layout = Layout::new(1.0);
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
        let layout = Layout::new(1.0);
        let press = key(0x41, Action::Down, "PrintScreen");
        let scene = scene_from(&[(&press, 0.0)], &timing);
        let mut frame = Frame::new(24, 24);
        render_panel(&mut frame, &scene, &face(), &layout, 0.0, &timing);
        assert!(inked(&frame) > 0, "clipped but not empty");
    }

    #[test]
    fn layout_scales_linearly() {
        let small = Layout::new(1.0);
        let large = Layout::new(1.5);
        assert_eq!(large.cap_height, (small.cap_height as f32 * 1.5) as u32);
        assert!((large.font_px - small.font_px * 1.5).abs() < 0.01);
    }

    #[test]
    fn mouse_buttons_light_individually() {
        let timing = Timing::default();
        let layout = Layout::new(1.0);
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
