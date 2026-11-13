//! Greyscale page images: one byte per pixel, 0 = black, 255 = white.

/// A greyscale raster, row-major, no padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub px: Vec<u8>,
}

/// Rec. 601 luma, as most scanners and image viewers use.
pub fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114 + 500) / 1000) as u8
}

impl Gray {
    pub fn new(width: usize, height: usize, value: u8) -> Gray {
        Gray { width, height, px: vec![value; width * height] }
    }

    /// Flatten an RGBA image onto white.
    pub fn from_rgba(img: &crate::image::Image) -> Gray {
        let px = img
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let y = u32::from(luma(p[0], p[1], p[2]));
                let a = u32::from(p[3]);
                ((y * a + 255 * (255 - a) + 127) / 255) as u8
            })
            .collect();
        Gray { width: img.width as usize, height: img.height as usize, px }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u8 {
        self.px[y * self.width + x]
    }

    pub fn row(&self, y: usize) -> &[u8] {
        &self.px[y * self.width..(y + 1) * self.width]
    }

    /// Resize to `width` x `height`: box averaging when shrinking, bilinear
    /// when enlarging (separately per axis).
    pub fn resize(&self, width: usize, height: usize) -> Gray {
        if width == self.width && height == self.height {
            return self.clone();
        }
        let width = width.max(1);
        let height = height.max(1);
        // Horizontal pass, then vertical.
        let xw = weights(self.width, width);
        let yw = weights(self.height, height);
        let mut tmp = vec![0f32; width * self.height];
        for y in 0..self.height {
            let src = self.row(y);
            let dst = &mut tmp[y * width..(y + 1) * width];
            for (x, (first, ws)) in xw.iter().enumerate() {
                let mut acc = 0f32;
                for (i, w) in ws.iter().enumerate() {
                    acc += f32::from(src[first + i]) * w;
                }
                dst[x] = acc;
            }
        }
        let mut out = vec![0u8; width * height];
        let mut acc = vec![0f32; width];
        for (y, (first, ws)) in yw.iter().enumerate() {
            acc.iter_mut().for_each(|a| *a = 0.0);
            for (i, w) in ws.iter().enumerate() {
                let src = &tmp[(first + i) * width..(first + i + 1) * width];
                for (a, s) in acc.iter_mut().zip(src) {
                    *a += s * w;
                }
            }
            for (d, a) in out[y * width..(y + 1) * width].iter_mut().zip(&acc) {
                *d = a.round().clamp(0.0, 255.0) as u8;
            }
        }
        Gray { width, height, px: out }
    }

    /// Scale by `factor` (both axes).
    pub fn scale(&self, factor: f32) -> Gray {
        let w = ((self.width as f32 * factor).round() as usize).max(1);
        let h = ((self.height as f32 * factor).round() as usize).max(1);
        self.resize(w, h)
    }

    /// Rotate by a multiple of 90 degrees clockwise.
    pub fn rotate90(&self, quarter_turns: u32) -> Gray {
        let (w, h) = (self.width, self.height);
        match quarter_turns % 4 {
            0 => self.clone(),
            2 => {
                let mut px = self.px.clone();
                px.reverse();
                Gray { width: w, height: h, px }
            }
            q => {
                let mut px = vec![0u8; w * h];
                // New image is h wide, w tall.
                for y in 0..h {
                    for x in 0..w {
                        let (nx, ny) = if q == 1 { (h - 1 - y, x) } else { (y, w - 1 - x) };
                        px[ny * h + nx] = self.px[y * w + x];
                    }
                }
                Gray { width: h, height: w, px }
            }
        }
    }

    /// Whether nearly every pixel is pure black or white (a black-and-white
    /// scan).
    pub fn is_bilevel(&self) -> bool {
        let step = (self.px.len() / 200_000).max(1);
        let (mut grey, mut n) = (0usize, 0usize);
        for &v in self.px.iter().step_by(step) {
            n += 1;
            if v != 0 && v != 255 {
                grey += 1;
            }
        }
        grey * 100 < n
    }

    /// A slight blur (3 x 3, weights 1-2-1), which gives black-and-white
    /// scans the soft edges the text recogniser was trained on.
    pub fn soften(&self) -> Gray {
        let (w, h) = (self.width, self.height);
        if w < 3 || h < 3 {
            return self.clone();
        }
        let mut tmp = vec![0u16; w * h];
        for y in 0..h {
            let row = &self.px[y * w..(y + 1) * w];
            let out = &mut tmp[y * w..(y + 1) * w];
            for x in 0..w {
                let l = row[x.saturating_sub(1)];
                let r = row[(x + 1).min(w - 1)];
                out[x] = u16::from(l) + 2 * u16::from(row[x]) + u16::from(r);
            }
        }
        let mut px = vec![0u8; w * h];
        for y in 0..h {
            let (up, down) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for x in 0..w {
                let v = tmp[up * w + x] + 2 * tmp[y * w + x] + tmp[down * w + x];
                px[y * w + x] = ((v + 8) / 16) as u8;
            }
        }
        Gray { width: w, height: h, px }
    }

    /// Mirror left to right.
    pub fn flip_horizontal(&self) -> Gray {
        let mut out = self.clone();
        for row in out.px.chunks_exact_mut(self.width) {
            row.reverse();
        }
        out
    }

    /// Rotate by a small angle (degrees, positive = clockwise) about the
    /// centre, keeping the size; uncovered corners become white.
    pub fn rotate_small(&self, degrees: f32) -> Gray {
        let (w, h) = (self.width, self.height);
        let (s, c) = degrees.to_radians().sin_cos();
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let mut px = vec![255u8; w * h];
        for y in 0..h {
            let dy = y as f32 + 0.5 - cy;
            for x in 0..w {
                let dx = x as f32 + 0.5 - cx;
                // Inverse rotation: where in the source this pixel comes from.
                let sx = c * dx + s * dy + cx - 0.5;
                let sy = -s * dx + c * dy + cy - 0.5;
                if sx < 0.0 || sy < 0.0 || sx > (w - 1) as f32 || sy > (h - 1) as f32 {
                    continue;
                }
                let (x0, y0) = (sx as usize, sy as usize);
                let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
                let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
                let p = |x: usize, y: usize| f32::from(self.px[y * w + x]);
                let top = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
                let bottom = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
                px[y * w + x] = (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
        }
        Gray { width: w, height: h, px }
    }

    /// A copy of a rectangle (clipped to the image), white outside.
    pub fn crop(&self, x: i64, y: i64, width: usize, height: usize) -> Gray {
        let mut out = Gray::new(width, height, 255);
        for oy in 0..height {
            let sy = y + oy as i64;
            if sy < 0 || sy >= self.height as i64 {
                continue;
            }
            let x0 = x.max(0);
            let x1 = (x + width as i64).min(self.width as i64);
            if x1 <= x0 {
                continue;
            }
            let src = &self.px[sy as usize * self.width + x0 as usize..sy as usize * self.width + x1 as usize];
            let dst = oy * width + (x0 - x) as usize;
            out.px[dst..dst + src.len()].copy_from_slice(src);
        }
        out
    }
}

/// Per output pixel: first input index and normalised weights.
fn weights(in_size: usize, out_size: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = in_size as f32 / out_size as f32;
    (0..out_size)
        .map(|o| {
            if scale > 1.0 {
                // Box filter over the source span.
                let start = o as f32 * scale;
                let end = (start + scale).min(in_size as f32);
                let first = start.floor() as usize;
                let last = (end.ceil() as usize).min(in_size);
                let ws: Vec<f32> = (first..last)
                    .map(|i| {
                        let lo = (i as f32).max(start);
                        let hi = ((i + 1) as f32).min(end);
                        (hi - lo).max(0.0)
                    })
                    .collect();
                let sum: f32 = ws.iter().sum::<f32>().max(1e-6);
                (first, ws.into_iter().map(|w| w / sum).collect())
            } else {
                // Bilinear.
                let center = (o as f32 + 0.5) * scale - 0.5;
                let c = center.max(0.0);
                let i0 = (c.floor() as usize).min(in_size - 1);
                let i1 = (i0 + 1).min(in_size - 1);
                let f = c - i0 as f32;
                if i1 == i0 { (i0, vec![1.0]) } else { (i0, vec![1.0 - f, f]) }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotations_round_trip() {
        let g = Gray { width: 3, height: 2, px: vec![1, 2, 3, 4, 5, 6] };
        let r = g.rotate90(1);
        assert_eq!((r.width, r.height), (2, 3));
        assert_eq!(r.px, vec![4, 1, 5, 2, 6, 3]);
        assert_eq!(g.rotate90(3).px, vec![3, 6, 2, 5, 1, 4]);
        assert_eq!(r.rotate90(3), g);
        assert_eq!(g.rotate90(2).px, vec![6, 5, 4, 3, 2, 1]);
    }

    #[test]
    fn resize_averages_when_shrinking() {
        let g = Gray { width: 4, height: 2, px: vec![0, 255, 0, 255, 0, 255, 0, 255] };
        let r = g.resize(2, 1);
        assert_eq!(r.px, vec![128, 128]);
        let up = Gray { width: 2, height: 1, px: vec![0, 200] }.resize(4, 1);
        assert_eq!(up.px[0], 0);
        assert_eq!(up.px[3], 200);
    }

    #[test]
    fn crop_pads_with_white() {
        let g = Gray { width: 2, height: 2, px: vec![1, 2, 3, 4] };
        assert_eq!(g.crop(-1, 0, 3, 3).px, vec![255, 1, 2, 255, 3, 4, 255, 255, 255]);
    }
}
