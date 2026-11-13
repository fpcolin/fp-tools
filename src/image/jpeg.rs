//! JPEG decoding: baseline and extended sequential (SOF0/SOF1) and
//! progressive (SOF2) Huffman-coded, 8-bit, with 1, 3 or 4 components, any
//! sampling factors 1-4, and restart markers.
//!
//! The arithmetic follows libjpeg-turbo's defaults as Pillow uses them
//! (integer "islow" IDCT, "fancy" upsampling for 2x1, 2x2 and 1x2 chroma,
//! fixed-point YCbCr conversion), so results match Pillow very closely.
//! CMYK/YCCK files are converted to RGB the way Pillow does.
//!
//! Not supported (reported as `Unsupported`): arithmetic coding, lossless,
//! hierarchical and 12-bit JPEGs. EXIF orientation is not applied (Pillow's
//! `Image.open` does not apply it either).

// Indexed loops read more naturally than iterator chains in codec code.
#![allow(clippy::needless_range_loop)]

use super::{DecodeError, Image, Limits};

/// Zigzag position -> natural (row-major) index.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21,
    28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54,
    47, 55, 62, 63,
];

/// Coefficient memory beyond this is refused, whatever the pixel limits say.
const MAX_COEFFICIENT_BYTES: u64 = 512 * 1024 * 1024;

// ------------------------------------------------------------ Huffman

const FAST_BITS: u32 = 9;

#[derive(Clone)]
struct HuffTable {
    /// (length << 8) | symbol for codes up to FAST_BITS, 0 otherwise.
    fast: Vec<u16>,
    /// Largest code of each length (-1 if none), index 1..=16.
    maxcode: [i32; 18],
    /// Offset from a code of each length to its symbol index.
    valoffset: [i32; 17],
    values: Vec<u8>,
}

impl HuffTable {
    fn new(counts: &[u8; 16], values: &[u8]) -> Result<HuffTable, DecodeError> {
        let mut fast = vec![0u16; 1 << FAST_BITS];
        let mut maxcode = [-1i32; 18];
        let mut valoffset = [0i32; 17];
        let mut code: i32 = 0;
        let mut k: i32 = 0;
        for len in 1..=16usize {
            let n = i32::from(counts[len - 1]);
            valoffset[len] = k - code;
            for _ in 0..n {
                if len as u32 <= FAST_BITS {
                    let shift = FAST_BITS - len as u32;
                    let start = (code as usize) << shift;
                    let entry = ((len as u16) << 8) | u16::from(values[k as usize]);
                    for slot in &mut fast[start..start + (1 << shift)] {
                        *slot = entry;
                    }
                }
                code += 1;
                k += 1;
            }
            if n > 0 {
                maxcode[len] = code - 1;
            }
            // Codes of this length must fit in `len` bits.
            if code > (1 << len) {
                return Err(DecodeError::Invalid("bad Huffman table"));
            }
            code <<= 1;
        }
        maxcode[17] = i32::MAX; // sentinel
        Ok(HuffTable { fast, maxcode, valoffset, values: values.to_vec() })
    }
}

