//! Off-screen drawing. Shapes are drawn in Rust straight into a 32-bit DIB
//! (anti-aliased, which GDI's own shapes are not); text is drawn by GDI into
//! the same bitmap, so it gets ClearType. The finished picture is copied to
//! the screen in one go, so nothing flickers.

#![allow(clippy::too_many_arguments)]

use std::ffi::c_void;

use super::win32::*;

/// Opaque BGRA pixels at a fixed size, ready to copy onto a canvas.
pub struct Bitmap {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
}

impl Bitmap {
    /// Scale `img` to `width` x `height` and flatten it onto `background`
    /// (0xRRGGBB).
    pub fn from_image(img: &crate::image::Image, width: u32, height: u32, background: u32) -> Bitmap {
        let scaled = if (img.width, img.height) == (width, height) {
            None
        } else {
            Some(crate::image::resize::resize(img, width.max(1), height.max(1)))
        };
        let img = scaled.as_ref().unwrap_or(img);
        let (br, bg, bb) = ((background >> 16) & 0xFF, (background >> 8) & 0xFF, background & 0xFF);
        let pixels = img
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let a = u32::from(p[3]);
                let mix = |c: u8, b: u32| (u32::from(c) * a + b * (255 - a) + 127) / 255;
                (mix(p[0], br) << 16) | (mix(p[1], bg) << 8) | mix(p[2], bb)
            })
            .collect();
        Bitmap { width: img.width as i32, height: img.height as i32, pixels }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> RectF {
        RectF { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn inset(&self, dx: f32, dy: f32) -> RectF {
        RectF::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    /// Scale from DIPs to pixels and shift by `dy` pixels; edges are rounded
    /// so neighbouring rectangles still meet.
    pub fn to_px(self, scale: f32, dy: f32) -> RECT {
        RECT {
            left: (self.x * scale).round() as i32,
            top: (self.y * scale - dy).round() as i32,
            right: (self.right() * scale).round() as i32,
            bottom: (self.bottom() * scale - dy).round() as i32,
        }
    }
}

pub struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    pub width: i32,
    pub height: i32,
    /// GDI has drawn since the last direct pixel access.
    gdi_pending: bool,
}

impl Canvas {
    pub fn new(width: i32, height: i32) -> Option<Canvas> {
        let (width, height) = (width.max(1), height.max(1));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [0],
        };
        // SAFETY: plain GDI object creation; every handle is checked and
        // released in Drop.
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return None;
            }
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bitmap);
            SetBkMode(dc, TRANSPARENT);
            Some(Canvas { dc, bitmap, old, bits: bits.cast(), width, height, gdi_pending: false })
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        if self.gdi_pending {
            // SAFETY: no arguments; finishes queued GDI drawing into the DIB.
            unsafe { GdiFlush() };
            self.gdi_pending = false;
        }
        // SAFETY: the DIB section holds width*height 32-bit pixels and lives
        // as long as `self`.
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.width * self.height) as usize) }
    }

    pub fn clear(&mut self, rgb: u32) {
        self.pixels().fill(rgb);
    }

    /// Blend `rgb` into pixel `i` with coverage `a` (0-255).
    #[inline]
    fn blend(px: &mut u32, rgb: u32, a: u32) {
        if a >= 255 {
            *px = rgb;
        } else if a > 0 {
            let d = *px;
            let mix = |shift: u32| {
                let s = (rgb >> shift) & 0xFF;
                let t = (d >> shift) & 0xFF;
                ((s * a + t * (255 - a) + 127) / 255) << shift
            };
            *px = mix(16) | mix(8) | mix(0);
        }
    }

    /// Run `coverage(x, y)` (pixel centres, returns 0..1) over the pixels in
    /// the bounding box and blend `rgb` in.
    fn shade(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, rgb: u32, coverage: impl Fn(f32, f32) -> f32) {
        let (w, h) = (self.width, self.height);
        let xa = (x0.floor() as i32 - 1).clamp(0, w);
        let xb = (x1.ceil() as i32 + 1).clamp(0, w);
        let ya = (y0.floor() as i32 - 1).clamp(0, h);
        let yb = (y1.ceil() as i32 + 1).clamp(0, h);
        let px = self.pixels();
        for y in ya..yb {
            let row = (y * w) as usize;
            for x in xa..xb {
                let c = coverage(x as f32 + 0.5, y as f32 + 0.5);
                if c > 0.0 {
                    Self::blend(&mut px[row + x as usize], rgb, (c.min(1.0) * 255.0 + 0.5) as u32);
                }
            }
        }
    }

    pub fn fill_rect(&mut self, r: RECT, rgb: u32) {
        let (w, h) = (self.width, self.height);
        let (x0, x1) = (r.left.clamp(0, w), r.right.clamp(0, w));
        let (y0, y1) = (r.top.clamp(0, h), r.bottom.clamp(0, h));
        let px = self.pixels();
        for y in y0..y1 {
            px[(y * w + x0) as usize..(y * w + x1) as usize].fill(rgb);
        }
    }

    /// Anti-aliased rounded rectangle, in pixels.
    pub fn fill_rrect(&mut self, r: RectF, radius: f32, rgb: u32) {
        let radius = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
        self.shade(r.x, r.y, r.right(), r.bottom(), rgb, |x, y| 0.5 - rrect_distance(r, radius, x, y));
    }

    /// Outline of a rounded rectangle; the stroke lies inside `r`.
    pub fn stroke_rrect(&mut self, r: RectF, radius: f32, width: f32, rgb: u32) {
        let radius = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
        let inner = r.inset(width, width);
        let inner_radius = (radius - width).max(0.0);
        self.shade(r.x, r.y, r.right(), r.bottom(), rgb, |x, y| {
            let outer = (0.5 - rrect_distance(r, radius, x, y)).clamp(0.0, 1.0);
            let hole = (0.5 - rrect_distance(inner, inner_radius, x, y)).clamp(0.0, 1.0);
            outer - hole
        });
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, rgb: u32) {
        self.shade(cx - radius, cy - radius, cx + radius, cy + radius, rgb, |x, y| {
            0.5 - (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - radius)
        });
    }

    pub fn stroke_circle(&mut self, cx: f32, cy: f32, radius: f32, width: f32, rgb: u32) {
        let r = radius + width / 2.0;
        self.shade(cx - r, cy - r, cx + r, cy + r, rgb, |x, y| {
            0.5 - ((((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - radius).abs() - width / 2.0)
        });
    }

    /// Line with round caps.
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, rgb: u32) {
        let half = width / 2.0;
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        self.shade(x0.min(x1) - half, y0.min(y1) - half, x0.max(x1) + half, y0.max(y1) + half, rgb, |x, y| {
            let t = (((x - x0) * dx + (y - y0) * dy) / len2).clamp(0.0, 1.0);
            let (px, py) = (x0 + t * dx - x, y0 + t * dy - y);
            0.5 - ((px * px + py * py).sqrt() - half)
        });
    }

    pub fn polyline(&mut self, points: &[(f32, f32)], width: f32, rgb: u32) {
        for pair in points.windows(2) {
            self.line(pair[0].0, pair[0].1, pair[1].0, pair[1].1, width, rgb);
        }
    }

    /// Copy an opaque bitmap with its top-left corner at (x, y).
    pub fn image(&mut self, x: i32, y: i32, bmp: &Bitmap) {
        let (w, h) = (self.width, self.height);
        let px = self.pixels();
        for row in 0..bmp.height {
            let ty = y + row;
            if !(0..h).contains(&ty) {
                continue;
            }
            let (sx0, sx1) = ((-x).max(0), bmp.width.min(w - x));
            if sx0 >= sx1 {
                return;
            }
            let src = &bmp.pixels[(row * bmp.width + sx0) as usize..(row * bmp.width + sx1) as usize];
            let dst = (ty * w + x + sx0) as usize;
            px[dst..dst + src.len()].copy_from_slice(src);
        }
    }

    /// Draw text with GDI. `rect` is in pixels.
    pub fn text(&mut self, rect: RECT, text: &str, font: HFONT, rgb: u32, flags: u32) {
        if text.is_empty() {
            return;
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut r = rect;
        // SAFETY: valid DC owned by this canvas; the font handle outlives the
        // call; `units` is a valid buffer of the given length.
        unsafe {
            let old = SelectObject(self.dc, font);
            SetTextColor(self.dc, colorref(rgb));
            DrawTextW(self.dc, units.as_ptr(), units.len() as i32, &mut r, flags);
            SelectObject(self.dc, old);
        }
        self.gdi_pending = true;
    }

    /// Copy the finished picture to `target` at (x, y).
    pub fn present(&mut self, target: HDC, x: i32, y: i32) {
        // SAFETY: both DCs are valid for the duration of the call.
        unsafe {
            GdiFlush();
            BitBlt(target, x, y, self.width, self.height, self.dc, 0, 0, SRCCOPY);
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: handles created in `new`, released once.
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

/// Signed distance from (x, y) to a rounded rectangle (negative inside).
fn rrect_distance(r: RectF, radius: f32, x: f32, y: f32) -> f32 {
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let qx = (x - cx).abs() - (r.w / 2.0 - radius);
    let qy = (y - cy).abs() - (r.h / 2.0 - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - radius
}

/// Measure wrapped text: height in pixels for a given width in pixels.
pub fn measure(font: HFONT, text: &str, width: i32, flags: u32) -> (i32, i32) {
    if text.is_empty() {
        return (0, 0);
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut r = RECT { left: 0, top: 0, right: width.max(1), bottom: 0 };
    // SAFETY: a memory DC created and deleted here; the font outlives the call.
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let old = SelectObject(dc, font);
        DrawTextW(dc, units.as_ptr(), units.len() as i32, &mut r, flags | DT_CALCRECT);
        SelectObject(dc, old);
        DeleteDC(dc);
    }
    (r.right - r.left, r.bottom - r.top)
}
