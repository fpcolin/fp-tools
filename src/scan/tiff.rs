//! TIFF reading: every page of a (multi-page) TIFF as a greyscale image.
//!
//! Covers what scanners write: uncompressed, CCITT Group 3 and 4, LZW,
//! Deflate, PackBits and JPEG compression; black-and-white, greyscale,
//! palette and RGB pages, in strips or tiles.

use super::ccitt;
use super::gray::{Gray, luma};

/// A parsed TIFF: the file and where each page's directory is.
pub struct Tiff {
    data: Vec<u8>,
    big_endian: bool,
    pages: Vec<usize>,
}

pub fn is_tiff(data: &[u8]) -> bool {
    data.starts_with(b"II*\0") || data.starts_with(b"MM\0*")
}

struct Field {
    kind: u16,
    count: usize,
    /// Offset of the value(s): inline in the entry, or elsewhere.
    at: usize,
}

const MAX_PAGES: usize = 500;

impl Tiff {
    pub fn open(data: Vec<u8>) -> Result<Tiff, String> {
        if !is_tiff(&data) {
            return Err("not a TIFF file".into());
        }
        let big_endian = data[0] == b'M';
        let mut t = Tiff { data, big_endian, pages: Vec::new() };
        let mut next = t.u32(4).ok_or("damaged TIFF file")? as usize;
        let mut seen = std::collections::HashSet::new();
        while next != 0 && t.pages.len() < MAX_PAGES && seen.insert(next) {
            let count = t.u16(next).ok_or("damaged TIFF file")? as usize;
            let entries_end = next + 2 + count * 12;
            if entries_end + 4 > t.data.len() {
                break;
            }
            // Skip reduced-resolution copies (thumbnails).
            let subfile = t.field(next, 254).and_then(|f| t.value(&f, 0)).unwrap_or(0);
            if subfile & 1 == 0 {
                t.pages.push(next);
            }
            next = t.u32(entries_end).unwrap_or(0) as usize;
        }
        if t.pages.is_empty() {
            return Err("the TIFF file has no pages".into());
        }
        Ok(t)
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let b = self.data.get(at..at + 2)?;
        Some(if self.big_endian { u16::from_be_bytes([b[0], b[1]]) } else { u16::from_le_bytes([b[0], b[1]]) })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b = self.data.get(at..at + 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if self.big_endian { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) })
    }

    fn field(&self, ifd: usize, tag: u16) -> Option<Field> {
        let count = self.u16(ifd)? as usize;
        for i in 0..count {
            let e = ifd + 2 + i * 12;
            if self.u16(e)? != tag {
                continue;
            }
            let kind = self.u16(e + 2)?;
            let n = self.u32(e + 4)? as usize;
            let size = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 => 4,
                5 | 10 | 12 => 8,
                _ => return None,
            };
            let at = if n.checked_mul(size)? <= 4 { e + 8 } else { self.u32(e + 8)? as usize };
            return Some(Field { kind, count: n, at });
        }
        None
    }

    /// The i-th value of an integer field.
    fn value(&self, f: &Field, i: usize) -> Option<u32> {
        if i >= f.count {
            return None;
        }
        match f.kind {
            1 | 6 | 7 => self.data.get(f.at + i).map(|&b| u32::from(b)),
            3 | 8 => self.u16(f.at + i * 2).map(u32::from),
            4 | 9 => self.u32(f.at + i * 4),
            _ => None,
        }
    }

    fn values(&self, f: &Field, limit: usize) -> Option<Vec<u32>> {
        if f.count > limit {
            return None;
        }
        (0..f.count).map(|i| self.value(f, i)).collect()
    }

    fn get(&self, ifd: usize, tag: u16) -> Option<u32> {
        self.field(ifd, tag).and_then(|f| self.value(&f, 0))
    }

    /// Dots per inch, if stated.
    fn resolution(&self, ifd: usize) -> Option<f32> {
        let f = self.field(ifd, 282)?;
        if f.kind != 5 {
            return None;
        }
        let (num, den) = (self.u32(f.at)?, self.u32(f.at + 4)?);
        if den == 0 || num == 0 {
            return None;
        }
        let unit = self.get(ifd, 296).unwrap_or(2);
        let v = num as f32 / den as f32;
        Some(if unit == 3 { v * 2.54 } else { v })
    }

    /// Page `index` as greyscale, upright, and its resolution if known.
    pub fn page(&self, index: usize, max_pixels: usize) -> Result<(Gray, Option<f32>), String> {
        let ifd = *self.pages.get(index).ok_or("no such page")?;
        let bad = || "the TIFF file is damaged".to_string();
        let width = self.get(ifd, 256).ok_or_else(bad)? as usize;
        let height = self.get(ifd, 257).ok_or_else(bad)? as usize;
        if width == 0 || height == 0 || width > 30_000 || height > 30_000 || width * height > max_pixels {
            return Err(format!("The scanned page is too large ({width} x {height} pixels)."));
        }
        let compression = self.get(ifd, 259).unwrap_or(1);
        let photometric = self.get(ifd, 262).unwrap_or(if (2..=4).contains(&compression) { 0 } else { 1 });
        let spp = self.get(ifd, 277).unwrap_or(1).clamp(1, 8) as usize;
        let bps = self.get(ifd, 258).unwrap_or(1) as usize;
        let planar = self.get(ifd, 284).unwrap_or(1);
        let predictor = self.get(ifd, 317).unwrap_or(1);
        let fill_lsb = self.get(ifd, 266) == Some(2);
        if planar == 2 && spp > 1 {
            return Err("This kind of TIFF file (separate colour planes) isn't supported.".into());
        }
        if ![1, 2, 4, 8, 16].contains(&bps) {
            return Err("This kind of TIFF file isn't supported.".into());
        }

        // Strips or tiles.
        let tiled = self.field(ifd, 322).is_some();
        let (tile_w, tile_h) = if tiled {
            (self.get(ifd, 322).ok_or_else(bad)? as usize, self.get(ifd, 323).ok_or_else(bad)? as usize)
        } else {
            (width, (self.get(ifd, 278).unwrap_or(height as u32) as usize).clamp(1, height))
        };
        if tile_w == 0 || tile_h == 0 || tile_w > 65_536 || tile_h > 65_536 {
            return Err(bad());
        }
        let (off_tag, len_tag) = if tiled { (324, 325) } else { (273, 279) };
        let offsets = self.field(ifd, off_tag).and_then(|f| self.values(&f, 1 << 20)).ok_or_else(bad)?;
        let lengths = self.field(ifd, len_tag).and_then(|f| self.values(&f, 1 << 20));
        let across = width.div_ceil(tile_w);
        let jpeg_tables = self.field(ifd, 347).and_then(|f| self.data.get(f.at..f.at + f.count));

        let mut gray = Gray::new(width, height, 255);
        for (i, &off) in offsets.iter().enumerate() {
            let (tx, ty) = if tiled { ((i % across) * tile_w, (i / across) * tile_h) } else { (0, i * tile_h) };
            if ty >= height {
                break;
            }
            let rows = if tiled { tile_h } else { tile_h.min(height - ty) };
            let off = off as usize;
            let len = match &lengths {
                Some(l) => *l.get(i).ok_or_else(bad)? as usize,
                None => self.data.len().saturating_sub(off),
            };
            let chunk = self.data.get(off..off.saturating_add(len).min(self.data.len())).ok_or_else(bad)?;
            let tile = match compression {
                2..=4 => {
                    let t4 = self.get(ifd, 292).unwrap_or(0);
                    let params = ccitt::Params {
                        k: match compression {
                            4 => -1,
                            3 if t4 & 1 != 0 => 1,
                            _ => 0,
                        },
                        columns: tile_w,
                        rows,
                        byte_align: compression == 3 && t4 & 4 != 0,
                        no_eol: compression == 2,
                        lsb_first: fill_lsb,
                    };
                    let (mut px, got) = ccitt::decode(chunk, &params, rows).map_err(|e| e.to_string())?;
                    // The codes describe 0 bits as "white" runs: right for
                    // WhiteIsZero (the usual); BlackIsZero means the opposite.
                    if photometric == 1 {
                        px.iter_mut().for_each(|v| *v = 255 - *v);
                    }
                    let mut g = Gray { width: tile_w, height: got, px };
                    g.px.resize(tile_w * rows, 255);
                    g.height = rows;
                    g
                }
                7 => {
                    let mut stream = Vec::new();
                    match jpeg_tables {
                        Some(t) if t.len() > 4 && chunk.len() > 2 => {
                            stream.extend_from_slice(&t[..t.len() - 2]);
                            stream.extend_from_slice(&chunk[2..]);
                        }
                        _ => stream.extend_from_slice(chunk),
                    }
                    let limits =
                        crate::image::Limits { max_width: 65_536, max_height: 65_536, max_pixels: max_pixels as u64 };
                    let img = crate::image::jpeg::decode(&stream, &limits).map_err(|e| e.to_string())?;
                    Gray::from_rgba(&img)
                }
                1 | 5 | 8 | 32946 | 32773 => {
                    let row_bytes = (tile_w * spp * bps).div_ceil(8);
                    let expected = row_bytes * rows;
                    let mut raw = match compression {
                        1 => chunk.to_vec(),
                        5 => lzw(chunk, expected, true)?,
                        8 | 32946 => {
                            crate::image::zlib::inflate_zlib_bounded(chunk, expected).map_err(|e| e.to_string())?
                        }
                        _ => packbits(chunk, expected),
                    };
                    raw.resize(expected, 0);
                    if predictor == 2 {
                        undo_horizontal_predictor(&mut raw, row_bytes, spp, bps);
                    }
                    samples_to_gray(&raw, tile_w, rows, spp, bps, photometric, self.colormap(ifd, bps))?
                }
                6 => return Err("This TIFF file uses old-style JPEG compression, which isn't supported.".into()),
                _ => return Err("This TIFF file's compression isn't supported.".into()),
            };
            // Copy into the page.
            for y in 0..rows.min(tile.height) {
                let py = ty + y;
                if py >= height {
                    break;
                }
                let n = tile.width.min(width.saturating_sub(tx));
                gray.px[py * width + tx..py * width + tx + n]
                    .copy_from_slice(&tile.px[y * tile.width..y * tile.width + n]);
            }
        }
        // Orientation tag.
        let gray = match self.get(ifd, 274).unwrap_or(1) {
            2 => gray.flip_horizontal(),
            3 => gray.rotate90(2),
            4 => gray.rotate90(2).flip_horizontal(),
            5 => gray.rotate90(1).flip_horizontal(),
            6 => gray.rotate90(1),
            7 => gray.rotate90(3).flip_horizontal(),
            8 => gray.rotate90(3),
            _ => gray,
        };
        Ok((gray, self.resolution(ifd)))
    }

    fn colormap(&self, ifd: usize, bps: usize) -> Option<Vec<u8>> {
        let f = self.field(ifd, 320)?;
        let n = 1usize << bps.min(16);
        if f.count != 3 * n {
            return None;
        }
        let v = self.values(&f, 3 * 65_536)?;
        Some((0..n).map(|i| luma((v[i] >> 8) as u8, (v[n + i] >> 8) as u8, (v[2 * n + i] >> 8) as u8)).collect())
    }
}

