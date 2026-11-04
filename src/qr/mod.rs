//! QR Code encoder (ISO/IEC 18004, model 2, versions 1-40).
//!
//! Data is split into numeric / alphanumeric / byte segments the same way the
//! Python `qrcode` package did (`optimize=20`), and the smallest version that
//! fits is chosen, so codes come out the same size as before. The mask is
//! chosen by the standard penalty rules.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EcLevel {
    /// ~7% recovery
    L,
    /// ~15% recovery
    M,
    /// ~25% recovery
    Q,
    /// ~30% recovery; required when a logo covers part of the code.
    H,
}

impl EcLevel {
    fn index(self) -> usize {
        match self {
            EcLevel::L => 0,
            EcLevel::M => 1,
            EcLevel::Q => 2,
            EcLevel::H => 3,
        }
    }

    fn format_bits(self) -> u32 {
        match self {
            EcLevel::L => 1,
            EcLevel::M => 0,
            EcLevel::Q => 3,
            EcLevel::H => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Numeric,
    Alphanumeric,
    Byte,
}

impl Mode {
    fn indicator(self) -> u32 {
        match self {
            Mode::Numeric => 0b0001,
            Mode::Alphanumeric => 0b0010,
            Mode::Byte => 0b0100,
        }
    }

    fn count_bits(self, version: u8) -> u8 {
        let group = match version {
            1..=9 => 0,
            10..=26 => 1,
            _ => 2,
        };
        match self {
            Mode::Numeric => [10, 12, 14][group],
            Mode::Alphanumeric => [9, 11, 13][group],
            Mode::Byte => [8, 16, 16][group],
        }
    }
}

const ALPHANUMERIC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    mode: Mode,
    /// Characters (bytes, for byte mode) in this segment.
    count: usize,
    bits: Vec<bool>,
}

impl Segment {
    /// Encode `data` in `mode`. The caller guarantees the bytes are valid for
    /// the mode (see `segments_for`).
    fn new(mode: Mode, data: &[u8]) -> Segment {
        let mut bits = Vec::new();
        match mode {
            Mode::Numeric => {
                for group in data.chunks(3) {
                    let value = group.iter().fold(0u32, |acc, &d| acc * 10 + u32::from(d - b'0'));
                    push_bits(&mut bits, value, group.len() * 3 + 1);
                }
            }
            Mode::Alphanumeric => {
                let code = |c: u8| ALPHANUMERIC.iter().position(|&a| a == c).unwrap_or(0) as u32;
                for pair in data.chunks(2) {
                    if pair.len() == 2 {
                        push_bits(&mut bits, code(pair[0]) * 45 + code(pair[1]), 11);
                    } else {
                        push_bits(&mut bits, code(pair[0]), 6);
                    }
                }
            }
            Mode::Byte => {
                for &b in data {
                    push_bits(&mut bits, u32::from(b), 8);
                }
            }
        }
        Segment { mode, count: data.len(), bits }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }
}

fn push_bits(bits: &mut Vec<bool>, value: u32, count: usize) {
    for i in (0..count).rev() {
        bits.push((value >> i) & 1 != 0);
    }
}

/// Split UTF-8 text into segments exactly as Python qrcode's
/// `optimal_data_chunks(data, minimum)` does: runs of at least `minimum`
/// digits become numeric segments, then runs of at least `minimum`
/// alphanumeric characters, and everything else is bytes. Data no longer than
/// `minimum` uses a single numeric or alphanumeric segment only if all of it
/// qualifies.
pub fn segments_for(data: &[u8], minimum: usize) -> Vec<Segment> {
    let is_digit = |b: &u8| b.is_ascii_digit();
    let is_alnum = |b: &u8| ALPHANUMERIC.contains(b);
    let whole = data.len() <= minimum;
    let mut out = Vec::new();
    for (numeric, chunk) in split_runs(data, &is_digit, whole, minimum) {
        if numeric {
            out.push(Segment::new(Mode::Numeric, chunk));
            continue;
        }
        for (alpha, sub) in split_runs(chunk, &is_alnum, whole, minimum) {
            out.push(Segment::new(if alpha { Mode::Alphanumeric } else { Mode::Byte }, sub));
        }
    }
    out
}

/// Alternate (matches, slice) pieces. With `whole` set the entire input must
/// qualify (the anchored regex); otherwise maximal runs of at least `minimum`.
fn split_runs<'a>(data: &'a [u8], class: &dyn Fn(&u8) -> bool, whole: bool, minimum: usize) -> Vec<(bool, &'a [u8])> {
    if data.is_empty() {
        return Vec::new();
    }
    if whole {
        // Python's `^x+$` also matches when one "\n" follows the run (`$`
        // matches before a final newline), splitting it off as plain data.
        if data.iter().all(class) {
            return vec![(true, data)];
        }
        if let Some((b'\n', head)) = data.split_last() {
            if !head.is_empty() && head.iter().all(class) {
                return vec![(true, head), (false, &data[head.len()..])];
            }
        }
        return vec![(false, data)];
    }
    let mut pieces = Vec::new();
    let mut plain_start = 0;
    let mut i = 0;
    while i < data.len() {
        if !class(&data[i]) {
            i += 1;
            continue;
        }
        let run_start = i;
        while i < data.len() && class(&data[i]) {
            i += 1;
        }
        if i - run_start >= minimum.max(1) {
            if plain_start < run_start {
                pieces.push((false, &data[plain_start..run_start]));
            }
            pieces.push((true, &data[run_start..i]));
            plain_start = i;
        }
    }
    if plain_start < data.len() {
        pieces.push((false, &data[plain_start..]));
    }
    pieces
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QrError {
    /// More data than the largest QR code (version 40) can hold.
    DataTooLong,
}

impl fmt::Display for QrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QrError::DataTooLong => f.write_str("there is too much text to fit in a QR code"),
        }
    }
}

