//! Render sample tiles to PNGs so the rendering can be eyeballed without the
//! hardware. Run: `cargo run --example preview` -> writes /tmp/pg-shots/*.png
//! (each scaled 4x for visibility).

use image::RgbImage;
use rustdecks::render::{icon_glyph, Renderer, Style};

const SCALE: u32 = 4;

fn save(name: &str, w: u32, h: u32, rgb565: &[u8]) {
    let mut img = RgbImage::new(w * SCALE, h * SCALE);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 2) as usize;
            let v = rgb565[i] as u16 | ((rgb565[i + 1] as u16) << 8);
            let r = (((v >> 11) & 0x1f) << 3) as u8;
            let g = (((v >> 5) & 0x3f) << 2) as u8;
            let b = ((v & 0x1f) << 3) as u8;
            for dy in 0..SCALE {
                for dx in 0..SCALE {
                    img.put_pixel(x * SCALE + dx, y * SCALE + dy, image::Rgb([r, g, b]));
                }
            }
        }
    }
    img.save(name).unwrap();
}

fn main() {
    let r = Renderer::new().unwrap();
    let s = Style::default();
    let dir = "/tmp/pg-shots";
    std::fs::create_dir_all(dir).unwrap();

    let strip = [
        Some(("THR".to_string(), "100%".to_string())),
        Some(("HDG".to_string(), "360".to_string())),
        Some(("QNH".to_string(), "29.92".to_string())),
    ];
    save(&format!("{dir}/strip.png"), 60, 270, &r.side_strip(&strip, &s));
    save(&format!("{dir}/key_alt.png"), 90, 90, &r.key(Some("ALT"), Some("5500"), &s));
    save(&format!("{dir}/key_com.png"), 90, 90, &r.key(Some("ACTIVE"), Some("121.300"), &s));
    save(&format!("{dir}/ann_on.png"), 90, 90, &r.annunciator("BAT 1", true, [40, 200, 60], &s));
    save(&format!("{dir}/ann_off.png"), 90, 90, &r.annunciator("BAT 1", false, [40, 200, 60], &s));
    let glyph = icon_glyph("engine").unwrap();
    save(&format!("{dir}/icon.png"), 90, 90, &r.icon_key(glyph, Some("ENG"), &s));

    // New visuals: 7-segment readout, and an accent-barred key.
    let seg = Style { seven_seg: true, ..s };
    save(&format!("{dir}/key_seg.png"), 90, 90, &r.key(Some("STBY"), Some("121.300"), &seg));
    save(&format!("{dir}/key_squawk.png"), 90, 90, &r.key(Some("SQUAWK"), Some("1200"), &seg));
    let accent = Style { accent: Some([40, 200, 60]), ..s };
    save(&format!("{dir}/key_accent.png"), 90, 90, &r.key(Some("SPD"), Some("145"), &accent));
    let icon_accent = Style { accent: Some([230, 210, 40]), ..s };
    save(&format!("{dir}/icon_accent.png"), 90, 90, &r.icon_key(glyph, Some("ENG"), &icon_accent));

    println!("wrote {dir}/*.png");
}