// ------------------------------------------------------------ bit reader

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    bits: u64,
    count: u32,
    /// A marker was found; no more bytes are read from the entropy data.
    marker: bool,
    /// Zero bytes inserted because the file ended.
    eof_padding: u32,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Reader { data, pos, bits: 0, count: 0, marker: false, eof_padding: 0 }
    }

    fn fill(&mut self) {
        while self.count <= 56 {
            let byte = if self.marker {
                0
            } else if self.pos >= self.data.len() {
                self.eof_padding = self.eof_padding.saturating_add(1);
                0
            } else {
                let b = self.data[self.pos];
                if b == 0xFF {
                    match self.data.get(self.pos + 1) {
                        Some(0x00) => {
                            self.pos += 2;
                            0xFF
                        }
                        Some(0xFF) => {
                            self.pos += 1; // fill byte
                            continue;
                        }
                        Some(_) => {
                            // Stay on the marker; pad with zeros like libjpeg.
                            self.marker = true;
                            0
                        }
                        None => {
                            self.pos += 1;
                            continue;
                        }
                    }
                } else {
                    self.pos += 1;
                    b
                }
            };
            self.bits |= u64::from(byte) << (56 - self.count);
            self.count += 8;
        }
    }

    fn peek(&mut self, n: u32) -> u32 {
        if self.count < n {
            self.fill();
        }
        (self.bits >> (64 - n)) as u32
    }

    fn consume(&mut self, n: u32) {
        self.bits <<= n;
        self.count -= n;
    }

    fn bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.peek(n);
        self.consume(n);
        v
    }

    fn bit(&mut self) -> bool {
        self.bits(1) == 1
    }

    /// Value of an `s`-bit coefficient (JPEG's EXTEND).
    fn receive_extend(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.bits(s) as i32;
        if v < (1 << (s - 1)) { v - (1 << s) + 1 } else { v }
    }

    fn decode(&mut self, table: &HuffTable) -> Result<u8, DecodeError> {
        if self.count < 16 {
            self.fill();
        }
        let entry = table.fast[(self.bits >> (64 - FAST_BITS)) as usize];
        if entry != 0 {
            self.consume(u32::from(entry >> 8));
            return Ok(entry as u8);
        }
        for len in FAST_BITS as usize + 1..=16 {
            let code = (self.bits >> (64 - len)) as i32;
            if code <= table.maxcode[len] {
                self.consume(len as u32);
                let index = code + table.valoffset[len];
                return table.values.get(index as usize).copied().ok_or(DecodeError::Invalid("bad Huffman code"));
            }
        }
        Err(DecodeError::Invalid("bad Huffman code"))
    }

    /// True if decoding used bits that were only padding past end of file.
    fn ran_past_end(&self) -> bool {
        u64::from(self.eof_padding) * 8 > u64::from(self.count)
    }

    /// Restart marker: drop leftover bits and step over RSTn.
    fn restart(&mut self) {
        self.bits = 0;
        self.count = 0;
        self.marker = false;
        // Find the next marker; skip it if it is RSTn.
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xFF && self.data[self.pos + 1] != 0x00 && self.data[self.pos + 1] != 0xFF {
                if (0xD0..=0xD7).contains(&self.data[self.pos + 1]) {
                    self.pos += 2;
                }
                return;
            }
            self.pos += 1;
        }
    }
}

// ------------------------------------------------------------ frame

struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Real sample size of this component.
    width: usize,
    height: usize,
    /// Blocks per line / column allocated (padded to whole MCUs).
    bw: usize,
    bh: usize,
    coefs: Vec<i16>,
    quant: Option<[u16; 64]>,
    dc_pred: i32,
}

struct Frame {
    width: usize,
    height: usize,
    progressive: bool,
    comps: Vec<Component>,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
}

fn be16(d: &[u8], at: usize) -> Result<usize, DecodeError> {
    d.get(at..at + 2).map(|b| usize::from(u16::from_be_bytes([b[0], b[1]]))).ok_or(DecodeError::Truncated)
}

/// Body of the marker segment starting at `pos` (just after the marker).
fn segment(data: &[u8], pos: usize) -> Result<(&[u8], usize), DecodeError> {
    let len = be16(data, pos)?;
    if len < 2 {
        return Err(DecodeError::Invalid("bad segment length"));
    }
    let body = data.get(pos + 2..pos + len).ok_or(DecodeError::Truncated)?;
    Ok((body, pos + len))
}

