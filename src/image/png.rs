//! PNG reading (every standard colour type and bit depth, Adam7, PLTE, tRNS)
//! and writing (8-bit RGB or RGBA with adaptive filtering).
//!
//! Decoding follows Pillow where it matters for the logo: 16-bit samples
//! keep their high byte, no gamma or colour-profile correction is applied.
//! Unlike Pillow's paste, transparency from tRNS is kept.

// Indexed loops read more naturally than iterator chains in codec code.
#![allow(clippy::needless_range_loop)]

use super::zlib;
use super::{DecodeError, Image, Limits};

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorType {
    Grey = 0,
    Rgb = 2,
    Palette = 3,
    GreyAlpha = 4,
    Rgba = 6,
}

impl ColorType {
    fn channels(self) -> u64 {
        match self {
            ColorType::Grey | ColorType::Palette => 1,
            ColorType::GreyAlpha => 2,
            ColorType::Rgb => 3,
            ColorType::Rgba => 4,
        }
    }
}

struct Header {
    width: u32,
    height: u32,
    depth: u8,
    color: ColorType,
    interlaced: bool,
}

/// Adam7 passes: (x start, y start, x step, y step).
const ADAM7: [(u32, u32, u32, u32); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Bytes per scanline (without the filter byte) for `width` pixels.
fn row_bytes(h: &Header, width: u32) -> Option<usize> {
    let bits = u64::from(width).checked_mul(h.color.channels())?.checked_mul(u64::from(h.depth))?;
    usize::try_from(bits.div_ceil(8)).ok()
}

/// (x0, y0, dx, dy, pass width, pass height) for each pass that has pixels.
fn passes(h: &Header) -> Vec<(u32, u32, u32, u32, u32, u32)> {
    let list: &[(u32, u32, u32, u32)] = if h.interlaced { &ADAM7 } else { &[(0, 0, 1, 1)] };
    list.iter()
        .filter_map(|&(x0, y0, dx, dy)| {
            let w = if h.width > x0 { (h.width - x0).div_ceil(dx) } else { 0 };
            let ht = if h.height > y0 { (h.height - y0).div_ceil(dy) } else { 0 };
            (w > 0 && ht > 0).then_some((x0, y0, dx, dy, w, ht))
        })
        .collect()
}

pub fn decode(data: &[u8], limits: &Limits) -> Result<Image, DecodeError> {
    if data.len() < 8 {
        return Err(DecodeError::Truncated);
    }
    if &data[..8] != SIGNATURE {
        return Err(DecodeError::Invalid("not a PNG file"));
    }

    let mut header: Option<Header> = None;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Option<Vec<u8>> = None;
    let mut idat: Vec<u8> = Vec::new();
    let mut seen_idat = false;
    let mut pos = 8;

    while pos < data.len() {
        let head = data.get(pos..pos + 8).ok_or(DecodeError::Truncated)?;
        let len = be32(&head[..4]);
        if len > 0x7FFF_FFFF {
            return Err(DecodeError::Invalid("chunk too long"));
        }
        let kind: [u8; 4] = [head[4], head[5], head[6], head[7]];
        let body_start = pos + 8;
        let body_end = body_start.checked_add(len as usize).ok_or(DecodeError::Truncated)?;
        let body = data.get(body_start..body_end).ok_or(DecodeError::Truncated)?;
        let crc = data.get(body_end..body_end + 4).ok_or(DecodeError::Truncated)?;
        pos = body_end + 4;

        let critical = kind[0] & 0x20 == 0;
        let crc_ok = zlib::crc32_update(zlib::crc32_update(0xFFFF_FFFF, &kind), body) ^ 0xFFFF_FFFF == be32(crc);
        if !crc_ok {
            if critical {
                return Err(DecodeError::Invalid("chunk checksum mismatch"));
            }
            continue; // a damaged ancillary chunk is simply ignored
        }

        match &kind {
            b"IHDR" => {
                if header.is_some() || body.len() != 13 {
                    return Err(DecodeError::Invalid("bad IHDR"));
                }
                let width = be32(&body[0..4]);
                let height = be32(&body[4..8]);
                let depth = body[8];
                let color = match body[9] {
                    0 => ColorType::Grey,
                    2 => ColorType::Rgb,
                    3 => ColorType::Palette,
                    4 => ColorType::GreyAlpha,
                    6 => ColorType::Rgba,
                    _ => return Err(DecodeError::Invalid("bad colour type")),
                };
                let depth_ok = match color {
                    ColorType::Grey => matches!(depth, 1 | 2 | 4 | 8 | 16),
                    ColorType::Palette => matches!(depth, 1 | 2 | 4 | 8),
                    _ => matches!(depth, 8 | 16),
                };
                if !depth_ok || body[10] != 0 || body[11] != 0 || body[12] > 1 {
                    return Err(DecodeError::Invalid("bad IHDR"));
                }
                if width > 0x7FFF_FFFF || height > 0x7FFF_FFFF {
                    return Err(DecodeError::Invalid("bad dimensions"));
                }
                limits.check(width, height)?;
                header = Some(Header { width, height, depth, color, interlaced: body[12] == 1 });
            }
            _ if header.is_none() => return Err(DecodeError::Invalid("IHDR is not the first chunk")),
            b"PLTE" => {
                if body.len() % 3 != 0 || body.len() > 256 * 3 || seen_idat {
                    return Err(DecodeError::Invalid("bad palette"));
                }
                palette = body.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
            }
            b"tRNS" if !seen_idat => trns = Some(body.to_vec()),
            b"IDAT" => {
                seen_idat = true;
                idat.extend_from_slice(body);
            }
            // Anything after IEND is ignored.
            b"IEND" => break,
            _ if critical => return Err(DecodeError::Unsupported("unknown critical PNG chunk")),
            _ => {}
        }
    }

    let h = header.ok_or(DecodeError::Invalid("no IHDR"))?;
    if !seen_idat {
        return Err(DecodeError::Truncated);
    }
    if h.color == ColorType::Palette && palette.is_empty() {
        return Err(DecodeError::Invalid("palette image without PLTE"));
    }

    // Exact size of the filtered data, so inflate can stop there.
    let mut expected: usize = 0;
    let pass_list = passes(&h);
    for &(.., w, ht) in &pass_list {
        let line = row_bytes(&h, w)
            .and_then(|r| r.checked_add(1))
            .ok_or(DecodeError::TooLarge { width: h.width, height: h.height })?;
        expected = line
            .checked_mul(ht as usize)
            .and_then(|n| n.checked_add(expected))
            .ok_or(DecodeError::TooLarge { width: h.width, height: h.height })?;
    }
    let raw = zlib::inflate_zlib(&idat, expected)?;
    drop(idat);

    let has_alpha = matches!(h.color, ColorType::GreyAlpha | ColorType::Rgba) || trns.is_some();
    let pixel_count = h.width as usize * h.height as usize;
    let mut pixels = vec![0u8; pixel_count * 4];
    let bpp = ((h.color.channels() * u64::from(h.depth)).div_ceil(8)) as usize;

    let mut offset = 0;
    for &(x0, y0, dx, dy, w, ht) in &pass_list {
        let line = row_bytes(&h, w).unwrap_or(0);
        let mut prev = vec![0u8; line];
        let mut cur = vec![0u8; line];
        for row in 0..ht {
            let filter = raw[offset];
            cur.copy_from_slice(&raw[offset + 1..offset + 1 + line]);
            offset += line + 1;
            unfilter(filter, &mut cur, &prev, bpp)?;
            let y = y0 + row * dy;
            for col in 0..w {
                let x = x0 + col * dx;
                let px = sample_pixel(&h, &cur, col as usize, &palette, trns.as_deref());
                let i = (y as usize * h.width as usize + x as usize) * 4;
                pixels[i..i + 4].copy_from_slice(&px);
            }
            std::mem::swap(&mut prev, &mut cur);
        }
    }

    Ok(Image { width: h.width, height: h.height, has_alpha, pixels })
}

fn unfilter(filter: u8, cur: &mut [u8], prev: &[u8], bpp: usize) -> Result<(), DecodeError> {
    match filter {
        0 => {}
        1 => {
            for i in bpp..cur.len() {
                cur[i] = cur[i].wrapping_add(cur[i - bpp]);
            }
        }
        2 => {
            for i in 0..cur.len() {
                cur[i] = cur[i].wrapping_add(prev[i]);
            }
        }
        3 => {
            for i in 0..cur.len() {
                let left = if i >= bpp { u16::from(cur[i - bpp]) } else { 0 };
                cur[i] = cur[i].wrapping_add(((left + u16::from(prev[i])) / 2) as u8);
            }
        }
        4 => {
            for i in 0..cur.len() {
                let a = if i >= bpp { cur[i - bpp] } else { 0 };
                let c = if i >= bpp { prev[i - bpp] } else { 0 };
                cur[i] = cur[i].wrapping_add(paeth(a, prev[i], c));
            }
        }
        _ => return Err(DecodeError::Invalid("bad filter type")),
    }
    Ok(())
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i16::from(a), i16::from(b), i16::from(c));
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Raw sample `index` of a row (any depth), as stored.
fn raw_sample(row: &[u8], depth: u8, index: usize) -> u16 {
    match depth {
        16 => u16::from_be_bytes([row[index * 2], row[index * 2 + 1]]),
        8 => u16::from(row[index]),
        d => {
            let bit = index * usize::from(d);
            let byte = row[bit / 8];
            let shift = 8 - usize::from(d) - bit % 8;
            u16::from((byte >> shift) & ((1u8 << d) - 1))
        }
    }
}

/// Scale a raw sample to 8 bits: high byte for 16-bit, full range below 8.
fn to8(v: u16, depth: u8) -> u8 {
    match depth {
        16 => (v >> 8) as u8,
        8 => v as u8,
        d => (u32::from(v) * 255 / ((1u32 << d) - 1)) as u8,
    }
}

fn sample_pixel(h: &Header, row: &[u8], x: usize, palette: &[[u8; 3]], trns: Option<&[u8]>) -> [u8; 4] {
    let d = h.depth;
    let trns16 = |i: usize| trns.and_then(|t| t.get(i * 2..i * 2 + 2)).map(|b| u16::from_be_bytes([b[0], b[1]]));
    match h.color {
        ColorType::Grey => {
            let v = raw_sample(row, d, x);
            let g = to8(v, d);
            let a = if trns16(0) == Some(v) { 0 } else { 255 };
            [g, g, g, a]
        }
        ColorType::Rgb => {
            let (r, g, b) = (raw_sample(row, d, x * 3), raw_sample(row, d, x * 3 + 1), raw_sample(row, d, x * 3 + 2));
            let transparent = matches!((trns16(0), trns16(1), trns16(2)), (Some(tr), Some(tg), Some(tb)) if (tr, tg, tb) == (r, g, b));
            [to8(r, d), to8(g, d), to8(b, d), if transparent { 0 } else { 255 }]
        }
        ColorType::Palette => {
            let i = usize::from(raw_sample(row, d, x));
            // An index past the palette is treated as opaque black.
            let [r, g, b] = palette.get(i).copied().unwrap_or([0, 0, 0]);
            let a = trns.and_then(|t| t.get(i)).copied().unwrap_or(255);
            [r, g, b, a]
        }
        ColorType::GreyAlpha => {
            let g = to8(raw_sample(row, d, x * 2), d);
            [g, g, g, to8(raw_sample(row, d, x * 2 + 1), d)]
        }
        ColorType::Rgba => [
            to8(raw_sample(row, d, x * 4), d),
            to8(raw_sample(row, d, x * 4 + 1), d),
            to8(raw_sample(row, d, x * 4 + 2), d),
            to8(raw_sample(row, d, x * 4 + 3), d),
        ],
    }
}

// ------------------------------------------------------------ encoding

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let crc = zlib::crc32_update(zlib::crc32_update(0xFFFF_FFFF, kind), body) ^ 0xFFFF_FFFF;
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode as 8-bit RGB (colour type 2) when `!img.has_alpha`, else RGBA (6).
pub fn encode(img: &Image) -> Vec<u8> {
    let channels = if img.has_alpha { 4 } else { 3 };
    let width = img.width as usize;
    let line = width * channels;
    let mut filtered = Vec::with_capacity((line + 1) * img.height as usize);
    let mut prev = vec![0u8; line];
    let mut cur = vec![0u8; line];
    let mut candidates: [Vec<u8>; 5] = std::array::from_fn(|_| vec![0u8; line]);

    for row in img.pixels.chunks_exact(width * 4) {
        for (dst, px) in cur.chunks_exact_mut(channels).zip(row.chunks_exact(4)) {
            dst.copy_from_slice(&px[..channels]);
        }
        // Try all five filters; keep the one with the smallest sum of
        // absolute values (the usual heuristic, as in libpng).
        let bpp = channels;
        for (f, out) in candidates.iter_mut().enumerate() {
            for i in 0..line {
                let a = if i >= bpp { cur[i - bpp] } else { 0 };
                let b = prev[i];
                let c = if i >= bpp { prev[i - bpp] } else { 0 };
                let predictor = match f {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                    _ => paeth(a, b, c),
                };
                out[i] = cur[i].wrapping_sub(predictor);
            }
        }
        let score = |v: &Vec<u8>| v.iter().map(|&b| u64::from((b as i8).unsigned_abs())).sum::<u64>();
        let best = (0..5).min_by_key(|&f| score(&candidates[f])).unwrap_or(0);
        filtered.push(best as u8);
        filtered.extend_from_slice(&candidates[best]);
        std::mem::swap(&mut prev, &mut cur);
    }

    let mut out = SIGNATURE.to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, if img.has_alpha { 6 } else { 2 }, 0, 0, 0]);
    write_chunk(&mut out, b"IHDR", &ihdr);
    let compressed = zlib::deflate_zlib(&filtered);
    for part in compressed.chunks(1 << 20) {
        write_chunk(&mut out, b"IDAT", part);
    }
    write_chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32, alpha: bool) -> Image {
        let mut img = Image::new(w, h, [0, 0, 0, 255]);
        img.has_alpha = alpha;
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                img.pixels[i..i + 4].copy_from_slice(&[
                    (x * 7) as u8,
                    (y * 13) as u8,
                    ((x ^ y) * 3) as u8,
                    if alpha { (x + y) as u8 } else { 255 },
                ]);
            }
        }
        img
    }

    #[test]
    fn round_trip() {
        for (w, h) in [(1, 1), (1, 9), (9, 1), (7, 13), (300, 200)] {
            for alpha in [false, true] {
                let img = gradient(w, h, alpha);
                let back = decode(&encode(&img), &Limits::default()).unwrap();
                assert_eq!(back, img, "{w}x{h} alpha={alpha}");
            }
        }
    }

    #[test]
    fn limits_and_garbage() {
        let png = encode(&gradient(64, 64, false));
        let tight = Limits { max_width: 32, max_height: 32, max_pixels: 1000 };
        assert!(matches!(decode(&png, &tight), Err(DecodeError::TooLarge { .. })));
        // Cut anywhere before the end of the image data: an error. (A file
        // missing only its IEND marker still decodes, as in Pillow.)
        let iend = png.len() - 12;
        for cut in 0..png.len() {
            let result = decode(&png[..cut], &Limits::default());
            assert!(cut >= iend || result.is_err(), "cut at {cut}");
        }
        let mut x = 99u32;
        for _ in 0..2000 {
            let mut m = png.clone();
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let pos = 8 + (x >> 8) as usize % (m.len() - 8);
            m[pos] ^= (x >> 24) as u8 | 1;
            let _ = decode(&m, &Limits::default());
        }
    }

    #[test]
    fn built_in_logo() {
        let logo = include_bytes!("../../assets/fp_logo.png");
        let img = decode(logo, &Limits::default()).unwrap();
        assert_eq!((img.width, img.height, img.has_alpha), (680, 680, false));
    }
}
