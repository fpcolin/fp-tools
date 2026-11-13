//! Opening a scan: PDF, TIFF (one or more pages), JPEG or PNG.

use super::gray::Gray;
use super::pdf::Pdf;
use super::tiff::{self, Tiff};

/// The largest page image accepted, in pixels (about 600 dpi on legal
/// paper); bigger pages are refused rather than risk running out of memory.
pub const MAX_PIXELS: usize = 60_000_000;

pub enum Document {
    Image(Vec<u8>),
    Tiff(Tiff),
    Pdf(Pdf),
}

impl Document {
    /// Recognise the file by its contents (not its name).
    pub fn open(data: Vec<u8>) -> Result<Document, String> {
        if data.starts_with(b"\x89PNG\r\n\x1a\n") || data.starts_with(&[0xFF, 0xD8]) {
            Ok(Document::Image(data))
        } else if tiff::is_tiff(&data) {
            Ok(Document::Tiff(Tiff::open(data)?))
        } else if data[..data.len().min(1024)].windows(5).any(|w| w == b"%PDF-") {
            Ok(Document::Pdf(Pdf::open(data)?))
        } else {
            Err("This file isn't a scan this app can read. Choose a PDF, TIFF, JPEG or PNG file.".into())
        }
    }

    pub fn page_count(&self) -> usize {
        match self {
            Document::Image(_) => 1,
            Document::Tiff(t) => t.page_count(),
            Document::Pdf(p) => p.page_count(),
        }
    }

    /// Page `index` (from 0) in grey. `None` for a PDF page without a
    /// scanned image on it (a typed cover page, say).
    pub fn page(&self, index: usize) -> Result<Option<Gray>, String> {
        match self {
            Document::Image(data) => {
                let limits =
                    crate::image::Limits { max_width: 30_000, max_height: 30_000, max_pixels: MAX_PIXELS as u64 };
                let img =
                    crate::image::decode(data, &limits).map_err(|e| format!("The image couldn't be read: {e}."))?;
                Ok(Some(Gray::from_rgba(&img)))
            }
            Document::Tiff(t) => t.page(index, MAX_PIXELS).map(|(g, _)| Some(g)),
            Document::Pdf(p) => match p.page(index, MAX_PIXELS) {
                Ok((g, _)) => Ok(Some(g)),
                Err(e) if e == "no image" => Ok(None),
                Err(e) => Err(e),
            },
        }
    }
}
