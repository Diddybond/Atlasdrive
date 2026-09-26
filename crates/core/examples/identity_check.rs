//! Check the face identity model on a real photograph.
//!
//! `cargo run --release --example identity_check -- photo.jpg`
//!
//! Finds every face, cuts each out the way the catalogue stores face crops
//! (a 45% margin, at most 200px), embeds the crop, and prints how similar every
//! pair of faces is. Different people should score low; each face against a
//! darkened, slightly shrunk copy of its own crop should score high.
use family_archive_core::ai::identity::{similarity, IdentityModel};
use image::RgbImage;

fn crop(img: &RgbImage, b: [f32; 4]) -> RgbImage {
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let (hw, hh) = ((b[2] - b[0]) * 1.45 / 2.0, (b[3] - b[1]) * 1.45 / 2.0);
    let x0 = (cx - hw).max(0.0) as u32;
    let y0 = (cy - hh).max(0.0) as u32;
    let x1 = ((cx + hw) as u32).min(img.width());
    let y1 = ((cy + hh) as u32).min(img.height());
    let c = image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image();
    let s = 200.0 / c.width().max(c.height()) as f32;
    if s < 1.0 {
        image::imageops::resize(&c, (c.width() as f32 * s) as u32, (c.height() as f32 * s) as u32, image::imageops::FilterType::Lanczos3)
    } else {
        c
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("photo path");
    let dir = IdentityModel::find(&[]).expect("model not installed");
    let model = IdentityModel::load(&dir).unwrap();
    let img = image::open(&path).unwrap().to_rgb8();
    let dets = model.detect(&img).unwrap();
    println!("{} faces", dets.len());
    let t = std::time::Instant::now();
    let mut embs = Vec::new();
    for (i, d) in dets.iter().enumerate() {
        let c = crop(&img, d.bbox);
        let e = model.embed_crop(&c).unwrap();
        let smaller = image::imageops::resize(&c, c.width() * 3 / 4, c.height() * 3 / 4, image::imageops::FilterType::Triangle);
        let altered = image::imageops::colorops::brighten(&smaller, -30);
        let e2 = model.embed_crop(&altered).unwrap();
        match (&e, &e2) {
            (Some(a), Some(b)) => println!("face {i} score {:.2} box {:?}: self-match after changes {:.2}", d.score, d.bbox.map(|v| v as i32), similarity(a, b)),
            _ => println!("face {i}: not found in its crop"),
        }
        embs.push(e);
    }
    println!("embedding time {:?} per face (x2 per face above)", t.elapsed() / (2 * dets.len().max(1) as u32));
    for i in 0..embs.len() {
        for j in i + 1..embs.len() {
            if let (Some(a), Some(b)) = (&embs[i], &embs[j]) {
                println!("  face {i} vs {j}: {:.2}", similarity(a, b));
            }
        }
    }
}
