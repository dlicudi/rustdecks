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
/// FontAwesome 6 Free (Solid) for icon glyphs on nav/menu keys.
const ICON_FONT: &[u8] = include_bytes!("../assets/fonts/FontAwesome6-Solid.otf");
/// Segment7 (SIL OFL) — a 7-segment face for authentic avionics readouts.
const SEG_FONT: &[u8] = include_bytes!("../assets/fonts/Segment7Standard.otf");

/// Tile frame and accent-bar geometry, shared by every key.
const FRAME_COLOR: [u8; 3] = [60, 62, 70];
const ACCENT_H: usize = 5;

/// Visual style for a rendered cell. Cockpit defaults: gold label, cyan value,
/// near-black background.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub label_color: [u8; 3],
    pub value_color: [u8; 3],
    pub bg_color: [u8; 3],
    pub label_size: f32,
    pub value_size: f32,
    /// Optional accent bar painted across the top of the key.
    pub accent: Option<[u8; 3]>,
    /// Render the value in the 7-segment face.
    pub seven_seg: bool,
    /// Draw a 1px frame around the tile.
    pub border: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            label_color: [228, 190, 70],
            value_color: [120, 210, 235],
            bg_color: [22, 22, 26],
            label_size: 15.0,
            value_size: 27.0,
            accent: None,
            seven_seg: false,
            border: true,
        }
    }
}

pub struct Renderer {
    label: Font,
    value: Font,
    icons: Font,
    seg: Font,
}

impl Renderer {
    pub fn new() -> Result<Renderer, String> {
        let opts = fontdue::FontSettings::default();
        Ok(Renderer {
            label: Font::from_bytes(LABEL_FONT, opts).map_err(|e| e.to_string())?,
            value: Font::from_bytes(VALUE_FONT, opts).map_err(|e| e.to_string())?,
            icons: Font::from_bytes(ICON_FONT, opts).map_err(|e| e.to_string())?,
            seg: Font::from_bytes(SEG_FONT, opts).map_err(|e| e.to_string())?,
        })
    }

    /// Render one 90x90 center key: optional accent bar, label near the top,
    /// value in the middle (7-segment face when `style.seven_seg`), framed.
    pub fn key(&self, label: Option<&str>, value: Option<&str>, style: &Style) -> Vec<u8> {
        let mut c = Canvas::new(KEY_SIZE as usize, KEY_SIZE as usize, style.bg_color);
        let top = self.accent(&mut c, style);
        if let Some(text) = label {
            self.line(&mut c, &self.label, text, style.label_size, style.label_color, top + 16);
        }
        if let Some(text) = value {
            let font = if style.seven_seg { &self.seg } else { &self.value };
            self.line(&mut c, font, text, style.value_size, style.value_color, 56);
        }
        self.finish(c, style)
    }

    /// Render a 90x90 icon key: a large centered glyph with an optional small
    /// label beneath. Falls back to a plain text key if the font lacks the glyph.
    pub fn icon_key(&self, glyph: char, label: Option<&str>, style: &Style) -> Vec<u8> {
        if self.icons.lookup_glyph_index(glyph) == 0 {
            return self.key(label, None, style); // missing glyph -> show the label
        }
        let mut c = Canvas::new(KEY_SIZE as usize, KEY_SIZE as usize, style.bg_color);
        self.accent(&mut c, style);
        let icon_baseline = if label.is_some() { 50 } else { 58 };
        self.line(&mut c, &self.icons, &glyph.to_string(), 46.0, [216, 222, 230], icon_baseline);
        if let Some(text) = label {
            self.line(&mut c, &self.label, text, 13.0, style.label_color, 82);
        }
        self.finish(c, style)
    }

    /// Render a 90x90 annunciator key: a dark tile with a small LED bar near the
    /// top that glows `color` when `lit` (dim when not) and the label below in
    /// white. The classic cockpit indicator look, instead of a full-cell flood.
    pub fn annunciator(&self, label: &str, lit: bool, color: [u8; 3], style: &Style) -> Vec<u8> {
        let mut c = Canvas::new(KEY_SIZE as usize, KEY_SIZE as usize, [16, 16, 18]);
        let (bar, fg) = if lit {
            (color, [235, 235, 235])
        } else {
            (dim(color), [120, 120, 120])
        };
        c.fill_rect(16, 14, KEY_SIZE as usize - 16, 28, bar);
        self.line(&mut c, &self.label, label, style.label_size + 3.0, fg, 62);
        self.finish(c, style)
    }

    /// Paint the optional accent bar; return the y where content should start.
    fn accent(&self, c: &mut Canvas, style: &Style) -> i32 {
        match style.accent {
            Some(rgb) => {
                c.fill_rect(0, 0, c.w, ACCENT_H, rgb);
                ACCENT_H as i32 + 4
            }
            None => 0,
        }
    }

    /// Draw the tile frame (if enabled) and pack to the device format.
    fn finish(&self, mut c: Canvas, style: &Style) -> Vec<u8> {
        if style.border {
            c.frame(FRAME_COLOR);
        }
        c.to_rgb565_le()
    }

