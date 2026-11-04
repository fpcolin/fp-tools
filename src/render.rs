//! Turning a QR symbol into pixels, matching the Python build's output:
//! 10-pixel modules, a 4-module white border, optional rounded modules, and an
//! optional logo covering the centre (sized and placed exactly as qrcode's
//! StyledPilImage did).

use crate::image::{Image, resize};
use crate::qr::QrCode;

/// Pixels per module (qrcode's default box_size).
pub const BOX_SIZE: u32 = 10;
/// Quiet-zone width in modules (qrcode's default border).
pub const BORDER: u32 = 4;
/// Share of the image width the logo aims for (qrcode's embeded_image_ratio).
pub const LOGO_RATIO: f64 = 0.25;

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

#[derive(Default)]
pub struct Style<'a> {
    /// Round the outer corners of each run of modules.
    pub round_edges: bool,
    /// Image drawn over the centre. Requires error correction level H.
    pub logo: Option<&'a Image>,
}

/// Width and height of the rendered image for a symbol of `modules` per side.
pub fn pixel_size(modules: usize) -> u32 {
    (modules as u32 + 2 * BORDER) * BOX_SIZE
}

/// Where the logo goes: (offset from top-left, side length), in pixels.
/// Same arithmetic as qrcode's StyledPilImage.draw_embeded_image, including
/// snapping the offset to the module grid.
pub fn logo_placement(total: u32) -> (u32, u32) {
    let approx = (f64::from(total) * LOGO_RATIO) as u32;
    let offset = (total / 2 - approx / 2) / BOX_SIZE * BOX_SIZE;
    (offset, total - offset * 2)
}

pub fn render(qr: &QrCode, style: &Style<'_>) -> Image {
    let size = qr.size();
    let total = pixel_size(size);
    let mut img = Image::new(total, total, WHITE);
    img.has_alpha = false;

    let corners = if style.round_edges { Some(CornerTile::new()) } else { None };
    let border = BORDER as usize;
    let dark = |x: isize, y: isize| x >= 0 && y >= 0 && qr.is_dark(x as usize, y as usize);

    for y in 0..size {
        for x in 0..size {
            if !qr.is_dark(x, y) {
                continue;
            }
            let left = ((x + border) as u32) * BOX_SIZE;
            let top = ((y + border) as u32) * BOX_SIZE;
            match &corners {
                None => fill_rect(&mut img, left, top, BOX_SIZE, BOX_SIZE, BLACK),
                Some(tile) => {
                    let (xi, yi) = (x as isize, y as isize);
                    let n = dark(xi, yi - 1);
                    let s = dark(xi, yi + 1);
                    let w = dark(xi - 1, yi);
                    let e = dark(xi + 1, yi);
                    let half = BOX_SIZE / 2;
                    // A quadrant is rounded when both neighbours on its outer
                    // sides are light (qrcode's RoundedModuleDrawer rule).
                    tile.draw(&mut img, left, top, Quadrant::NorthWest, !n && !w);
                    tile.draw(&mut img, left + half, top, Quadrant::NorthEast, !n && !e);
                    tile.draw(&mut img, left + half, top + half, Quadrant::SouthEast, !s && !e);
                    tile.draw(&mut img, left, top + half, Quadrant::SouthWest, !s && !w);
                }
            }
        }
    }

    if let Some(logo) = style.logo {
        let (offset, side) = logo_placement(total);
        let scaled = resize::resize(logo, side, side);
        composite(&mut img, &scaled, offset, offset);
    }
    img
}

fn fill_rect(img: &mut Image, left: u32, top: u32, w: u32, h: u32, rgba: [u8; 4]) {
    for y in top..top + h {
        let row = (y * img.width) as usize * 4;
        for x in left..left + w {
            let i = row + x as usize * 4;
            img.pixels[i..i + 4].copy_from_slice(&rgba);
        }
    }
}

/// Draw `src` over `dst` at (left, top). Opaque sources are copied (Pillow's
/// paste); sources with transparency are blended (Pillow's alpha_composite).
fn composite(dst: &mut Image, src: &Image, left: u32, top: u32) {
    for sy in 0..src.height {
        let dy = top + sy;
        if dy >= dst.height {
            break;
        }
        for sx in 0..src.width {
            let dx = left + sx;
            if dx >= dst.width {
                break;
            }
            let s = ((sy * src.width + sx) * 4) as usize;
            let d = ((dy * dst.width + dx) * 4) as usize;
            let a = u32::from(src.pixels[s + 3]);
            if !src.has_alpha || a == 255 {
                dst.pixels[d..d + 3].copy_from_slice(&src.pixels[s..s + 3]);
            } else {
                for c in 0..3 {
                    let blended = u32::from(src.pixels[s + c]) * a + u32::from(dst.pixels[d + c]) * (255 - a);
                    dst.pixels[d + c] = ((blended + 127) / 255) as u8;
                }
            }
            dst.pixels[d + 3] = 255;
        }
    }
}

