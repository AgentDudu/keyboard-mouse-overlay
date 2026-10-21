//! Glyph rasterising into a [`Frame`] with `ab_glyph`.
//!
//! One font is embedded at build time (`assets/IBMPlexMono-Bold.ttf`, OFL 1.1, 134 KB) so the
//! overlay never depends on a font being installed or on font-dir scanning.

use ab_glyph::{Font, FontRef, point};

use crate::overlay::{Frame, Rgba};

pub const FONT_BYTES: &[u8] = include_bytes!("../assets/IBMPlexMono-Bold.ttf");

/// A loaded face plus the pre-scaled metrics every label needs.
#[derive(Debug)]
pub struct Face<'font> {
    font: FontRef<'font>,
    units_per_em: f32,
}

impl<'font> Face<'font> {
    pub fn new(bytes: &'font [u8]) -> Result<Self, String> {
        let font = FontRef::try_from_slice(bytes).map_err(|err| format!("bad font: {err}"))?;
        let units_per_em = font.units_per_em().unwrap_or(1000.0);
        Ok(Self { font, units_per_em })
    }

    /// Distance from baseline to cap top, used to centre text vertically in a keycap.
    pub fn ascent(&self, pixel: f32) -> f32 {
        self.font.ascent_unscaled() * pixel / self.units_per_em
    }

    /// Advance width of `text` at `pixel` size. Kerning is skipped: the bundled face is mono.
    ///
    /// `shortcut:` no kerning, mono face assumed; add `kern_unscaled` if a proportional font
    /// ever replaces it.
    pub fn width(&self, text: &str, pixel: f32) -> f32 {
        text.chars()
            .map(|ch| self.font.h_advance_unscaled(self.font.glyph_id(ch)))
            .sum::<f32>()
            * pixel
            / self.units_per_em
    }

    /// Draw `text` with its baseline at `baseline`, blending onto whatever is already there.
    ///
    /// Returns the right edge so callers can lay out the next label without re-measuring.
    pub fn draw(
        &self,
        frame: &mut Frame,
        x: i32,
        baseline: i32,
        pixel: f32,
        text: &str,
        colour: Rgba,
    ) -> i32 {
        let mut pen = x as f32;
        for ch in text.chars() {
            let id = self.font.glyph_id(ch);
            let glyph = id.with_scale_and_position(pixel, point(pen, baseline as f32));
            if let Some(outline) = self.font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                let left = bounds.min.x.floor();
                let top = bounds.min.y.floor();
                outline.draw(|column, row, coverage| {
                    if coverage <= 0.0 {
                        return;
                    }
                    let px = left + column as f32;
                    let py = top + row as f32;
                    frame.over(
                        px as i32,
                        py as i32,
                        Rgba {
                            r: colour.r,
                            g: colour.g,
                            b: colour.b,
                            a: (coverage * colour.a as f32) as u8,
                        },
                    );
                });
            }
            pen += self.font.h_advance_unscaled(id) * pixel / self.units_per_em;
        }
        pen.ceil() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::Rect;

    const WHITE: Rgba = Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };

    fn face() -> Face<'static> {
        Face::new(FONT_BYTES).expect("bundled font parses")
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

    #[test]
    fn font_loads_and_has_usable_metrics() {
        let face = face();
        assert!(
            (face.ascent(32.0) - 24.0).abs() < 12.0,
            "ascent sanity: {}",
            face.ascent(32.0)
        );
        let wide = face.width("WWWW", 32.0);
        let narrow = face.width("W", 32.0);
        assert!(
            narrow > 0.0 && (wide - narrow * 4.0).abs() < 1.0,
            "mono advances: {wide} {narrow}"
        );
    }

    #[test]
    fn bad_font_bytes_are_an_error_not_a_panic() {
        assert!(Face::new(&[0, 1, 2, 3]).is_err());
    }

    #[test]
    fn drawing_paints_ink_and_returns_the_right_edge() {
        let face = face();
        let mut frame = Frame::new(220, 48);
        let right = face.draw(&mut frame, 4, 36, 32.0, "A", WHITE);
        assert!(inked(&frame) > 50, "glyph produced no coverage");
        assert!(right > 4 && right < 60, "right edge {right}");
        assert!(face.width("A", 32.0) > 0.0);
    }

    #[test]
    fn empty_text_paints_nothing() {
        let face = face();
        let mut frame = Frame::new(40, 40);
        let right = face.draw(&mut frame, 5, 20, 20.0, "", WHITE);
        assert_eq!(right, 5);
        assert_eq!(inked(&frame), 0);
    }

    #[test]
    fn glyphs_running_off_the_edge_are_clipped_without_panicking() {
        let face = face();
        let mut frame = Frame::new(30, 30);
        face.draw(&mut frame, 26, 24, 28.0, "Windows", WHITE);
        face.draw(&mut frame, -20, 24, 28.0, "Ctrl", WHITE);
        assert!(inked(&frame) > 0, "still drew what fit");
    }

    #[test]
    fn translucent_text_lands_on_top_of_existing_pixels() {
        let face = face();
        let mut frame = Frame::new(80, 40);
        frame.fill_rect(
            Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 40,
            },
            Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 90,
            },
        );
        let before = inked(&frame);
        face.draw(&mut frame, 2, 30, 26.0, "F13", WHITE);
        assert!(
            inked(&frame) >= before,
            "text composites, never erases the panel"
        );
    }
}