pub fn decode(data: &[u8], limits: &Limits) -> Result<Image, DecodeError> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return Err(DecodeError::Invalid("not a JPEG file"));
    }
    let mut pos = 2;
    let mut frame: Option<Frame> = None;
    let mut qt: [Option<[u16; 64]>; 4] = [None; 4];
    let mut dc_tables: [Option<HuffTable>; 4] = [None, None, None, None];
    let mut ac_tables: [Option<HuffTable>; 4] = [None, None, None, None];
    let mut restart_interval = 0usize;
    let mut adobe_transform: Option<u8> = None;
    let mut scans = 0;

    loop {
        // Find the next marker, skipping any stray bytes between segments.
        while pos < data.len() && data[pos] != 0xFF {
            pos += 1;
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            break; // tolerate a missing EOI
        }
        let marker = data[pos];
        pos += 1;
        match marker {
            0xD8 | 0x01 | 0xD0..=0xD7 => {}
            0xD9 => break,
            0xC0..=0xC2 => {
                let (body, next) = segment(data, pos)?;
                pos = next;
                if frame.is_some() {
                    return Err(DecodeError::Invalid("more than one frame"));
                }
                frame = Some(parse_frame(body, marker == 0xC2, limits)?);
            }
            0xC3 | 0xC5..=0xC7 | 0xCB | 0xCD..=0xCF => {
                return Err(DecodeError::Unsupported("lossless or hierarchical JPEG"));
            }
            0xC9 | 0xCA => return Err(DecodeError::Unsupported("arithmetic-coded JPEG")),
            0xCC => return Err(DecodeError::Unsupported("arithmetic-coded JPEG")),
            0xC4 => {
                let (mut body, next) = segment(data, pos)?;
                pos = next;
                while !body.is_empty() {
                    let class = body[0] >> 4;
                    let id = usize::from(body[0] & 15);
                    let counts: [u8; 16] = body.get(1..17).ok_or(DecodeError::Truncated)?.try_into().unwrap_or([0; 16]);
                    let total: usize = counts.iter().map(|&c| usize::from(c)).sum();
                    if total > 256 || class > 1 || id > 3 {
                        return Err(DecodeError::Invalid("bad Huffman table"));
                    }
                    let values = body.get(17..17 + total).ok_or(DecodeError::Truncated)?;
                    let table = HuffTable::new(&counts, values)?;
                    if class == 0 {
                        dc_tables[id] = Some(table);
                    } else {
                        ac_tables[id] = Some(table);
                    }
                    body = &body[17 + total..];
                }
            }
            0xDB => {
                let (mut body, next) = segment(data, pos)?;
                pos = next;
                while !body.is_empty() {
                    let precision = body[0] >> 4;
                    let id = usize::from(body[0] & 15);
                    if id > 3 || precision > 1 {
                        return Err(DecodeError::Invalid("bad quantisation table"));
                    }
                    let size = if precision == 0 { 64 } else { 128 };
                    let values = body.get(1..1 + size).ok_or(DecodeError::Truncated)?;
                    let mut table = [0u16; 64];
                    for k in 0..64 {
                        let v = if precision == 0 {
                            u16::from(values[k])
                        } else {
                            u16::from_be_bytes([values[2 * k], values[2 * k + 1]])
                        };
                        table[ZIGZAG[k]] = v;
                    }
                    qt[id] = Some(table);
                    body = &body[1 + size..];
                }
            }
            0xDD => {
                let (body, next) = segment(data, pos)?;
                pos = next;
                restart_interval = be16(body, 0)?;
            }
            0xEE => {
                let (body, next) = segment(data, pos)?;
                pos = next;
                if body.len() >= 12 && body.starts_with(b"Adobe") {
                    adobe_transform = Some(body[11]);
                }
            }
            0xDA => {
                let (body, next) = segment(data, pos)?;
                let f = frame.as_mut().ok_or(DecodeError::Invalid("scan before frame header"))?;
                pos = decode_scan(data, next, body, f, &qt, &dc_tables, &ac_tables, restart_interval)?;
                scans += 1;
            }
            0xDC => return Err(DecodeError::Unsupported("JPEG with DNL marker")),
            _ => {
                let (_, next) = segment(data, pos)?;
                pos = next;
            }
        }
    }

    let frame = frame.ok_or(DecodeError::Invalid("no frame header"))?;
    if scans == 0 {
        return Err(DecodeError::Truncated);
    }
    Ok(output(&frame, adobe_transform))
}

fn parse_frame(body: &[u8], progressive: bool, limits: &Limits) -> Result<Frame, DecodeError> {
    if body.len() < 6 {
        return Err(DecodeError::Truncated);
    }
    if body[0] != 8 {
        return Err(DecodeError::Unsupported("12-bit JPEG"));
    }
    let height = be16(body, 1)?;
    let width = be16(body, 3)?;
    let n = usize::from(body[5]);
    if height == 0 {
        return Err(DecodeError::Unsupported("JPEG with DNL marker"));
    }
    limits.check(width as u32, height as u32)?;
    if !matches!(n, 1 | 3 | 4) {
        return Err(DecodeError::Unsupported("JPEG with an unusual number of colour channels"));
    }
    let specs = body.get(6..6 + 3 * n).ok_or(DecodeError::Truncated)?;
    let mut comps = Vec::with_capacity(n);
    for c in specs.as_chunks::<3>().0 {
        let (h, v) = (usize::from(c[1] >> 4), usize::from(c[1] & 15));
        if !(1..=4).contains(&h) || !(1..=4).contains(&v) || c[2] > 3 {
            return Err(DecodeError::Invalid("bad component"));
        }
        if comps.iter().any(|x: &Component| x.id == c[0]) {
            return Err(DecodeError::Invalid("duplicate component"));
        }
        comps.push(Component {
            id: c[0],
            h,
            v,
            tq: usize::from(c[2]),
            width: 0,
            height: 0,
            bw: 0,
            bh: 0,
            coefs: Vec::new(),
            quant: None,
            dc_pred: 0,
        });
    }
    let hmax = comps.iter().map(|c| c.h).max().unwrap_or(1);
    let vmax = comps.iter().map(|c| c.v).max().unwrap_or(1);
    let mcux = width.div_ceil(8 * hmax);
    let mcuy = height.div_ceil(8 * vmax);
    let mut total: u64 = 0;
    for c in &mut comps {
        c.width = (width * c.h).div_ceil(hmax);
        c.height = (height * c.v).div_ceil(vmax);
        c.bw = mcux * c.h;
        c.bh = mcuy * c.v;
        total += (c.bw * c.bh) as u64 * 64 * 2;
    }
    if total > MAX_COEFFICIENT_BYTES {
        return Err(DecodeError::TooLarge { width: width as u32, height: height as u32 });
    }
    for c in &mut comps {
        c.coefs = vec![0i16; c.bw * c.bh * 64];
    }
    Ok(Frame { width, height, progressive, comps, hmax, vmax, mcux, mcuy })
}