impl std::error::Error for QrError {}

/// A finished symbol: a square grid of dark/light modules (no quiet zone).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrCode {
    version: u8,
    size: usize,
    mask: u8,
    modules: Vec<bool>,
    function: Vec<bool>,
}

impl QrCode {
    /// Encode text the way the Python build did: UTF-8, qrcode-style
    /// segmentation (minimum run 20), smallest fitting version.
    pub fn encode_text(text: &str, ecl: EcLevel) -> Result<QrCode, QrError> {
        QrCode::encode_segments(&segments_for(text.as_bytes(), 20), ecl, None)
    }

    /// Encode segments. `mask` forces a mask pattern (0-7); `None` picks the
    /// one with the lowest penalty.
    pub fn encode_segments(segments: &[Segment], ecl: EcLevel, mask: Option<u8>) -> Result<QrCode, QrError> {
        let (version, used) = (1..=40u8)
            .find_map(|v| {
                let used = bits_needed(segments, v)?;
                (used <= data_codewords(v, ecl) * 8).then_some((v, used))
            })
            .ok_or(QrError::DataTooLong)?;

        // Concatenate segments, then terminator and padding.
        let capacity = data_codewords(version, ecl) * 8;
        let mut bits = Vec::with_capacity(capacity);
        for seg in segments {
            push_bits(&mut bits, seg.mode.indicator(), 4);
            push_bits(&mut bits, seg.count as u32, usize::from(seg.mode.count_bits(version)));
            bits.extend_from_slice(&seg.bits);
        }
        debug_assert_eq!(bits.len(), used);
        bits.resize(bits.len() + (capacity - bits.len()).min(4), false);
        bits.resize(bits.len().div_ceil(8) * 8, false);
        let mut data: Vec<u8> =
            bits.chunks(8).map(|byte| byte.iter().fold(0u8, |acc, &b| (acc << 1) | u8::from(b))).collect();
        for pad in [0xEC, 0x11].iter().cycle() {
            if data.len() >= capacity / 8 {
                break;
            }
            data.push(*pad);
        }

        let codewords = add_ecc_and_interleave(&data, version, ecl);
        let mut qr = QrCode::blank(version);
        qr.draw_function_patterns(ecl);
        qr.draw_codewords(&codewords);

        let mask = match mask {
            Some(m) => m.min(7),
            // Same choice as Python qrcode: score each mask with the format
            // and version areas (and the dark module) left light, using its
            // penalty rules, and keep the first lowest. Codes therefore come
            // out identical to the previous version's.
            None => (0..8u8)
                .min_by_key(|&m| {
                    let mut trial = qr.clone();
                    trial.apply_mask(m);
                    trial.draw_format_and_version(0, 0, false);
                    trial.python_lost_point()
                })
                .unwrap_or(0),
        };
        qr.apply_mask(mask);
        qr.draw_format_bits(ecl, mask);
        qr.mask = mask;
        Ok(qr)
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn mask(&self) -> u8 {
        self.mask
    }

    /// Modules per side, without the quiet zone.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Whether the module at column `x`, row `y` is dark. Out of range = light.
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.modules[y * self.size + x]
    }

