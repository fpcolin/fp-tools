//! Minimal, dependency-free raster image support.
//!
//! Every decoder in this module treats its input as untrusted: it must return
//! an error rather than panic, and must check `Limits` before allocating
//! anything proportional to the image size.

pub mod jpeg;
pub mod png;
pub mod resize;
pub mod zlib;

use std::fmt;

/// An 8-bit RGBA raster with straight (non-premultiplied) alpha.
/// Row-major, top-to-bottom, no padding: `pixels.len() == width * height * 4`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// True when the source carried any transparency information
    /// (an alpha channel, or a PNG tRNS chunk).
    pub has_alpha: bool,
    pub pixels: Vec<u8>,
}

impl Image {
    /// A new image filled with one colour.
    pub fn new(width: u32, height: u32, rgba: [u8; 4]) -> Image {
        let count = width as usize * height as usize;
        let mut pixels = Vec::with_capacity(count * 4);
        for _ in 0..count {
            pixels.extend_from_slice(&rgba);
        }
        Image { width, height, has_alpha: rgba[3] != 255, pixels }
    }
}

/// Upper bounds a decoder enforces before allocating.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_width: 16_384, max_height: 16_384, max_pixels: 40_000_000 }
    }
}

impl Limits {
    /// Reject zero-sized images and anything over the limits.
    pub fn check(&self, width: u32, height: u32) -> Result<(), DecodeError> {
        if width == 0 || height == 0 {
            return Err(DecodeError::Invalid("image has zero width or height"));
        }
        if width > self.max_width || height > self.max_height || width as u64 * height as u64 > self.max_pixels {
            return Err(DecodeError::TooLarge { width, height });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The data ended before the image was complete.
    Truncated,
    /// The data is not a valid file of this format.
    Invalid(&'static str),
    /// Valid, but uses a feature this decoder does not implement.
    Unsupported(&'static str),
    /// Dimensions exceed the configured limits.
    TooLarge { width: u32, height: u32 },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated => f.write_str("the image file is incomplete"),
            DecodeError::Invalid(why) => write!(f, "the image file is damaged or not a supported image ({why})"),
            DecodeError::Unsupported(what) => write!(f, "this kind of image is not supported ({what})"),
            DecodeError::TooLarge { width, height } => {
                write!(f, "the image is too large ({width} x {height} pixels)")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// Decode a PNG or JPEG, recognised by content rather than file extension.
pub fn decode(data: &[u8], limits: &Limits) -> Result<Image, DecodeError> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        png::decode(data, limits)
    } else if data.starts_with(&[0xFF, 0xD8]) {
        jpeg::decode(data, limits)
    } else {
        Err(DecodeError::Unsupported("only PNG and JPEG images can be used"))
    }
}