#[derive(Clone, Copy)]
enum Quadrant {
    NorthWest,
    NorthEast,
    SouthEast,
    SouthWest,
}

/// Anti-aliased quarter disc for one quadrant of a module: the arc's centre is
/// the module's centre, its radius half a module.
struct CornerTile {
    /// Darkness 0..=255 for the north-west orientation, row-major.
    coverage: Vec<u8>,
}

impl CornerTile {
    fn new() -> CornerTile {
        const SAMPLES: u32 = 16;
        let side = BOX_SIZE / 2;
        let r = f64::from(side);
        let mut coverage = Vec::with_capacity((side * side) as usize);
        for py in 0..side {
            for px in 0..side {
                let mut inside = 0u32;
                for sy in 0..SAMPLES {
                    for sx in 0..SAMPLES {
                        // Distance from the tile's south-east corner, which
                        // is the module centre for the north-west quadrant.
                        let x = f64::from(px) + (f64::from(sx) + 0.5) / f64::from(SAMPLES) - r;
                        let y = f64::from(py) + (f64::from(sy) + 0.5) / f64::from(SAMPLES) - r;
                        if x * x + y * y <= r * r {
                            inside += 1;
                        }
                    }
                }
                coverage.push(((inside * 255 + SAMPLES * SAMPLES / 2) / (SAMPLES * SAMPLES)) as u8);
            }
        }
        CornerTile { coverage }
    }

    fn draw(&self, img: &mut Image, left: u32, top: u32, quadrant: Quadrant, rounded: bool) {
        let side = BOX_SIZE / 2;
        if !rounded {
            fill_rect(img, left, top, side, side, BLACK);
            return;
        }
        for ty in 0..side {
            for tx in 0..side {
                // Mirror the north-west tile into the requested quadrant.
                let (sx, sy) = match quadrant {
                    Quadrant::NorthWest => (tx, ty),
                    Quadrant::NorthEast => (side - 1 - tx, ty),
                    Quadrant::SouthEast => (side - 1 - tx, side - 1 - ty),
                    Quadrant::SouthWest => (tx, side - 1 - ty),
                };
                let ink = u32::from(self.coverage[(sy * side + sx) as usize]);
                let i = (((top + ty) * img.width + left + tx) * 4) as usize;
                // Black over the existing (white) background.
                for c in 0..3 {
                    img.pixels[i + c] = ((u32::from(img.pixels[i + c]) * (255 - ink) + 127) / 255) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qr::EcLevel;

    #[test]
    fn placement_matches_python() {
        // Values computed with qrcode 8.x StyledPilImage for these sizes.
        assert_eq!(logo_placement(290), (100, 90)); // version 1
        assert_eq!(logo_placement(450), (160, 130)); // version 5
        assert_eq!(logo_placement(1850), (690, 470)); // version 40
    }

    #[test]
    fn renders_expected_geometry() {
        let qr = QrCode::encode_text("https://example.com", EcLevel::H).unwrap();
        let img = render(&qr, &Style::default());
        assert_eq!(img.width, pixel_size(qr.size()));
        // Top-left finder pattern starts dark at the border.
        let at = |x: u32, y: u32| img.pixels[((y * img.width + x) * 4) as usize];
        assert_eq!(at(BORDER * BOX_SIZE, BORDER * BOX_SIZE), 0);
        assert_eq!(at(BORDER * BOX_SIZE - 1, BORDER * BOX_SIZE), 255);
        let round = render(&qr, &Style { round_edges: true, logo: None });
        // The finder's outer corner pixel is rounded off; its centre is not.
        let at_r = |x: u32, y: u32| round.pixels[((y * round.width + x) * 4) as usize];
        assert!(at_r(BORDER * BOX_SIZE, BORDER * BOX_SIZE) > 200);
        assert_eq!(at_r(BORDER * BOX_SIZE + 4, BORDER * BOX_SIZE + 4), 0);
    }
}
