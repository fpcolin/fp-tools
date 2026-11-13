//! CCITT fax decoding (ITU-T T.4 "Group 3", one- and two-dimensional, and
//! T.6 "Group 4"): the compression black-and-white scanners use in TIFF and
//! PDF files.
//!
//! Output is one byte per pixel: 255 for white runs, 0 for black.

/// How the data is coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// 0: one-dimensional (Modified Huffman); > 0: mixed one- and
    /// two-dimensional (each line says which, after its end-of-line code);
    /// < 0: pure two-dimensional (Group 4). As in PDF's /K.
    pub k: i32,
    pub columns: usize,
    /// Rows expected; 0 = until the data ends.
    pub rows: usize,
    /// Each line starts on a byte boundary.
    pub byte_align: bool,
    /// TIFF compression 2 (Modified Huffman without end-of-line codes, each
    /// row byte-aligned).
    pub no_eol: bool,
    /// Bits are stored least significant first (TIFF FillOrder 2).
    pub lsb_first: bool,
}

/// The decoded image: `columns` x rows, 255 = white, 0 = black.
pub fn decode(data: &[u8], p: &Params, max_rows: usize) -> Result<(Vec<u8>, usize), &'static str> {
    if p.columns == 0 || p.columns > 65_536 {
        return Err("bad fax image width");
    }
    let tables = tables();
    let mut r = Bits { data, pos: 0, lsb: p.lsb_first };
    let w = p.columns;
    let mut out: Vec<u8> = Vec::new();
    // Changing elements of the reference (previous) line: positions where
    // the colour changes, starting with white -> black. Sentinels at the end.
    let mut reference: Vec<u32> = vec![w as u32, w as u32];
    let mut current: Vec<u32> = Vec::with_capacity(w + 4);
    let mut rows = 0usize;
    let limit = if p.rows > 0 { p.rows.min(max_rows) } else { max_rows };
    while rows < limit {
        // Group 3 aligns by putting fill bits before the end-of-line code,
        // so only Group 4 and end-of-line-less data align here.
        if p.no_eol || (p.k < 0 && p.byte_align) {
            r.align();
        }
        let mut two_d = p.k < 0;
        if p.k >= 0 && !p.no_eol {
            // Optional end-of-line code (with fill bits before it).
            match r.skip_eol() {
                Eol::Found => {}
                Eol::EndOfData => break,
                Eol::None => {}
            }
            if p.k > 0 {
                match r.bit() {
                    Some(b) => two_d = b == 0,
                    None => break,
                }
            }
            // Another end-of-line straight away: end of the page.
            if r.peek(12) == 1 {
                break;
            }
        } else if p.k < 0 && r.at_eofb() {
            break;
        }
        if r.remaining() == 0 {
            break;
        }
        current.clear();
        let ok = if two_d {
            decode_2d(&mut r, &tables, &reference, &mut current, w)
        } else {
            decode_1d(&mut r, &tables, &mut current, w)
        };
        if ok.is_err() {
            // Damaged data: keep the rows decoded so far, if any.
            if rows == 0 {
                return Err("damaged fax data");
            }
            break;
        }
        // Pixels from the changes.
        let start = out.len();
        out.resize(start + w, 255);
        let row = &mut out[start..];
        let mut x = 0usize;
        let mut black = false;
        for &c in current.iter().chain(std::iter::once(&(w as u32))) {
            let c = (c as usize).min(w);
            if black && c > x {
                row[x..c].fill(0);
            }
            x = x.max(c);
            black = !black;
        }
        // The next reference line: drop zero-length runs (pairs of equal
        // positions keep the colour order), then sentinels.
        reference.clear();
        for &c in &current {
            let c = c.min(w as u32);
            if reference.last() == Some(&c) {
                reference.pop();
            } else {
                reference.push(c);
            }
        }
        reference.push(w as u32);
        reference.push(w as u32);
        rows += 1;
    }
    if rows == 0 {
        return Err("no fax image data");
    }
    Ok((out, rows))
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    lsb: bool,
}

enum Eol {
    Found,
    None,
    EndOfData,
}

