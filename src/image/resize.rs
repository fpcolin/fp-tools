//! Lanczos-3 resampling, modelled on Pillow's `Image.resize(size, LANCZOS)`
//! (which the Python build used for the embedded logo): separable passes,
//! the filter widened when shrinking, and colour premultiplied by alpha so
//! transparent edges do not pick up dark fringes.

use super::Image;

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let x = x * std::f64::consts::PI;
        x.sin() / x
    }
}

fn lanczos3(x: f64) -> f64 {
    if (-3.0..3.0).contains(&x) { sinc(x) * sinc(x / 3.0) } else { 0.0 }
}

/// For each output position: first input index and the normalised weights.
fn coefficients(in_size: usize, out_size: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = in_size as f64 / out_size as f64;
    let filter_scale = scale.max(1.0);
    let support = 3.0 * filter_scale;
    (0..out_size)
        .map(|i| {
            let center = (i as f64 + 0.5) * scale;
            let first = ((center - support + 0.5).floor().max(0.0)) as usize;
            let last = ((center + support + 0.5).floor() as usize).min(in_size);
            let mut weights: Vec<f64> =
                (first..last).map(|x| lanczos3((x as f64 - center + 0.5) / filter_scale)).collect();
            let sum: f64 = weights.iter().sum();
            if sum != 0.0 {
                weights.iter_mut().for_each(|w| *w /= sum);
            }
            (first, weights.into_iter().map(|w| w as f32).collect())
        })
        .collect()
}

fn clamp_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// One separable pass over premultiplied RGBA rows. `stride_in`/`stride_out`
/// step between samples along the filtered axis; `lines` counts the other axis.
#[allow(clippy::too_many_arguments)]
fn pass(
    src: &[u8],
    dst: &mut [u8],
    coeffs: &[(usize, Vec<f32>)],
    lines: usize,
    line_step_in: usize,
    line_step_out: usize,
    stride_in: usize,
    stride_out: usize,
) {
    for line in 0..lines {
        let base_in = line * line_step_in;
        let base_out = line * line_step_out;
        for (i, (first, weights)) in coeffs.iter().enumerate() {
            let mut acc = [0f32; 4];
            for (k, w) in weights.iter().enumerate() {
                let p = base_in + (first + k) * stride_in;
                for c in 0..4 {
                    acc[c] += f32::from(src[p + c]) * w;
                }
            }
            let q = base_out + i * stride_out;
            for c in 0..4 {
                dst[q + c] = clamp_u8(acc[c]);
            }
        }
    }
}

/// Resize to exactly `width` x `height` (aspect ratio is not preserved, which
/// matches the Python build: the logo is always squashed into a square).
pub fn resize(img: &Image, width: u32, height: u32) -> Image {
    let (w0, h0) = (img.width as usize, img.height as usize);
    let (w1, h1) = (width.max(1) as usize, height.max(1) as usize);

    // Premultiply.
    let mut src = img.pixels.clone();
    for px in src.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }

    // Horizontal, then vertical, skipping a pass whose size is unchanged.
    let horizontal = if w1 == w0 {
        src
    } else {
        let mut out = vec![0u8; w1 * h0 * 4];
        pass(&src, &mut out, &coefficients(w0, w1), h0, w0 * 4, w1 * 4, 4, 4);
        out
    };
    let mut out = if h1 == h0 {
        horizontal
    } else {
        let mut out = vec![0u8; w1 * h1 * 4];
        pass(&horizontal, &mut out, &coefficients(h0, h1), w1, 4, 4, w1 * 4, w1 * 4);
        out
    };

    // Back to straight alpha.
    for px in out.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = (u32::from(*c) * 255 + a / 2).checked_div(a).map_or(0, |v| v.min(255) as u8);
        }
    }
    Image { width: w1 as u32, height: h1 as u32, has_alpha: img.has_alpha, pixels: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_colour_stays_flat() {
        let img = Image::new(37, 53, [200, 100, 50, 255]);
        let out = resize(&img, 10, 10);
        assert_eq!((out.width, out.height), (10, 10));
        assert!(out.pixels.chunks(4).all(|p| p == [200, 100, 50, 255]));
        let up = resize(&img, 80, 81);
        assert!(up.pixels.chunks(4).all(|p| p == [200, 100, 50, 255]));
    }

    #[test]
    fn transparent_pixels_do_not_darken_edges() {
        // Left half opaque red, right half fully transparent black.
        let mut img = Image::new(20, 4, [0, 0, 0, 0]);
        for y in 0..4 {
            for x in 0..10 {
                let i = (y * 20 + x) * 4;
                img.pixels[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let out = resize(&img, 7, 4);
        for px in out.pixels.chunks(4) {
            if px[3] > 10 {
                assert!(px[0] >= 250 && px[1] < 5 && px[2] < 5, "{px:?}");
            }
        }
    }
}
