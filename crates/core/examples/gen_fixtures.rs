//! Generate synthetic test images (never real photographs) for CLI smoke tests
//! and demos. Usage: `cargo run -p family-archive-core --example gen_fixtures -- <dir>`

use image::{Rgb, RgbImage};

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "./fixtures-drive".into());
    let base = std::path::Path::new(&dir);

    // A blue "outdoor" scene.
    save(&base.join("holiday/beach_1998.png"), gradient([20, 90, 200], [200, 220, 255], 200, 150));
    // A warm "indoor" scene with a skin-tone block (a synthetic "face").
    let mut portrait = solid([70, 55, 45], 200, 150);
    for y in 40..100 {
        for x in 70..130 {
            portrait.put_pixel(x, y, Rgb([205, 160, 130]));
        }
    }
    save(&base.join("family/portrait_1987.png"), portrait);
    // A faded, near-grayscale "scanned print" with a bright border.
    let mut scan = solid([180, 176, 170], 220, 160);
    for x in 0..220 {
        for t in 0..8 {
            scan.put_pixel(x, t, Rgb([250, 250, 250]));
            scan.put_pixel(x, 159 - t, Rgb([250, 250, 250]));
        }
    }
    for y in 0..160 {
        for t in 0..8 {
            scan.put_pixel(t, y, Rgb([250, 250, 250]));
            scan.put_pixel(219 - t, y, Rgb([250, 250, 250]));
        }
    }
    save(&base.join("scans/old_scan.png"), scan);

    println!("Wrote synthetic fixtures under {}", base.display());
}

fn solid(c: [u8; 3], w: u32, h: u32) -> RgbImage {
    RgbImage::from_pixel(w, h, Rgb(c))
}

fn gradient(a: [u8; 3], b: [u8; 3], w: u32, h: u32) -> RgbImage {
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        let t = y as f32 / h as f32;
        let px = Rgb([
            (a[0] as f32 * (1.0 - t) + b[0] as f32 * t) as u8,
            (a[1] as f32 * (1.0 - t) + b[1] as f32 * t) as u8,
            (a[2] as f32 * (1.0 - t) + b[2] as f32 * t) as u8,
        ]);
        for x in 0..w {
            img.put_pixel(x, y, px);
        }
    }
    img
}

fn save(path: &std::path::Path, img: RgbImage) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    img.save(path).unwrap();
}