impl Bits<'_> {
    fn remaining(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.pos)
    }

    fn bit_at(&self, pos: usize) -> Option<u32> {
        let byte = *self.data.get(pos / 8)?;
        let shift = if self.lsb { pos % 8 } else { 7 - pos % 8 };
        Some(u32::from(byte >> shift) & 1)
    }

    fn bit(&mut self) -> Option<u32> {
        let b = self.bit_at(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    /// The next `n` (<= 16) bits, zero-padded past the end.
    fn peek(&self, n: usize) -> u32 {
        let mut v = 0;
        for i in 0..n {
            v = v << 1 | self.bit_at(self.pos + i).unwrap_or(0);
        }
        v
    }

    fn align(&mut self) {
        self.pos = self.pos.next_multiple_of(8);
    }

    /// Skip fill bits and an end-of-line code (000000000001) if one is next.
    fn skip_eol(&mut self) -> Eol {
        let mut zeros = 0;
        let mut p = self.pos;
        loop {
            match self.bit_at(p) {
                None => return if zeros > 0 || self.remaining() == 0 { Eol::EndOfData } else { Eol::None },
                Some(0) => {
                    zeros += 1;
                    p += 1;
                }
                Some(_) => break,
            }
        }
        if zeros >= 11 {
            self.pos = p + 1;
            Eol::Found
        } else {
            Eol::None
        }
    }

    /// End of facsimile block (two end-of-line codes) in Group 4 data, or
    /// nothing but padding left.
    fn at_eofb(&self) -> bool {
        self.peek(12) == 1 && self.remaining() >= 12 || self.remaining() < 8 && self.peek(self.remaining()) == 0
    }
}

/// Code tables, looked up by the next 13 bits.
struct Tables {
    /// (length, run) per 13-bit prefix; length 0 = invalid.
    white: Vec<(u8, u16)>,
    black: Vec<(u8, u16)>,
}

const PEEK: usize = 13;

fn build(codes: &[(u16, u8, u16)]) -> Vec<(u8, u16)> {
    let mut t = vec![(0u8, 0u16); 1 << PEEK];
    for &(code, len, run) in codes {
        let shift = PEEK - usize::from(len);
        let first = usize::from(code) << shift;
        for e in t.iter_mut().skip(first).take(1 << shift) {
            *e = (len, run);
        }
    }
    t
}

/// (code, length in bits, run length).
const WHITE: [(u16, u8, u16); 91] = [
    (0b00110101, 8, 0),
    (0b000111, 6, 1),
    (0b0111, 4, 2),
    (0b1000, 4, 3),
    (0b1011, 4, 4),
    (0b1100, 4, 5),
    (0b1110, 4, 6),
    (0b1111, 4, 7),
    (0b10011, 5, 8),
    (0b10100, 5, 9),
    (0b00111, 5, 10),
    (0b01000, 5, 11),
    (0b001000, 6, 12),
    (0b000011, 6, 13),
    (0b110100, 6, 14),
    (0b110101, 6, 15),
    (0b101010, 6, 16),
    (0b101011, 6, 17),
    (0b0100111, 7, 18),
    (0b0001100, 7, 19),
    (0b0001000, 7, 20),
    (0b0010111, 7, 21),
    (0b0000011, 7, 22),
    (0b0000100, 7, 23),
    (0b0101000, 7, 24),
    (0b0101011, 7, 25),
    (0b0010011, 7, 26),
    (0b0100100, 7, 27),
    (0b0011000, 7, 28),
    (0b00000010, 8, 29),
    (0b00000011, 8, 30),
    (0b00011010, 8, 31),
    (0b00011011, 8, 32),
    (0b00010010, 8, 33),
    (0b00010011, 8, 34),
    (0b00010100, 8, 35),
    (0b00010101, 8, 36),
    (0b00010110, 8, 37),
    (0b00010111, 8, 38),
    (0b00101000, 8, 39),
    (0b00101001, 8, 40),
    (0b00101010, 8, 41),
    (0b00101011, 8, 42),
    (0b00101100, 8, 43),
    (0b00101101, 8, 44),
    (0b00000100, 8, 45),
    (0b00000101, 8, 46),
    (0b00001010, 8, 47),
    (0b00001011, 8, 48),
    (0b01010010, 8, 49),
    (0b01010011, 8, 50),
    (0b01010100, 8, 51),
    (0b01010101, 8, 52),
    (0b00100100, 8, 53),
    (0b00100101, 8, 54),
    (0b01011000, 8, 55),
    (0b01011001, 8, 56),
    (0b01011010, 8, 57),
    (0b01011011, 8, 58),
    (0b01001010, 8, 59),
    (0b01001011, 8, 60),
    (0b00110010, 8, 61),
    (0b00110011, 8, 62),
    (0b00110100, 8, 63),
    // Make-up codes.
    (0b11011, 5, 64),
    (0b10010, 5, 128),
    (0b010111, 6, 192),
    (0b0110111, 7, 256),
    (0b00110110, 8, 320),
    (0b00110111, 8, 384),
    (0b01100100, 8, 448),
    (0b01100101, 8, 512),
    (0b01101000, 8, 576),
    (0b01100111, 8, 640),
    (0b011001100, 9, 704),
    (0b011001101, 9, 768),
    (0b011010010, 9, 832),
    (0b011010011, 9, 896),
    (0b011010100, 9, 960),
    (0b011010101, 9, 1024),
    (0b011010110, 9, 1088),
    (0b011010111, 9, 1152),
    (0b011011000, 9, 1216),
    (0b011011001, 9, 1280),
    (0b011011010, 9, 1344),
    (0b011011011, 9, 1408),
    (0b010011000, 9, 1472),
    (0b010011001, 9, 1536),
    (0b010011010, 9, 1600),
    (0b011000, 6, 1664),
    (0b010011011, 9, 1728),
];

const BLACK: [(u16, u8, u16); 91] = [
    (0b0000110111, 10, 0),
    (0b010, 3, 1),
    (0b11, 2, 2),
    (0b10, 2, 3),
    (0b011, 3, 4),
    (0b0011, 4, 5),
    (0b0010, 4, 6),
    (0b00011, 5, 7),
    (0b000101, 6, 8),
    (0b000100, 6, 9),
    (0b0000100, 7, 10),
    (0b0000101, 7, 11),
    (0b0000111, 7, 12),
    (0b00000100, 8, 13),
    (0b00000111, 8, 14),
    (0b000011000, 9, 15),
    (0b0000010111, 10, 16),
    (0b0000011000, 10, 17),
    (0b0000001000, 10, 18),
    (0b00001100111, 11, 19),
    (0b00001101000, 11, 20),
    (0b00001101100, 11, 21),
    (0b00000110111, 11, 22),
    (0b00000101000, 11, 23),
    (0b00000010111, 11, 24),
    (0b00000011000, 11, 25),
    (0b000011001010, 12, 26),
    (0b000011001011, 12, 27),
    (0b000011001100, 12, 28),
    (0b000011001101, 12, 29),
    (0b000001101000, 12, 30),
    (0b000001101001, 12, 31),
    (0b000001101010, 12, 32),
    (0b000001101011, 12, 33),
    (0b000011010010, 12, 34),
    (0b000011010011, 12, 35),
    (0b000011010100, 12, 36),
    (0b000011010101, 12, 37),
    (0b000011010110, 12, 38),
    (0b000011010111, 12, 39),
    (0b000001101100, 12, 40),
    (0b000001101101, 12, 41),
    (0b000011011010, 12, 42),
    (0b000011011011, 12, 43),
    (0b000001010100, 12, 44),
    (0b000001010101, 12, 45),
    (0b000001010110, 12, 46),
    (0b000001010111, 12, 47),
    (0b000001100100, 12, 48),
    (0b000001100101, 12, 49),
    (0b000001010010, 12, 50),
    (0b000001010011, 12, 51),
    (0b000000100100, 12, 52),
    (0b000000110111, 12, 53),
    (0b000000111000, 12, 54),
    (0b000000100111, 12, 55),
    (0b000000101000, 12, 56),
    (0b000001011000, 12, 57),
    (0b000001011001, 12, 58),
    (0b000000101011, 12, 59),
    (0b000000101100, 12, 60),
    (0b000001011010, 12, 61),
    (0b000001100110, 12, 62),
    (0b000001100111, 12, 63),
    // Make-up codes.
    (0b0000001111, 10, 64),
    (0b000011001000, 12, 128),
    (0b000011001001, 12, 192),
    (0b000001011011, 12, 256),
    (0b000000110011, 12, 320),
    (0b000000110100, 12, 384),
    (0b000000110101, 12, 448),
    (0b0000001101100, 13, 512),
    (0b0000001101101, 13, 576),
    (0b0000001001010, 13, 640),
    (0b0000001001011, 13, 704),
    (0b0000001001100, 13, 768),
    (0b0000001001101, 13, 832),
    (0b0000001110010, 13, 896),
    (0b0000001110011, 13, 960),
    (0b0000001110100, 13, 1024),
    (0b0000001110101, 13, 1088),
    (0b0000001110110, 13, 1152),
    (0b0000001110111, 13, 1216),
    (0b0000001010010, 13, 1280),
    (0b0000001010011, 13, 1344),
    (0b0000001010100, 13, 1408),
    (0b0000001010101, 13, 1472),
    (0b0000001011010, 13, 1536),
    (0b0000001011011, 13, 1600),
    (0b0000001100100, 13, 1664),
    (0b0000001100101, 13, 1728),
];

/// Extended make-up codes, shared by both colours.
const EXTENDED: [(u16, u8, u16); 13] = [
    (0b00000001000, 11, 1792),
    (0b00000001100, 11, 1856),
    (0b00000001101, 11, 1920),
    (0b000000010010, 12, 1984),
    (0b000000010011, 12, 2048),
    (0b000000010100, 12, 2112),
    (0b000000010101, 12, 2176),
    (0b000000010110, 12, 2240),
    (0b000000010111, 12, 2304),
    (0b000000011100, 12, 2368),
    (0b000000011101, 12, 2432),
    (0b000000011110, 12, 2496),
    (0b000000011111, 12, 2560),
];

fn tables() -> Tables {
    let mut white = WHITE.to_vec();
    white.extend_from_slice(&EXTENDED);
    let mut black = BLACK.to_vec();
    black.extend_from_slice(&EXTENDED);
    Tables { white: build(&white), black: build(&black) }
}

/// One run of `black` or white: make-up codes, then a terminating code.
fn run(r: &mut Bits<'_>, t: &Tables, black: bool) -> Result<usize, ()> {
    let table = if black { &t.black } else { &t.white };
    let mut total = 0usize;
    loop {
        if r.remaining() == 0 {
            return Err(());
        }
        let (len, value) = table[r.peek(PEEK) as usize];
        if len == 0 {
            return Err(());
        }
        r.pos += usize::from(len);
        total += usize::from(value);
        if value < 64 {
            return Ok(total);
        }
    }
}

fn decode_1d(r: &mut Bits<'_>, t: &Tables, changes: &mut Vec<u32>, w: usize) -> Result<(), ()> {
    let mut x = 0usize;
    let mut black = false;
    while x < w {
        let n = run(r, t, black)?;
        x = (x + n).min(w);
        changes.push(x as u32);
        black = !black;
    }
    Ok(())
}

/// Two-dimensional modes.
enum Mode {
    Pass,
    Horizontal,
    Vertical(i32),
}

fn mode(r: &mut Bits<'_>) -> Result<Mode, ()> {
    let v = r.peek(7);
    let (len, m) = if v >> 6 == 1 {
        (1, Mode::Vertical(0))
    } else if v >> 4 == 0b011 {
        (3, Mode::Vertical(1))
    } else if v >> 4 == 0b010 {
        (3, Mode::Vertical(-1))
    } else if v >> 4 == 0b001 {
        (3, Mode::Horizontal)
    } else if v >> 3 == 0b0001 {
        (4, Mode::Pass)
    } else if v >> 1 == 0b000011 {
        (6, Mode::Vertical(2))
    } else if v >> 1 == 0b000010 {
        (6, Mode::Vertical(-2))
    } else if v == 0b0000011 {
        (7, Mode::Vertical(3))
    } else if v == 0b0000010 {
        (7, Mode::Vertical(-3))
    } else {
        // Extensions (uncompressed mode) and end-of-line codes mid-line.
        return Err(());
    };
    if r.remaining() < len {
        return Err(());
    }
    r.pos += len;
    Ok(m)
}

fn decode_2d(r: &mut Bits<'_>, t: &Tables, reference: &[u32], changes: &mut Vec<u32>, w: usize) -> Result<(), ()> {
    let mut a0: i64 = -1;
    let mut black = false;
    while a0 < w as i64 {
        // b1: first change on the reference line right of a0 to the colour
        // opposite a0's (white->black changes are at even indices).
        let want = usize::from(black);
        let mut i = reference.partition_point(|&c| i64::from(c) <= a0);
        if i % 2 != want {
            i += 1;
        }
        let b1 = i64::from(*reference.get(i).unwrap_or(&(w as u32)));
        let b2 = i64::from(*reference.get(i + 1).unwrap_or(&(w as u32)));
        match mode(r)? {
            Mode::Pass => {
                a0 = b2;
            }
            Mode::Horizontal => {
                let start = a0.max(0);
                let first = run(r, t, black)? as i64;
                let second = run(r, t, !black)? as i64;
                let a1 = (start + first).min(w as i64);
                let a2 = (a1 + second).min(w as i64);
                changes.push(a1 as u32);
                changes.push(a2 as u32);
                a0 = a2;
            }
            Mode::Vertical(d) => {
                let a1 = (b1 + i64::from(d)).clamp(0, w as i64);
                if a1 < a0 {
                    return Err(());
                }
                changes.push(a1 as u32);
                a0 = a1;
                black = !black;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_prefix_free() {
        for codes in [&WHITE[..], &BLACK[..]] {
            let mut all = codes.to_vec();
            all.extend_from_slice(&EXTENDED);
            for (i, a) in all.iter().enumerate() {
                for b in all.iter().skip(i + 1) {
                    let (short, long) = if a.1 <= b.1 { (a, b) } else { (b, a) };
                    assert_ne!(long.0 >> (long.1 - short.1), short.0, "{a:?} {b:?}");
                }
            }
        }
    }

    /// Bits from a string of 0s and 1s, padded to whole bytes.
    fn bits(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| *c == '0' || *c == '1').collect();
        let mut out = vec![0u8; s.len().div_ceil(8)];
        for (i, c) in s.chars().enumerate() {
            if c == '1' {
                out[i / 8] |= 0x80 >> (i % 8);
            }
        }
        out
    }

    #[test]
    fn one_dimensional_line() {
        // 8 pixels: 3 white (1000), 2 black (11), 3 white (1000).
        let data = bits("1000 11 1000");
        let p = Params { k: 0, columns: 8, rows: 1, byte_align: false, no_eol: false, lsb_first: false };
        let (px, rows) = decode(&data, &p, 10).unwrap();
        assert_eq!(rows, 1);
        assert_eq!(px, vec![255, 255, 255, 0, 0, 255, 255, 255]);
    }

    #[test]
    fn two_dimensional_lines() {
        // Line 1 (horizontal mode): 2 white (0111), 3 black (10), then
        // V0 for the end. Line 2: V0, V0 (same as line 1), then V0.
        let data = bits("001 0111 10 1   1 1 1");
        let p = Params { k: -1, columns: 8, rows: 2, byte_align: false, no_eol: false, lsb_first: false };
        let (px, rows) = decode(&data, &p, 10).unwrap();
        assert_eq!(rows, 2);
        let line = [255, 255, 0, 0, 0, 255, 255, 255];
        assert_eq!(&px[..8], &line[..]);
        assert_eq!(&px[8..], &line[..]);
    }
}