#[allow(clippy::too_many_arguments)]
fn decode_scan(
    data: &[u8],
    start: usize,
    header: &[u8],
    f: &mut Frame,
    qt: &[Option<[u16; 64]>; 4],
    dc_tables: &[Option<HuffTable>; 4],
    ac_tables: &[Option<HuffTable>; 4],
    restart_interval: usize,
) -> Result<usize, DecodeError> {
    let ns = usize::from(*header.first().ok_or(DecodeError::Truncated)?);
    if ns == 0 || ns > 4 || header.len() < 1 + 2 * ns + 3 {
        return Err(DecodeError::Invalid("bad scan header"));
    }
    let mut members = Vec::with_capacity(ns);
    for k in 0..ns {
        let id = header[1 + 2 * k];
        let tables = header[2 + 2 * k];
        let index = f.comps.iter().position(|c| c.id == id).ok_or(DecodeError::Invalid("unknown component in scan"))?;
        members.push((index, usize::from(tables >> 4), usize::from(tables & 15)));
    }
    let ss = usize::from(header[1 + 2 * ns]);
    let se = usize::from(header[2 + 2 * ns]);
    let ah = u32::from(header[3 + 2 * ns] >> 4);
    let al = u32::from(header[3 + 2 * ns] & 15);

    if f.progressive {
        let ok = if ss == 0 { se == 0 } else { ss <= se && se <= 63 && ns == 1 };
        if !ok || ah > 13 || al > 13 {
            return Err(DecodeError::Invalid("bad progressive scan"));
        }
    } else if ss != 0 || se != 63 || ah != 0 || al != 0 {
        // Some encoders write odd values here for baseline files; libjpeg
        // ignores them, and so do we.
    }

    // Tables needed by this scan, and quantisation tables latched for good.
    for &(index, td, ta) in &members {
        let comp = &mut f.comps[index];
        if comp.quant.is_none() {
            comp.quant = Some(qt[comp.tq].ok_or(DecodeError::Invalid("missing quantisation table"))?);
        }
        let needs_dc = !f.progressive || (ss == 0 && ah == 0);
        let needs_ac = !f.progressive || ss > 0;
        if (needs_dc && dc_tables.get(td).and_then(Option::as_ref).is_none())
            || (needs_ac && ac_tables.get(ta).and_then(Option::as_ref).is_none())
        {
            return Err(DecodeError::Invalid("missing Huffman table"));
        }
        comp.dc_pred = 0;
    }

    let mut reader = Reader::new(data, start);
    let mut eobrun: u32 = 0;
    let decode_block =
        |reader: &mut Reader<'_>, comp: &mut Component, block: usize, td: usize, ta: usize, eobrun: &mut u32| {
            let coefs = &mut comp.coefs[block * 64..block * 64 + 64];
            if !f.progressive {
                let dc = dc_tables[td].as_ref().unwrap_or_else(|| unreachable_table());
                let ac = ac_tables[ta].as_ref().unwrap_or_else(|| unreachable_table());
                decode_baseline(reader, dc, ac, &mut comp.dc_pred, coefs)
            } else if ss == 0 {
                if ah == 0 {
                    let dc = dc_tables[td].as_ref().unwrap_or_else(|| unreachable_table());
                    let t = u32::from(reader.decode(dc)?);
                    if t > 15 {
                        return Err(DecodeError::Invalid("bad DC difference"));
                    }
                    comp.dc_pred = comp.dc_pred.wrapping_add(reader.receive_extend(t));
                    coefs[0] = clamp16(comp.dc_pred.wrapping_shl(al));
                } else if reader.bit() {
                    coefs[0] |= 1 << al;
                }
                Ok(())
            } else {
                let ac = ac_tables[ta].as_ref().unwrap_or_else(|| unreachable_table());
                if ah == 0 {
                    decode_ac_first(reader, ac, coefs, ss, se, al, eobrun)
                } else {
                    decode_ac_refine(reader, ac, coefs, ss, se, al, eobrun)
                }
            }
        };

    if ns > 1 {
        let total = f.mcux * f.mcuy;
        for m in 0..total {
            if restart_interval > 0 && m > 0 && m % restart_interval == 0 {
                reader.restart();
                eobrun = 0;
                for &(index, ..) in &members {
                    f.comps[index].dc_pred = 0;
                }
            }
            let (mx, my) = (m % f.mcux, m / f.mcux);
            for &(index, td, ta) in &members {
                let comp = &mut f.comps[index];
                for v in 0..comp.v {
                    for h in 0..comp.h {
                        let block = (my * comp.v + v) * comp.bw + mx * comp.h + h;
                        decode_block(&mut reader, comp, block, td, ta, &mut eobrun)?;
                    }
                }
            }
        }
    } else {
        let (index, td, ta) = members[0];
        let (bw_real, bh_real, bw) = {
            let c = &f.comps[index];
            (c.width.div_ceil(8), c.height.div_ceil(8), c.bw)
        };
        for m in 0..bw_real * bh_real {
            if restart_interval > 0 && m > 0 && m % restart_interval == 0 {
                reader.restart();
                eobrun = 0;
                f.comps[index].dc_pred = 0;
            }
            let block = (m / bw_real) * bw + m % bw_real;
            decode_block(&mut reader, &mut f.comps[index], block, td, ta, &mut eobrun)?;
        }
    }
    if reader.ran_past_end() {
        return Err(DecodeError::Truncated);
    }
    Ok(reader.pos)
}

