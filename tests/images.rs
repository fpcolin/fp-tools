//! Image codec checks against reference decodes made with Pillow, plus
//! robustness against damaged files. The full corpus comparison (hundreds of
//! files, every PNG colour type and bit depth, every JPEG subsampling) was run
//! during development; these keep the essentials under CI.

use fp_tools::image::{self, DecodeError, Limits};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn rgb_sha256(img: &image::Image) -> String {
    let rgb: Vec<u8> = img.pixels.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    ring::digest::digest(&ring::digest::SHA256, &rgb).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of `Image.open(f).convert("RGB").tobytes()` from Pillow 12.
#[test]
fn jpeg_matches_pillow_exactly() {
    for (file, expected) in [
        ("baseline_420.jpg", "8b971ec08df8aa593fa042e87cf13f05dfad9d9859685faf3bbc1d6f01dab927"),
        ("progressive_420.jpg", "ae163df12a255f9bb28f4211d1beb6bb08ad02e815ab9fc990e25ffe0b814efb"),
        ("progressive_restart.jpg", "86a2461c274cc1f2c94ddd8eec300674e0fc01c7ce3d9d75b42099d5576c6a02"),
        ("cmyk.jpg", "2553607848f5fb08294a66602d009c090d4eb2f7a7baf8389550be246aa528cc"),
        ("sample_4x2.jpg", "02144fdcdb776a09e35b20fc211396ce1955f1e9f70dc35caaa608c296ddc404"),
    ] {
        let img = image::decode(&data(file), &Limits::default()).unwrap();
        assert_eq!((img.width, img.height), (33, 47), "{file}");
        assert_eq!(rgb_sha256(&img), expected, "{file}");
    }
}

#[test]
fn png_transparency_is_kept() {
    let rgba = image::decode(&data("rgba.png"), &Limits::default()).unwrap();
    assert!(rgba.has_alpha);
    assert_eq!(&rgba.pixels[..4], &[0, 0, 0, 0]);
    let pal = image::decode(&data("palette.png"), &Limits::default()).unwrap();
    assert!(pal.has_alpha, "palette PNG with tRNS");
}

#[test]
fn damaged_files_never_panic() {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for name in ["baseline_420.jpg", "progressive_restart.jpg", "cmyk.jpg", "rgba.png", "palette.png"] {
        let original = data(name);
        for cut in 0..original.len() {
            let _ = image::decode(&original[..cut], &Limits::default());
        }
        for _ in 0..1500 {
            let mut m = original.clone();
            for _ in 0..3 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let pos = x as usize % m.len();
                m[pos] = (x >> 32) as u8;
            }
            let _ = image::decode(&m, &Limits::default());
        }
    }
}

#[test]
fn oversized_and_foreign_files_are_refused() {
    let mut jpeg = data("baseline_420.jpg");
    let sof = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    jpeg[sof + 5..sof + 9].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    assert!(matches!(image::decode(&jpeg, &Limits::default()), Err(DecodeError::TooLarge { .. })));
    assert!(matches!(image::decode(b"GIF89a....", &Limits::default()), Err(DecodeError::Unsupported(_))));
}