fn undo_horizontal_predictor(raw: &mut [u8], row_bytes: usize, spp: usize, bps: usize) {
    for row in raw.chunks_mut(row_bytes) {
        match bps {
            8 => {
                for i in spp..row.len() {
                    row[i] = row[i].wrapping_add(row[i - spp]);
                }
            }
            16 => {
                // Big- or little-endian makes no difference to the high
                // byte's use below only if consistent; treat as big-endian.
                let n = row.len() / 2;
                for i in spp..n {
                    let prev = u16::from_be_bytes([row[(i - spp) * 2], row[(i - spp) * 2 + 1]]);
                    let cur = u16::from_be_bytes([row[i * 2], row[i * 2 + 1]]);
                    let v = cur.wrapping_add(prev).to_be_bytes();
                    row[i * 2] = v[0];
                    row[i * 2 + 1] = v[1];
                }
            }
            _ => {}
        }
    }
}

/// Unpacked samples (rows of `width` pixels, `spp` samples each) to grey.
pub fn samples_to_gray(
    raw: &[u8],
    width: usize,
    height: usize,
    spp: usize,
    bps: usize,
    photometric: u32,
    palette: Option<Vec<u8>>,
) -> Result<Gray, String> {
    let row_bytes = (width * spp * bps).div_ceil(8);
    let mut out = Gray::new(width, height, 255);
    let sample = |row: &[u8], i: usize| -> u32 {
        match bps {
            8 => u32::from(row[i]),
            16 => u32::from(row[i * 2]),
            _ => {
                let bit = i * bps;
                let v = row[bit / 8] >> (8 - bps - bit % 8);
                u32::from(v & ((1u8 << bps) - 1))
            }
        }
    };
    let max = if bps >= 8 { 255 } else { (1u32 << bps) - 1 };
    let scale = |v: u32| (v * 255 / max) as u8;
    for y in 0..height {
        let Some(row) = raw.get(y * row_bytes..(y + 1) * row_bytes) else { break };
        for x in 0..width {
            let i = x * spp;
            let v = match photometric {
                0 => 255 - scale(sample(row, i)),
                1 => scale(sample(row, i)),
                2 if spp >= 3 => luma(scale(sample(row, i)), scale(sample(row, i + 1)), scale(sample(row, i + 2))),
                3 => match &palette {
                    Some(p) => p[sample(row, i) as usize % p.len()],
                    None => scale(sample(row, i)),
                },
                5 if spp >= 4 => {
                    let k = sample(row, i + 3);
                    let ink = |c: u32| 255 - (255u32.min(scale(c) as u32 + scale(k) as u32)) as u8;
                    luma(ink(sample(row, i)), ink(sample(row, i + 1)), ink(sample(row, i + 2)))
                }
                6 => scale(sample(row, i)),
                _ => return Err("This TIFF file's colour type isn't supported.".into()),
            };
            out.px[y * width + x] = v;
        }
    }
    Ok(out)
}