/// Tables are checked before decoding starts, so this cannot happen.
fn unreachable_table() -> &'static HuffTable {
    static EMPTY: std::sync::OnceLock<HuffTable> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| HuffTable {
        fast: vec![0; 1 << FAST_BITS],
        maxcode: [-1; 18],
        valoffset: [0; 17],
        values: Vec::new(),
    })
}

fn clamp16(v: i32) -> i16 {
    v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

fn decode_baseline(
    r: &mut Reader<'_>,
    dc: &HuffTable,
    ac: &HuffTable,
    pred: &mut i32,
    coefs: &mut [i16],
) -> Result<(), DecodeError> {
    let t = u32::from(r.decode(dc)?);
    if t > 15 {
        return Err(DecodeError::Invalid("bad DC difference"));
    }
    *pred = pred.wrapping_add(r.receive_extend(t));
    coefs[0] = clamp16(*pred);
    let mut k = 1;
    while k < 64 {
        let rs = r.decode(ac)?;
        let (run, s) = (usize::from(rs >> 4), u32::from(rs & 15));
        if s == 0 {
            if run != 15 {
                break;
            }
            k += 16;
            continue;
        }
        k += run;
        if k > 63 {
            return Err(DecodeError::Invalid("coefficient index out of range"));
        }
        coefs[ZIGZAG[k]] = clamp16(r.receive_extend(s));
        k += 1;
    }
    Ok(())
}

fn decode_ac_first(
    r: &mut Reader<'_>,
    ac: &HuffTable,
    coefs: &mut [i16],
    ss: usize,
    se: usize,
    al: u32,
    eobrun: &mut u32,
) -> Result<(), DecodeError> {
    if *eobrun > 0 {
        *eobrun -= 1;
        return Ok(());
    }
    let mut k = ss;
    while k <= se {
        let rs = r.decode(ac)?;
        let (run, s) = (u32::from(rs >> 4), u32::from(rs & 15));
        if s == 0 {
            if run < 15 {
                *eobrun = (1 << run) - 1 + r.bits(run);
                break;
            }
            k += 16;
            continue;
        }
        k += run as usize;
        if k > 63 {
            return Err(DecodeError::Invalid("coefficient index out of range"));
        }
        coefs[ZIGZAG[k]] = clamp16(r.receive_extend(s).wrapping_shl(al));
        k += 1;
    }
    Ok(())
}

/// Successive-approximation refinement of AC coefficients (ITU T.81 G.1.2.3),
/// structured as in stb_image.
fn decode_ac_refine(
    r: &mut Reader<'_>,
    ac: &HuffTable,
    coefs: &mut [i16],
    ss: usize,
    se: usize,
    al: u32,
    eobrun: &mut u32,
) -> Result<(), DecodeError> {
    let bit = 1i16 << al;
    let refine = |r: &mut Reader<'_>, c: &mut i16| {
        if *c != 0 && r.bit() && (*c & bit) == 0 {
            *c = if *c > 0 { c.saturating_add(bit) } else { c.saturating_sub(bit) };
        }
    };
    if *eobrun > 0 {
        *eobrun -= 1;
        for k in ss..=se {
            refine(r, &mut coefs[ZIGZAG[k]]);
        }
        return Ok(());
    }
    let mut k = ss;
    while k <= se {
        let rs = r.decode(ac)?;
        let (mut run, s) = (u32::from(rs >> 4), rs & 15);
        let mut value: i16 = 0;
        if s == 0 {
            if run < 15 {
                *eobrun = (1 << run) - 1 + r.bits(run);
                run = 64; // refine the rest of this block, then stop
            }
        } else {
            if s != 1 {
                return Err(DecodeError::Invalid("bad refinement code"));
            }
            value = if r.bit() { bit } else { -bit };
        }
        while k <= se {
            let c = &mut coefs[ZIGZAG[k]];
            k += 1;
            if *c != 0 {
                refine(r, c);
            } else {
                if run == 0 {
                    *c = value;
                    break;
                }
                run -= 1;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------ output

const CONST_BITS: i32 = 13;
const PASS1_BITS: i32 = 2;
const FIX_0_298631336: i32 = 2446;
const FIX_0_390180644: i32 = 3196;
const FIX_0_541196100: i32 = 4433;
const FIX_0_765366865: i32 = 6270;
const FIX_0_899976223: i32 = 7373;
const FIX_1_175875602: i32 = 9633;
const FIX_1_501321110: i32 = 12299;
const FIX_1_847759065: i32 = 15137;
const FIX_1_961570560: i32 = 16069;
const FIX_2_053119869: i32 = 16819;
const FIX_2_562915447: i32 = 20995;
const FIX_3_072711026: i32 = 25172;

fn descale(x: i32, n: i32) -> i32 {
    (x + (1 << (n - 1))) >> n
}

fn range_limit(x: i32) -> u8 {
    (x + 128).clamp(0, 255) as u8
}

/// libjpeg's jpeg_idct_islow: dequantise and inverse-DCT one block into
/// `out` (8 rows of `stride`).
fn idct_islow(coefs: &[i16], quant: &[u16; 64], out: &mut [u8], stride: usize) {
    let mut ws = [0i32; 64];
    let dq = |i: usize| i32::from(coefs[i]).wrapping_mul(i32::from(quant[i]));
    for col in 0..8 {
        if (1..8).all(|r| coefs[col + 8 * r] == 0) {
            let dc = dq(col).wrapping_shl(PASS1_BITS as u32);
            for r in 0..8 {
                ws[col + 8 * r] = dc;
            }
            continue;
        }
        let z2 = dq(col + 16);
        let z3 = dq(col + 48);
        let z1 = (z2 + z3).wrapping_mul(FIX_0_541196100);
        let tmp2 = z1.wrapping_add(z3.wrapping_mul(-FIX_1_847759065));
        let tmp3 = z1.wrapping_add(z2.wrapping_mul(FIX_0_765366865));
        let z2 = dq(col);
        let z3 = dq(col + 32);
        let tmp0 = (z2.wrapping_add(z3)).wrapping_shl(CONST_BITS as u32);
        let tmp1 = (z2.wrapping_sub(z3)).wrapping_shl(CONST_BITS as u32);
        let (tmp10, tmp13, tmp11, tmp12) =
            (tmp0.wrapping_add(tmp3), tmp0.wrapping_sub(tmp3), tmp1.wrapping_add(tmp2), tmp1.wrapping_sub(tmp2));
        let (o0, o1, o2, o3) = odd_part(dq(col + 56), dq(col + 40), dq(col + 24), dq(col + 8));
        let d = CONST_BITS - PASS1_BITS;
        ws[col] = descale(tmp10.wrapping_add(o3), d);
        ws[col + 56] = descale(tmp10.wrapping_sub(o3), d);
        ws[col + 8] = descale(tmp11.wrapping_add(o2), d);
        ws[col + 48] = descale(tmp11.wrapping_sub(o2), d);
        ws[col + 16] = descale(tmp12.wrapping_add(o1), d);
        ws[col + 40] = descale(tmp12.wrapping_sub(o1), d);
        ws[col + 24] = descale(tmp13.wrapping_add(o0), d);
        ws[col + 32] = descale(tmp13.wrapping_sub(o0), d);
    }
    for row in 0..8 {
        let w = &ws[row * 8..row * 8 + 8];
        let o = &mut out[row * stride..row * stride + 8];
        if w[1..].iter().all(|&v| v == 0) {
            let v = range_limit(descale(w[0], PASS1_BITS + 3));
            o.fill(v);
            continue;
        }
        let z1 = (w[2].wrapping_add(w[6])).wrapping_mul(FIX_0_541196100);
        let tmp2 = z1.wrapping_add(w[6].wrapping_mul(-FIX_1_847759065));
        let tmp3 = z1.wrapping_add(w[2].wrapping_mul(FIX_0_765366865));
        let tmp0 = (w[0].wrapping_add(w[4])).wrapping_shl(CONST_BITS as u32);
        let tmp1 = (w[0].wrapping_sub(w[4])).wrapping_shl(CONST_BITS as u32);
        let (tmp10, tmp13, tmp11, tmp12) =
            (tmp0.wrapping_add(tmp3), tmp0.wrapping_sub(tmp3), tmp1.wrapping_add(tmp2), tmp1.wrapping_sub(tmp2));
        let (o0, o1, o2, o3) = odd_part(w[7], w[5], w[3], w[1]);
        let d = CONST_BITS + PASS1_BITS + 3;
        o[0] = range_limit(descale(tmp10.wrapping_add(o3), d));
        o[7] = range_limit(descale(tmp10.wrapping_sub(o3), d));
        o[1] = range_limit(descale(tmp11.wrapping_add(o2), d));
        o[6] = range_limit(descale(tmp11.wrapping_sub(o2), d));
        o[2] = range_limit(descale(tmp12.wrapping_add(o1), d));
        o[5] = range_limit(descale(tmp12.wrapping_sub(o1), d));
        o[3] = range_limit(descale(tmp13.wrapping_add(o0), d));
        o[4] = range_limit(descale(tmp13.wrapping_sub(o0), d));
    }
}

/// The shared odd part of the islow IDCT for inputs 7, 5, 3, 1.
fn odd_part(t0: i32, t1: i32, t2: i32, t3: i32) -> (i32, i32, i32, i32) {
    let z1 = t0.wrapping_add(t3);
    let z2 = t1.wrapping_add(t2);
    let z3 = t0.wrapping_add(t2);
    let z4 = t1.wrapping_add(t3);
    let z5 = (z3.wrapping_add(z4)).wrapping_mul(FIX_1_175875602);
    let t0 = t0.wrapping_mul(FIX_0_298631336);
    let t1 = t1.wrapping_mul(FIX_2_053119869);
    let t2 = t2.wrapping_mul(FIX_3_072711026);
    let t3 = t3.wrapping_mul(FIX_1_501321110);
    let z1 = z1.wrapping_mul(-FIX_0_899976223);
    let z2 = z2.wrapping_mul(-FIX_2_562915447);
    let z3 = z3.wrapping_mul(-FIX_1_961570560).wrapping_add(z5);
    let z4 = z4.wrapping_mul(-FIX_0_390180644).wrapping_add(z5);
    (
        t0.wrapping_add(z1).wrapping_add(z3),
        t1.wrapping_add(z2).wrapping_add(z4),
        t2.wrapping_add(z2).wrapping_add(z3),
        t3.wrapping_add(z1).wrapping_add(z4),
    )
}

/// A component's decoded samples at its own resolution.
struct Plane {
    width: usize,
    height: usize,
    stride: usize,
    samples: Vec<u8>,
}

impl Plane {
    fn at(&self, x: usize, y: usize) -> i32 {
        i32::from(self.samples[y * self.stride + x])
    }
}

fn plane_of(c: &Component) -> Plane {
    let stride = c.bw * 8;
    let mut samples = vec![0u8; stride * c.bh * 8];
    let quant = c.quant.unwrap_or([0; 64]);
    for by in 0..c.bh {
        for bx in 0..c.bw {
            let block = by * c.bw + bx;
            let out = &mut samples[by * 8 * stride + bx * 8..];
            idct_islow(&c.coefs[block * 64..block * 64 + 64], &quant, out, stride);
        }
    }
    Plane { width: c.width, height: c.height, stride, samples }
}

/// Bring a plane to full resolution the way libjpeg-turbo does by default.
fn upsample(p: &Plane, hf: usize, vf: usize, width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    let last_x = p.width.saturating_sub(1);
    let last_y = p.height.saturating_sub(1);
    match (hf, vf) {
        (1, 1) => {
            for y in 0..height {
                for x in 0..width {
                    out[y * width + x] = p.at(x.min(last_x), y.min(last_y)) as u8;
                }
            }
        }
        // libjpeg-turbo only uses the fancy 2x filters when the component is
        // wider than two samples; narrower ones are replicated.
        (2, 1) if p.width > 2 => {
            for y in 0..height {
                let row = h2v1_row(|x| p.at(x, y.min(last_y)), p.width);
                for x in 0..width {
                    out[y * width + x] = row[x.min(row.len() - 1)];
                }
            }
        }
        (1, 2) => {
            for y in 0..height {
                let sy = (y / 2).min(last_y);
                let (other, bias) = if y % 2 == 0 { (sy.saturating_sub(1), 1) } else { ((sy + 1).min(last_y), 2) };
                for x in 0..width {
                    let sx = x.min(last_x);
                    out[y * width + x] = ((p.at(sx, sy) * 3 + p.at(sx, other) + bias) >> 2) as u8;
                }
            }
        }
        (2, 2) if p.width > 2 => {
            for y in 0..height {
                let sy = (y / 2).min(last_y);
                let other = if y % 2 == 0 { sy.saturating_sub(1) } else { (sy + 1).min(last_y) };
                let row = h2v2_row(|x| p.at(x, sy) * 3 + p.at(x, other), p.width);
                for x in 0..width {
                    out[y * width + x] = row[x.min(row.len() - 1)];
                }
            }
        }
        _ => {
            // Other ratios: replicate samples (libjpeg's int_upsample).
            for y in 0..height {
                for x in 0..width {
                    out[y * width + x] = p.at((x / hf).min(last_x), (y / vf).min(last_y)) as u8;
                }
            }
        }
    }
    out
}

/// libjpeg h2v1_fancy_upsample for one row of `n` samples.
fn h2v1_row(s: impl Fn(usize) -> i32, n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n * 2);
    if n == 1 {
        out.extend_from_slice(&[s(0) as u8, s(0) as u8]);
        return out;
    }
    for i in 0..n {
        let this = s(i) * 3;
        let even = if i == 0 { s(0) } else { (this + s(i - 1) + 1) >> 2 };
        let odd = if i == n - 1 { s(i) } else { (this + s(i + 1) + 2) >> 2 };
        out.push(even as u8);
        out.push(odd as u8);
    }
    out
}

/// libjpeg h2v2_fancy_upsample for one output row, given the vertically
/// mixed column sums `c(i) = 3 * nearer row + farther row`.
fn h2v2_row(c: impl Fn(usize) -> i32, n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let this = c(i);
        let even = if i == 0 { (this * 4 + 8) >> 4 } else { (this * 3 + c(i - 1) + 8) >> 4 };
        let odd = if i == n - 1 { (this * 4 + 7) >> 4 } else { (this * 3 + c(i + 1) + 7) >> 4 };
        out.push(even as u8);
        out.push(odd as u8);
    }
    out
}

fn muldiv255(a: i32, b: i32) -> i32 {
    let t = a * b + 128;
    ((t >> 8) + t) >> 8
}

fn output(f: &Frame, adobe_transform: Option<u8>) -> Image {
    let (w, h) = (f.width, f.height);
    let channels: Vec<Vec<u8>> = f
        .comps
        .iter()
        .map(|c| {
            let plane = plane_of(c);
            upsample(&plane, f.hmax / c.h.max(1), f.vmax / c.v.max(1), w, h)
        })
        .collect();
    let mut pixels = vec![255u8; w * h * 4];
    let ycc = |y: i32, cb: i32, cr: i32| -> [u8; 3] {
        let (cb, cr) = (cb - 128, cr - 128);
        let r = y + ((91_881 * cr + 32_768) >> 16);
        let g = y + ((-22_554 * cb - 46_802 * cr + 32_768) >> 16);
        let b = y + ((116_130 * cb + 32_768) >> 16);
        [r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8]
    };
    match channels.len() {
        1 => {
            for (i, &g) in channels[0].iter().enumerate() {
                pixels[i * 4..i * 4 + 3].fill(g);
            }
        }
        3 => {
            let ids: Vec<u8> = f.comps.iter().map(|c| c.id).collect();
            let rgb = adobe_transform == Some(0) || ids == b"RGB";
            for i in 0..w * h {
                let (a, b, c) = (channels[0][i], channels[1][i], channels[2][i]);
                let px = if rgb { [a, b, c] } else { ycc(i32::from(a), i32::from(b), i32::from(c)) };
                pixels[i * 4..i * 4 + 3].copy_from_slice(&px);
            }
        }
        _ => {
            for i in 0..w * h {
                let (a, b, c, k) = (channels[0][i], channels[1][i], channels[2][i], channels[3][i]);
                // What libjpeg hands back: CMYK as stored, or YCCK converted.
                let cmyk = if adobe_transform == Some(2) {
                    let [r, g, bl] = ycc(i32::from(a), i32::from(b), i32::from(c));
                    [255 - r, 255 - g, 255 - bl, k]
                } else {
                    [a, b, c, k]
                };
                // Adobe files store inverted ink; Pillow reads them as "CMYK;I".
                let ink = if adobe_transform.is_some() { cmyk.map(|v| 255 - v) } else { cmyk };
                let nk = 255 - i32::from(ink[3]);
                for ch in 0..3 {
                    pixels[i * 4 + ch] = (nk - muldiv255(i32::from(ink[ch]), nk)).clamp(0, 255) as u8;
                }
            }
        }
    }
    Image { width: w as u32, height: h as u32, has_alpha: false, pixels }
}