    fn blank(version: u8) -> QrCode {
        let size = usize::from(version) * 4 + 17;
        QrCode { version, size, mask: 0, modules: vec![false; size * size], function: vec![false; size * size] }
    }

    fn set_function(&mut self, x: usize, y: usize, dark: bool) {
        let i = y * self.size + x;
        self.modules[i] = dark;
        self.function[i] = true;
    }

    fn draw_function_patterns(&mut self, ecl: EcLevel) {
        let size = self.size;
        for i in 0..size {
            self.set_function(6, i, i % 2 == 0);
            self.set_function(i, 6, i % 2 == 0);
        }
        for (cx, cy) in [(3, 3), (size - 4, 3), (3, size - 4)] {
            for dy in -4i32..=4 {
                for dx in -4i32..=4 {
                    let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                    if (0..size as i32).contains(&x) && (0..size as i32).contains(&y) {
                        let dist = dx.abs().max(dy.abs());
                        self.set_function(x as usize, y as usize, dist != 2 && dist != 4);
                    }
                }
            }
        }
        let positions = alignment_positions(self.version);
        let last = positions.len().saturating_sub(1);
        for (i, &ay) in positions.iter().enumerate() {
            for (j, &ax) in positions.iter().enumerate() {
                if (i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0) {
                    continue; // these would overlap the finder patterns
                }
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        let dist = dx.abs().max(dy.abs());
                        self.set_function((ax as i32 + dx) as usize, (ay as i32 + dy) as usize, dist != 1);
                    }
                }
            }
        }
        self.draw_format_bits(ecl, 0); // reserves the areas; redrawn later
    }

    fn draw_format_bits(&mut self, ecl: EcLevel, mask: u8) {
        let data = (ecl.format_bits() << 3) | u32::from(mask);
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let format = ((data << 10) | rem) ^ 0x5412;
        self.draw_format_and_version(format, self.version_bits(), true);
    }

    /// The 18-bit version information block (versions 7 and up).
    fn version_bits(&self) -> u32 {
        let mut rem = u32::from(self.version);
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
        }
        (u32::from(self.version) << 12) | rem
    }

    fn draw_format_and_version(&mut self, format: u32, version: u32, dark_module: bool) {
        let bit = |i: usize| (format >> i) & 1 != 0;
        let size = self.size;
        for i in 0..=5 {
            self.set_function(8, i, bit(i));
        }
        self.set_function(8, 7, bit(6));
        self.set_function(8, 8, bit(7));
        self.set_function(7, 8, bit(8));
        for i in 9..15 {
            self.set_function(14 - i, 8, bit(i));
        }
        for i in 0..8 {
            self.set_function(size - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set_function(8, size - 15 + i, bit(i));
        }
        self.set_function(8, size - 8, dark_module);
        if self.version >= 7 {
            for i in 0..18 {
                let dark = (version >> i) & 1 != 0;
                let a = size - 11 + i % 3;
                let b = i / 3;
                self.set_function(a, b, dark);
                self.set_function(b, a, dark);
            }
        }
    }

    fn draw_codewords(&mut self, data: &[u8]) {
        let size = self.size;
        let total_bits = data.len() * 8;
        let mut i = 0;
        let mut right = size - 1;
        loop {
            if right == 6 {
                right = 5; // skip the vertical timing column
            }
            for vert in 0..size {
                for j in 0..2 {
                    let x = right - j;
                    let upward = (right + 1) & 2 == 0;
                    let y = if upward { size - 1 - vert } else { vert };
                    let idx = y * size + x;
                    if !self.function[idx] && i < total_bits {
                        self.modules[idx] = (data[i >> 3] >> (7 - (i & 7))) & 1 != 0;
                        i += 1;
                    }
                }
            }
            if right < 2 {
                break;
            }
            right -= 2;
        }
    }

    fn apply_mask(&mut self, mask: u8) {
        for y in 0..self.size {
            for x in 0..self.size {
                let invert = match mask {
                    0 => (x + y) % 2 == 0,
                    1 => y % 2 == 0,
                    2 => x % 3 == 0,
                    3 => (x + y) % 3 == 0,
                    4 => (x / 3 + y / 2) % 2 == 0,
                    5 => x * y % 2 + x * y % 3 == 0,
                    6 => (x * y % 2 + x * y % 3) % 2 == 0,
                    _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
                };
                let idx = y * self.size + x;
                if invert && !self.function[idx] {
                    self.modules[idx] = !self.modules[idx];
                }
            }
        }
    }

    /// Python qrcode's `util.lost_point`, reproduced exactly (including its
    /// float arithmetic for the dark-module balance) so the same mask wins.
    fn python_lost_point(&self) -> u64 {
        let n = self.size;
        let at = |row: usize, col: usize| self.modules[row * n + col];
        let mut lost = 0u64;

        // Level 1: runs of five or more same-coloured modules.
        for horizontal in [true, false] {
            for a in 0..n {
                let get = |b: usize| if horizontal { at(a, b) } else { at(b, a) };
                let mut previous = get(0);
                let mut length = 0u64;
                for b in 0..n {
                    if get(b) == previous {
                        length += 1;
                    } else {
                        if length >= 5 {
                            lost += length - 2;
                        }
                        length = 1;
                        previous = get(b);
                    }
                }
                if length >= 5 {
                    lost += length - 2;
                }
            }
        }

        // Level 2: 2x2 blocks of one colour.
        for row in 0..n - 1 {
            for col in 0..n - 1 {
                let c = at(row, col);
                if c == at(row, col + 1) && c == at(row + 1, col) && c == at(row + 1, col + 1) {
                    lost += 3;
                }
            }
        }

        // Level 3: 1:1:3:1:1 finder-like patterns with four light modules on
        // one side, looking only inside the symbol.
        const P1: [bool; 11] = [true, false, true, true, true, false, true, false, false, false, false];
        const P2: [bool; 11] = [false, false, false, false, true, false, true, true, true, false, true];
        if n > 10 {
            for horizontal in [true, false] {
                for a in 0..n {
                    for start in 0..n - 10 {
                        let window = |k: usize| if horizontal { at(a, start + k) } else { at(start + k, a) };
                        if (0..11).all(|k| window(k) == P1[k]) || (0..11).all(|k| window(k) == P2[k]) {
                            lost += 40;
                        }
                    }
                }
            }
        }

        // Level 4: every 5% away from half dark costs 10.
        let dark = self.modules.iter().filter(|&&m| m).count();
        let percent = dark as f64 / (n * n) as f64;
        let rating = ((percent * 100.0 - 50.0).abs() / 5.0) as u64;
        lost + rating * 10
    }
}

