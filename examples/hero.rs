//! Render a representative deck page (screen + LED row) to `docs/preview.png`
//! for the README, using the real renderer so it always reflects the visuals.
//! Run: `cargo run --example hero`

use image::{Rgb, RgbImage};
use rustdecks::render::{icon_glyph, parse_color, Renderer, Style};

const SCALE: u32 = 2;
const KEY: u32 = 90;
const SIDE_W: u32 = 60;
const SCREEN_W: u32 = SIDE_W + 4 * KEY + SIDE_W; // 480
const SCREEN_H: u32 = 3 * KEY; // 270
const LED_H: u32 = 50;
const BG: Rgb<u8> = Rgb([10, 10, 12]);

fn main() {
    let r = Renderer::new().unwrap();
    let mut img = RgbImage::from_pixel(SCREEN_W * SCALE, (SCREEN_H + LED_H) * SCALE, BG);

    let green = parse_color("green").unwrap();
    let cyan = parse_color("cyan").unwrap();
    let s = Style::default();
    let seg = Style { seven_seg: true, ..s };
    let seg_acc = Style { seven_seg: true, accent: Some(green), ..s };

    // A showcase page: 7-seg flight readouts, lit/unlit LED annunciators,
    // a tuned frequency, and accented icon keys.
    let key = |label, value| r.key(Some(label), Some(value), &seg);
    let tiles: [Vec<u8>; 12] = [
        r.key(Some("SPD"), Some("145"), &seg_acc),
        key("HDG", "270"),
        key("ALT", "5500"),
        key("VS", "-500"),
        r.annunciator("AP", true, green, &s),
        r.annunciator("HDG", true, green, &s),
        r.annunciator("ALT", false, green, &s),
        r.icon_key(icon_glyph("engine").unwrap(), Some("ENG"), &Style { accent: Some(green), ..s }),
        key("COM1", "121.300"),
        r.annunciator("BAT 1", true, cyan, &s),
        r.annunciator("ALT 1", true, cyan, &s),
        r.icon_key(icon_glyph("transponder").unwrap(), Some("XPDR"), &Style { accent: Some(parse_color("orange").unwrap()), ..s }),
    ];
    for (i, buf) in tiles.iter().enumerate() {
        let ox = SIDE_W + (i as u32 % 4) * KEY;
        let oy = (i as u32 / 4) * KEY;
        blit565(&mut img, buf, KEY, KEY, ox, oy);
    }

    // Side strips (encoder cells).
    let left = strip(&[("THR", "85%"), ("HDG", "270"), ("QNH", "29.92")]);
    let right = strip(&[("ALT", "5500"), ("VS", "-500"), ("MIX", "100%")]);
    blit565(&mut img, &r.side_strip(&left, &s), SIDE_W, SCREEN_H, 0, 0);
    blit565(&mut img, &r.side_strip(&right, &s), SIDE_W, SCREEN_H, SIDE_W + 4 * KEY, 0);

    // Round nav LEDs (b0..b7), colors matching the SR22 pager.
    let leds = ["orange", "green", "blue", "red", "yellow", "green", "red", "orange"];
    for (i, name) in leds.iter().enumerate() {
        let cx = SCREEN_W * (i as u32 * 2 + 1) / 16;
        disk(&mut img, cx, SCREEN_H + LED_H / 2, 9, parse_color(name).unwrap());
    }

    std::fs::create_dir_all("docs").unwrap();
    img.save("docs/preview.png").unwrap();
    println!("wrote docs/preview.png ({}x{})", img.width(), img.height());
}

fn strip(cells: &[(&str, &str); 3]) -> [Option<(String, String)>; 3] {
    std::array::from_fn(|i| Some((cells[i].0.to_string(), cells[i].1.to_string())))
}

/// Unpack an RGB565-LE tile and draw it scaled into the big image at (ox, oy).
fn blit565(img: &mut RgbImage, buf: &[u8], w: u32, h: u32, ox: u32, oy: u32) {
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 2) as usize;
            let v = buf[i] as u16 | ((buf[i + 1] as u16) << 8);
            let rgb = Rgb([
                (((v >> 11) & 0x1f) << 3) as u8,
                (((v >> 5) & 0x3f) << 2) as u8,
                ((v & 0x1f) << 3) as u8,
            ]);
            for dy in 0..SCALE {
                for dx in 0..SCALE {
                    img.put_pixel((ox + x) * SCALE + dx, (oy + y) * SCALE + dy, rgb);
                }
            }
        }
    }
}

/// Fill a scaled disk centered at native (cx, cy).
fn disk(img: &mut RgbImage, cx: u32, cy: u32, rad: u32, color: [u8; 3]) {
    let r2 = (rad * rad) as i32;
    for dy in -(rad as i32)..=rad as i32 {
        for dx in -(rad as i32)..=rad as i32 {
            if dx * dx + dy * dy > r2 {
                continue;
            }
            let (px, py) = ((cx as i32 + dx) * SCALE as i32, (cy as i32 + dy) * SCALE as i32);
            for sy in 0..SCALE as i32 {
                for sx in 0..SCALE as i32 {
                    img.put_pixel((px + sx) as u32, (py + sy) as u32, Rgb(color));
                }
            }
        }
    }
}
