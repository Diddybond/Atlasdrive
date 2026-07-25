//! CLI integration tests: exercise the real binaries and assert the stable exit
//! codes and end-to-end behaviour (register → index → search → verify).

use std::path::Path;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_family-archive")
}
fn verify_bin() -> &'static str {
    env!("CARGO_BIN_EXE_family-archive-verify")
}

fn write_png(path: &Path, color: [u8; 3]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = image::RgbImage::from_pixel(64, 48, image::Rgb(color));
    img.save(path).unwrap();
}

#[test]
fn register_index_search_verify_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let drive = tmp.path().join("DriveVol");
    write_png(&drive.join("a/red_1990.png"), [200, 30, 30]);
    write_png(&drive.join("b/blue.png"), [30, 30, 200]);

    // register (exit 0)
    let out = Command::new(bin())
        .args(["--home", home.to_str().unwrap(), "drive", "register", "--number", "14", "--path", drive.to_str().unwrap(), "--name", "Test"])
        .output()
        .unwrap();
    assert!(out.status.success(), "register failed: {}", String::from_utf8_lossy(&out.stderr));

    // duplicate number → exit 2 (invalid args)
    let dup = Command::new(bin())
        .args(["--home", home.to_str().unwrap(), "drive", "register", "--number", "14", "--path", drive.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(dup.status.code(), Some(2), "duplicate drive number should exit 2");

    // index (exit 0) with disk floor disabled for CI
    let idx = Command::new(bin())
        .args(["--home", home.to_str().unwrap(), "index", "--drive", "14", "--path", drive.to_str().unwrap(), "--free-space-floor", "0", "--batch-size", "1"])
        .output()
        .unwrap();
    assert!(idx.status.success(), "index failed: {}", String::from_utf8_lossy(&idx.stderr));

    // index of an unregistered drive → invalid args exit 2
    let bad = Command::new(bin())
        .args(["--home", home.to_str().unwrap(), "index", "--drive", "99", "--path", drive.to_str().unwrap(), "--free-space-floor", "0"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));

    // search returns the indexed file
    let search = Command::new(bin())
        .args(["--home", home.to_str().unwrap(), "search", "red", "--offline-included"])
        .output()
        .unwrap();
    assert!(search.status.success());
    let text = String::from_utf8_lossy(&search.stdout);
    assert!(text.contains("red_1990.png"), "search output: {text}");

    // standalone verifier → exit 0
    let verify = Command::new(verify_bin())
        .args(["--home", home.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(verify.status.success(), "verify failed: {}", String::from_utf8_lossy(&verify.stdout));
    assert_eq!(verify.status.code(), Some(0));
}

#[test]
fn verifier_exits_nonzero_on_corrupt_thumbnail() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let drive = tmp.path().join("DriveVol");
    write_png(&drive.join("x.png"), [10, 200, 10]);

    Command::new(bin())
        .args(["--home", home.to_str().unwrap(), "drive", "register", "--number", "3", "--path", drive.to_str().unwrap()])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["--home", home.to_str().unwrap(), "index", "--drive", "3", "--path", drive.to_str().unwrap(), "--free-space-floor", "0"])
        .output()
        .unwrap();

    // Corrupt a thumbnail on disk.
    let thumbs = home.join("thumbnails");
    let mut corrupted = false;
    for entry in walkdir::WalkDir::new(&thumbs) {
        let entry = entry.unwrap();
        if entry.file_type().is_file() {
            std::fs::write(entry.path(), b"not a real png").unwrap();
            corrupted = true;
        }
    }
    assert!(corrupted, "expected at least one thumbnail to corrupt");

    // Verifier must now exit non-zero.
    let verify = Command::new(verify_bin())
        .args(["--home", home.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!verify.status.success(), "verifier must fail on corrupt thumbnail");
    assert_ne!(verify.status.code(), Some(0));
}
