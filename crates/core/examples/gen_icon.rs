//! Generate a simple placeholder app icon PNG. On macOS run
//! `cargo tauri icon src-tauri/icons/icon.png` to expand it into the full set.

use image::{Rgba, RgbaImage};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "src-tauri/icons/icon.png".into());
    let size = 1024u32;
    let mut img = RgbaImage::new(size, size);
    let cx = size as f32 / 2.0;
    let cy = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            // Rounded background gradient (deep blue → slate).
            let t = y as f32 / size as f32;
            let r = (30.0 * (1.0 - t) + 58.0 * t) as u8;
            let g = (52.0 * (1.0 - t) + 90.0 * t) as u8;
            let b = (110.0 * (1.0 - t) + 155.0 * t) as u8;
            img.put_pixel(x, y, Rgba([r, g, b, 255]));
        }
    }
    // A simple diamond mark in the centre.
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 - cx).abs();
            let dy = (y as f32 - cy).abs();
            if dx + dy < size as f32 * 0.28 && dx + dy > size as f32 * 0.20 {
                img.put_pixel(x, y, Rgba([245, 245, 250, 255]));
            }
        }
    }
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    img.save(&out).unwrap();
    println!("Wrote {out}");
}