/// Total bits for the segments at `version`, or None if a count overflows
/// its field.
fn bits_needed(segments: &[Segment], version: u8) -> Option<usize> {
    let mut total = 0usize;
    for seg in segments {
        let count_bits = seg.mode.count_bits(version);
        if seg.count >= 1usize << count_bits {
            return None;
        }
        total += 4 + usize::from(count_bits) + seg.bits.len();
    }
    Some(total)
}

fn alignment_positions(version: u8) -> Vec<usize> {
    if version == 1 {
        return Vec::new();
    }
    let v = usize::from(version);
    let count = v / 7 + 2;
    let step = (v * 8 + count * 3 + 5) / (count * 4 - 4) * 2;
    let last = v * 4 + 17 - 7;
    let mut result = vec![6; count];
    for (k, slot) in result.iter_mut().skip(1).rev().enumerate() {
        *slot = last - k * step;
    }
    result
}

/// Modules available for data and ECC at `version`.
fn raw_data_modules(version: u8) -> usize {
    let v = usize::from(version);
    let mut result = (16 * v + 128) * v + 64;
    if v >= 2 {
        let align = v / 7 + 2;
        result -= (25 * align - 10) * align - 55;
        if v >= 7 {
            result -= 36;
        }
    }
    result
}

fn data_codewords(version: u8, ecl: EcLevel) -> usize {
    let v = usize::from(version);
    raw_data_modules(version) / 8 - usize::from(ECC_PER_BLOCK[ecl.index()][v]) * usize::from(ECC_BLOCKS[ecl.index()][v])
}

