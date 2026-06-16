//! Rendering: compose key (90x90) and side-strip (60x270) images and pack them
//! into the device's native RGB565 little-endian format.
//!
//! Deliberately tiny: an in-house RGB888 canvas plus `fontdue` for glyphs. No
//! shape/path library — a cell is a background fill plus one or two text lines.
//! Fonts are the bundled B612 family (designed for cockpit displays).

use fontdue::Font;

use crate::device::{KEY_SIZE, SIDE_H, SIDE_W};

/// Proportional face for labels; monospace face for values (numbers align).
const LABEL_FONT: &[u8] = include_bytes!("../assets/fonts/B612-Regular.ttf");
const VALUE_FONT: &[u8] = include_bytes!("../assets/fonts/B612Mono-Regular.ttf");

/// Visual style for a rendered cell.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub text_color: [u8; 3],
    pub bg_color: [u8; 3],
    pub label_size: f32,
    pub value_size: f32,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            text_color: [230, 230, 230],
            bg_color: [16, 16, 16],
            label_size: 15.0,
            value_size: 24.0,
        }
    }
}

pub struct Renderer {
    label: Font,
    value: Font,
}

impl Renderer {
    pub fn new() -> Result<Renderer, String> {
        let opts = fontdue::FontSettings::default();
        Ok(Renderer {
            label: Font::from_bytes(LABEL_FONT, opts).map_err(|e| e.to_string())?,
            value: Font::from_bytes(VALUE_FONT, opts).map_err(|e| e.to_string())?,
        })
    }

    /// Render one 90x90 center key: label near the top, value in the middle.
    pub fn key(&self, label: Option<&str>, value: Option<&str>, style: &Style) -> Vec<u8> {
        let mut c = Canvas::new(KEY_SIZE as usize, KEY_SIZE as usize, style.bg_color);
        if let Some(text) = label {
            self.line(&mut c, &self.label, text, style.label_size, style.text_color, 18);
        }
        if let Some(text) = value {
            self.line(&mut c, &self.value, text, style.value_size, style.text_color, 56);
        }
        c.to_rgb565_le()
    }

    /// Render a 60x270 side strip from up to three (label, value) cells, stacked.
    pub fn side_strip(&self, cells: &[Option<(String, String)>; 3], style: &Style) -> Vec<u8> {
        let mut c = Canvas::new(SIDE_W as usize, SIDE_H as usize, style.bg_color);
        let cell_h = SIDE_H as i32 / 3; // 90
        for (i, cell) in cells.iter().enumerate() {
            let Some((label, value)) = cell else { continue };
            let base = i as i32 * cell_h;
            self.line(&mut c, &self.label, label, style.label_size, style.text_color, base + 28);
            self.line(&mut c, &self.value, value, style.value_size, style.text_color, base + 62);
        }
        c.to_rgb565_le()
    }

    /// Draw one horizontally-centered line of text with its baseline at `baseline`.
    fn line(&self, c: &mut Canvas, font: &Font, text: &str, size: f32, color: [u8; 3], baseline: i32) {
        let glyphs: Vec<_> = text
            .chars()
            .map(|ch| (ch, font.metrics(ch, size)))
            .collect();
        let total: f32 = glyphs.iter().map(|(_, m)| m.advance_width).sum();
        let mut pen = ((c.w as f32 - total) / 2.0).max(0.0);
        for (ch, m) in glyphs {
            let (metrics, bitmap) = font.rasterize(ch, size);
            let gx = (pen + metrics.xmin as f32).round() as i32;
            let gy = baseline - metrics.ymin as i32 - metrics.height as i32;
            c.blit(&bitmap, metrics.width, metrics.height, gx, gy, color);
            pen += m.advance_width;
        }
    }
}

/// A small RGB888 drawing surface.
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[u8; 3]>,
}

impl Canvas {
    fn new(w: usize, h: usize, bg: [u8; 3]) -> Canvas {
        Canvas { w, h, px: vec![bg; w * h] }
    }

    /// Alpha-blend a fontdue coverage bitmap toward `color` at (x, y).
    fn blit(&mut self, cov: &[u8], cw: usize, ch: usize, x: i32, y: i32, color: [u8; 3]) {
        for row in 0..ch {
            let py = y + row as i32;
            if py < 0 || py as usize >= self.h {
                continue;
            }
            for col in 0..cw {
                let px = x + col as i32;
                if px < 0 || px as usize >= self.w {
                    continue;
                }
                let a = cov[row * cw + col] as u16;
                if a == 0 {
                    continue;
                }
                let dst = &mut self.px[py as usize * self.w + px as usize];
                for ch in 0..3 {
                    let src = color[ch] as u16;
                    let bg = dst[ch] as u16;
                    dst[ch] = ((src * a + bg * (255 - a)) / 255) as u8;
                }
            }
        }
    }

    /// Pack to RGB565 little-endian (RRRRR GGGGGG BBBBB), 2 bytes per pixel.
    fn to_rgb565_le(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 2);
        for &[r, g, b] in &self.px {
            let v = ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3);
            out.push(v as u8);
            out.push((v >> 8) as u8);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_buffer_has_correct_size() {
        let r = Renderer::new().unwrap();
        let buf = r.key(Some("GEN"), Some("123"), &Style::default());
        assert_eq!(buf.len(), 90 * 90 * 2);
    }

    #[test]
    fn side_strip_has_correct_size() {
        let r = Renderer::new().unwrap();
        let cells = [
            Some(("THR".into(), "80%".into())),
            None,
            Some(("HDG".into(), "270".into())),
        ];
        let buf = r.side_strip(&cells, &Style::default());
        assert_eq!(buf.len(), 60 * 270 * 2);
    }

    #[test]
    fn text_actually_draws_pixels() {
        // A labelled key must differ from a blank one of the same background.
        let r = Renderer::new().unwrap();
        let style = Style::default();
        let blank = r.key(None, None, &style);
        let text = r.key(Some("ABC"), None, &style);
        assert_ne!(blank, text, "rendering text should change the buffer");
    }
}