    /// Render a 60x270 side strip from up to three (label, value) cells, stacked.
    /// Cells are only 60px wide, so use smaller type than the 90px keys.
    pub fn side_strip(&self, cells: &[Option<(String, String)>; 3], style: &Style) -> Vec<u8> {
        const STRIP_LABEL: f32 = 13.0;
        const STRIP_VALUE: f32 = 14.0;
        let mut c = Canvas::new(SIDE_W as usize, SIDE_H as usize, style.bg_color);
        let cell_h = SIDE_H as i32 / 3; // 90
        for (i, cell) in cells.iter().enumerate() {
            let Some((label, value)) = cell else { continue };
            let base = i as i32 * cell_h;
            self.line(&mut c, &self.label, label, STRIP_LABEL, style.label_color, base + 30);
            self.line(&mut c, &self.value, value, STRIP_VALUE, style.value_color, base + 58);
        }
        c.to_rgb565_le()
    }

    /// Draw one horizontally-centered line of text with its baseline at `baseline`,
    /// shrinking the size if the text would otherwise overflow the tile width.
    fn line(&self, c: &mut Canvas, font: &Font, text: &str, size: f32, color: [u8; 3], baseline: i32) {
        let avail = c.w as f32 - 6.0;
        let width = |sz: f32| text.chars().map(|ch| font.metrics(ch, sz).advance_width).sum::<f32>();
        let size = match width(size) {
            w if w > avail && w > 0.0 => size * avail / w,
            _ => size,
        };
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

    /// Fill the half-open rect [x0,x1) x [y0,y1) with a solid color (clamped).
    fn fill_rect(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, color: [u8; 3]) {
        for y in y0..y1.min(self.h) {
            for x in x0..x1.min(self.w) {
                self.px[y * self.w + x] = color;
            }
        }
    }

    /// Draw a 1px frame around the canvas edge.
    fn frame(&mut self, color: [u8; 3]) {
        for x in 0..self.w {
            self.px[x] = color;
            self.px[(self.h - 1) * self.w + x] = color;
        }
        for y in 0..self.h {
            self.px[y * self.w] = color;
            self.px[y * self.w + self.w - 1] = color;
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

/// Parse `#rgb`, `#rrggbb`, or a small set of named colors.
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return match hex.len() {
            6 => Some([hex2(hex, 0)?, hex2(hex, 2)?, hex2(hex, 4)?]),
            3 => {
                let d = |i| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|v| v * 17);
                Some([d(0)?, d(1)?, d(2)?])
            }
            _ => None,
        };
    }
    Some(match s.to_ascii_lowercase().as_str() {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [220, 40, 40],
        "green" => [40, 200, 60],
        "blue" => [40, 90, 220],
        "yellow" => [230, 210, 40],
        "orange" => [240, 150, 30],
        "cyan" => [40, 200, 220],
        "deepskyblue" | "skyblue" => [0, 191, 255],
        "gray" | "grey" => [128, 128, 128],
        _ => return None,
    })
}

fn hex2(hex: &str, i: usize) -> Option<u8> {
    u8::from_str_radix(&hex[i..i + 2], 16).ok()
}

/// Resolve an icon name (or a raw hex codepoint like `"f085"`) to a glyph.
/// Names map to the same FontAwesome codepoints cockpitdecks uses.
pub fn icon_glyph(name: &str) -> Option<char> {
    let cp = match name {
        "pfi" | "gauge" => 0xf3fd,
        "switches" | "toggle" => 0xf204,
        "icing" | "snowflake" => 0xf2dc,
        "weather" | "cloud" => 0xf0c2,
        "autopilot" | "ap" => 0xe22d,
        "gcu" | "chip" => 0xf2db,
        "radio" => 0xe585,
        "engine" | "gears" => 0xf085,
        "views" | "camera" => 0xf030,
        "transponder" | "xpdr" => 0xf8d7,
        "fms" | "map" => 0xf279,
        hex => u32::from_str_radix(hex.trim_start_matches("u"), 16).ok()?,
    };
    char::from_u32(cp)
}

/// Darken a color to roughly an unlit-annunciator shade.
fn dim(c: [u8; 3]) -> [u8; 3] {
    [
        (c[0] as f32 * 0.22) as u8,
        (c[1] as f32 * 0.22) as u8,
        (c[2] as f32 * 0.22) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_parsing() {
        assert_eq!(parse_color("#00ff00"), Some([0, 255, 0]));
        assert_eq!(parse_color("#0f0"), Some([0, 255, 0]));
        assert_eq!(parse_color("red"), Some([220, 40, 40]));
        assert_eq!(parse_color("nonsense"), None);
    }

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
    fn icon_glyph_resolves_names_and_hex() {
        assert_eq!(icon_glyph("engine"), Some('\u{f085}'));
        assert_eq!(icon_glyph("f085"), Some('\u{f085}'));
        assert_eq!(icon_glyph("uf085"), Some('\u{f085}'));
        assert_eq!(icon_glyph("not-a-glyph"), None);
    }

    #[test]
    fn icon_key_size_and_fallback() {
        let r = Renderer::new().unwrap();
        let style = Style::default();
        // A present glyph renders an icon; correct buffer size.
        let icon = r.icon_key('\u{f085}', Some("ENG"), &style);
        assert_eq!(icon.len(), 90 * 90 * 2);
        // A missing glyph falls back to the text key (still valid output).
        let missing = r.icon_key('\u{1}', Some("ENG"), &style);
        assert_eq!(missing, r.key(Some("ENG"), None, &style));
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