fn add_ecc_and_interleave(data: &[u8], version: u8, ecl: EcLevel) -> Vec<u8> {
    let v = usize::from(version);
    let blocks = usize::from(ECC_BLOCKS[ecl.index()][v]);
    let ecc_len = usize::from(ECC_PER_BLOCK[ecl.index()][v]);
    let raw = raw_data_modules(version) / 8;
    let short_blocks = blocks - raw % blocks;
    let short_len = raw / blocks;

    let divisor = rs_divisor(ecc_len);
    let mut split: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(blocks);
    let mut k = 0;
    for i in 0..blocks {
        let len = short_len - ecc_len + usize::from(i >= short_blocks);
        let block = data[k..k + len].to_vec();
        k += len;
        let ecc = rs_remainder(&block, &divisor);
        split.push((block, ecc));
    }

    let mut out = Vec::with_capacity(raw);
    let longest = short_len - ecc_len + 1;
    for i in 0..longest {
        for (block, _) in &split {
            if let Some(&b) = block.get(i) {
                out.push(b);
            }
        }
    }
    for i in 0..ecc_len {
        for (_, ecc) in &split {
            out.push(ecc[i]);
        }
    }
    out
}

fn gf_mul(x: u8, y: u8) -> u8 {
    let mut z: u32 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= ((u32::from(y) >> i) & 1) * u32::from(x);
    }
    z as u8
}

fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_mul(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_mul(root, 0x02);
    }
    result
}

fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &b in data {
        let factor = b ^ result[0];
        result.remove(0);
        result.push(0);
        for (r, &d) in result.iter_mut().zip(divisor) {
            *r ^= gf_mul(d, factor);
        }
    }
    result
}

// Index 0 is unused so the version number can index directly.
#[rustfmt::skip]
const ECC_PER_BLOCK: [[u8; 41]; 4] = [
    [0, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28, 30, 30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
    [0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28],
    [0, 13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28, 30, 30, 30, 30, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
    [0, 17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30, 24, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
];

#[rustfmt::skip]
const ECC_BLOCKS: [[u8; 41]; 4] = [
    [0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25],
    [0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49],
    [0, 1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25, 27, 29, 34, 34, 35, 38, 40, 43, 45, 48, 51, 53, 56, 59, 62, 65, 68],
    [0, 1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30, 32, 35, 37, 40, 42, 45, 48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacities_match_the_standard() {
        // Byte-mode capacity at level H for a few versions (ISO 18004 table 7).
        for (version, bytes) in [(1u8, 7usize), (2, 14), (10, 119), (27, 625), (40, 1273)] {
            let header = 4 + usize::from(Mode::Byte.count_bits(version));
            assert_eq!((data_codewords(version, EcLevel::H) * 8 - header) / 8, bytes, "v{version}");
        }
        assert_eq!(data_codewords(40, EcLevel::L), 2956);
    }

    #[test]
    fn segmentation_matches_python_qrcode() {
        let modes = |s: &str| segments_for(s.as_bytes(), 20).iter().map(|s| s.mode).collect::<Vec<_>>();
        assert_eq!(modes("https://example.com"), vec![Mode::Byte]);
        assert_eq!(modes("12345"), vec![Mode::Numeric]);
        assert_eq!(modes("HELLO WORLD"), vec![Mode::Alphanumeric]);
        assert_eq!(modes("abc12345678901234567890xyz"), vec![Mode::Byte, Mode::Numeric, Mode::Byte]);
        assert_eq!(modes("x1234567890123456789y"), vec![Mode::Byte]);
        // Python's `$` matches before a final newline.
        assert_eq!(modes("6\n"), vec![Mode::Numeric, Mode::Byte]);
        assert_eq!(modes("AB\n"), vec![Mode::Alphanumeric, Mode::Byte]);
        assert!(modes("").is_empty());
    }

    #[test]
    fn too_long() {
        assert_eq!(QrCode::encode_text(&"a".repeat(1274), EcLevel::H), Err(QrError::DataTooLong));
        assert_eq!(QrCode::encode_text(&"a".repeat(1273), EcLevel::H).unwrap().version(), 40);
    }
}
