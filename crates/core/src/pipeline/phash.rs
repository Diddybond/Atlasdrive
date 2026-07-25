//! Perceptual hashing (dHash) for duplicate and near-duplicate detection.
//!
//! A 64-bit difference hash computed from a 9×8 grayscale downscale. Hamming
//! distance between two hashes measures visual similarity. Deterministic.

use image::RgbImage;

/// Compute a 64-bit dHash, returned as a 16-char hex string.
pub fn dhash(img: &RgbImage) -> String {
    let small = image::imageops::resize(img, 9, 8, image::imageops::FilterType::Triangle);
    let mut bits: u64 = 0;
    let mut bit = 0;
    for y in 0..8u32 {
        for x in 0..8u32 {
            let l = luma(small.get_pixel(x, y));
            let r = luma(small.get_pixel(x + 1, y));
            if l < r {
                bits |= 1 << bit;
            }
            bit += 1;
        }
    }
    format!("{bits:016x}")
}

/// Hamming distance between two hex dHash strings (0 = identical). Returns 64
/// (max distance) if either string is malformed.
pub fn hamming(a: &str, b: &str) -> u32 {
    match (u64::from_str_radix(a, 16), u64::from_str_radix(b, 16)) {
        (Ok(x), Ok(y)) => (x ^ y).count_ones(),
        _ => 64,
    }
}

fn luma(p: &image::Rgb<u8>) -> u32 {
    // Rec. 601 luma.
    (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn identical_images_zero_distance() {
        let img = RgbImage::from_pixel(32, 32, Rgb([100, 120, 140]));
        let a = dhash(&img);
        let b = dhash(&img);
        assert_eq!(a, b);
        assert_eq!(hamming(&a, &b), 0);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn gradient_differs_from_solid() {
        let solid = RgbImage::from_pixel(32, 32, Rgb([10, 10, 10]));
        let mut grad = RgbImage::new(32, 32);
        for x in 0..32u32 {
            for y in 0..32u32 {
                let v = (x * 8) as u8;
                grad.put_pixel(x, y, Rgb([v, v, v]));
            }
        }
        assert!(hamming(&dhash(&solid), &dhash(&grad)) > 0);
    }
}