/// LZW as used in TIFF and PDF: MSB-first codes of 9 to 12 bits.
/// `early` = the code size grows one code early (TIFF, and PDF's default).
pub fn lzw(data: &[u8], limit: usize, early: bool) -> Result<Vec<u8>, String> {
    if data.len() >= 2 && data[0] == 0 && data[1] & 1 != 0 {
        return Err("This file uses an old LZW variant that isn't supported.".into());
    }
    let mut out = Vec::with_capacity(limit.min(1 << 26));
    let mut prefix: Vec<u16> = vec![0; 4096];
    let mut suffix: Vec<u8> = vec![0; 4096];
    let mut first: Vec<u8> = vec![0; 4096];
    let mut length: Vec<u32> = vec![0; 4096];
    for i in 0..256 {
        suffix[i] = i as u8;
        first[i] = i as u8;
        length[i] = 1;
    }
    let mut next = 258usize;
    let mut width = 9u32;
    let mut prev: Option<usize> = None;
    let (mut acc, mut bits, mut pos) = (0u32, 0u32, 0usize);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        while bits < width {
            let Some(&b) = data.get(pos) else { return Ok(out) };
            acc = acc << 8 | u32::from(b);
            bits += 8;
            pos += 1;
        }
        let code = ((acc >> (bits - width)) & ((1 << width) - 1)) as usize;
        bits -= width;
        acc &= (1 << bits) - 1;
        if code == 256 {
            next = 258;
            width = 9;
            prev = None;
            continue;
        }
        if code == 257 {
            break;
        }
        let entry = match prev {
            None => {
                if code > 255 {
                    return Err("damaged LZW data".into());
                }
                code
            }
            Some(p) => {
                if code > next || next >= 4096 && code >= next {
                    return Err("damaged LZW data".into());
                }
                if next < 4096 {
                    let first_char = if code == next { first[p] } else { first[code] };
                    prefix[next] = p as u16;
                    suffix[next] = first_char;
                    first[next] = first[p];
                    length[next] = length[p] + 1;
                    next += 1;
                }
                code
            }
        };
        // Write the string for `entry`.
        buf.clear();
        let mut c = entry;
        loop {
            buf.push(suffix[c]);
            if length[c] == 1 {
                break;
            }
            c = usize::from(prefix[c]);
        }
        buf.reverse();
        out.extend_from_slice(&buf);
        if out.len() >= limit {
            out.truncate(limit);
            return Ok(out);
        }
        prev = Some(entry);
        let grow_at = if early { next + 1 } else { next };
        width = match grow_at {
            n if n >= 2048 => 12,
            n if n >= 1024 => 11,
            n if n >= 512 => 10,
            _ => 9,
        };
    }
    Ok(out)
}

/// PackBits run-length decoding.
pub fn packbits(data: &[u8], limit: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(limit);
    let mut i = 0;
    while i < data.len() && out.len() < limit {
        let n = data[i] as i8;
        i += 1;
        if n >= 0 {
            let count = n as usize + 1;
            let end = (i + count).min(data.len());
            out.extend_from_slice(&data[i..end]);
            i = end;
        } else if n != -128 {
            let count = (1 - n as isize) as usize;
            if let Some(&b) = data.get(i) {
                out.extend(std::iter::repeat_n(b, count));
            }
            i += 1;
        }
    }
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packbits_runs() {
        // From Apple's description of the format.
        let data = [0xFE, 0xAA, 0x02, 0x80, 0x00, 0x2A, 0xFD, 0xAA, 0x03, 0x80, 0x00, 0x2A, 0x22, 0xF7, 0xAA];
        let want = [
            0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0xAA, 0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0x22, 0xAA, 0xAA, 0xAA, 0xAA,
            0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA,
        ];
        assert_eq!(packbits(&data, 100), want);
    }
}
