//! zlib streams (RFC 1950) around DEFLATE (RFC 1951), plus the checksums
//! PNG needs.
//!
//! Inflate is bounded by the caller's expected output size, which is the
//! defence against "zip bombs": it never produces or allocates more.
//! Deflate uses LZ77 with hash chains and lazy matching, and picks stored,
//! fixed or dynamic Huffman coding per block, whichever is smallest.

// Indexed loops read more naturally than iterator chains in codec code.
#![allow(clippy::needless_range_loop)]

use super::DecodeError;

// ------------------------------------------------------------ checksums

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// Continue a CRC-32 (start with `0xFFFF_FFFF`, finish by inverting).
pub fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = CRC_TABLE[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc
}

/// CRC-32 as used by PNG chunks.
pub fn crc32(data: &[u8]) -> u32 {
    !crc32_update(0xFFFF_FFFF, data)
}

/// Adler-32 as used by zlib streams.
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 is the largest run that cannot overflow u32 before reducing.
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

// ------------------------------------------------------------ tables

const LENGTH_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LENGTH_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
/// Order in which code-length code lengths are stored.
const CLEN_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

fn fixed_litlen_lengths() -> [u8; 288] {
    let mut l = [0u8; 288];
    l[..144].fill(8);
    l[144..256].fill(9);
    l[256..280].fill(7);
    l[280..].fill(8);
    l
}

// ------------------------------------------------------------ inflate

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bits: u64,
    count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, bits: 0, count: 0 }
    }

    fn refill(&mut self) {
        while self.count <= 56 {
            match self.data.get(self.pos) {
                Some(&b) => {
                    self.bits |= u64::from(b) << self.count;
                    self.pos += 1;
                    self.count += 8;
                }
                None => break,
            }
        }
    }

    fn need(&mut self, n: u32) -> Result<(), DecodeError> {
        if self.count < n {
            self.refill();
            if self.count < n {
                return Err(DecodeError::Truncated);
            }
        }
        Ok(())
    }

    fn bits(&mut self, n: u32) -> Result<u32, DecodeError> {
        if n == 0 {
            return Ok(0);
        }
        self.need(n)?;
        let v = (self.bits & ((1u64 << n) - 1)) as u32;
        self.bits >>= n;
        self.count -= n;
        Ok(v)
    }

    /// Drop bits up to the next byte boundary.
    fn align(&mut self) {
        let r = self.count % 8;
        self.bits >>= r;
        self.count -= r;
    }

    /// Position of the next unread byte (after `align`).
    fn byte_pos(&self) -> usize {
        self.pos - (self.count / 8) as usize
    }

    fn reset_to(&mut self, pos: usize) {
        self.pos = pos;
        self.bits = 0;
        self.count = 0;
    }
}

const FAST_BITS: u32 = 10;

/// Canonical Huffman decoder: a lookup table for codes up to FAST_BITS long,
/// and the counts/symbols form for longer ones.
struct Huffman {
    /// (symbol << 4) | length, or 0 for "not in the table".
    fast: Vec<u16>,
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Huffman, DecodeError> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[usize::from(l)] += 1;
        }
        counts[0] = 0;
        // Reject over-subscribed sets. Incomplete ones are allowed (a single
        // distance code is legal); an unused code then fails at decode time.
        let mut left: i32 = 1;
        for len in 1..16 {
            left = (left << 1) - i32::from(counts[len]);
            if left < 0 {
                return Err(DecodeError::Invalid("over-subscribed Huffman code"));
            }
        }
        let mut offsets = [0u16; 16];
        for len in 1..15 {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; usize::from(offsets[15] + counts[15])];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[usize::from(offsets[usize::from(l)])] = sym as u16;
                offsets[usize::from(l)] += 1;
            }
        }
        // Fast table: walk codes in canonical order.
        let mut fast = vec![0u16; 1 << FAST_BITS];
        let mut code = 0u32;
        let mut index = 0usize;
        for len in 1..=FAST_BITS as usize {
            for _ in 0..counts[len] {
                let sym = symbols[index];
                index += 1;
                // Deflate sends codes most-significant bit first, so the
                // table is indexed by the bit-reversed code.
                let rev = reverse_bits(code, len as u32);
                let mut fill = rev as usize;
                while fill < fast.len() {
                    fast[fill] = (sym << 4) | len as u16;
                    fill += 1 << len;
                }
                code += 1;
            }
            code <<= 1;
        }
        Ok(Huffman { fast, counts, symbols })
    }

    fn decode(&self, r: &mut BitReader<'_>) -> Result<u16, DecodeError> {
        if r.count < 15 {
            r.refill();
        }
        let entry = self.fast[(r.bits & ((1 << FAST_BITS) - 1)) as usize];
        if entry != 0 {
            let len = u32::from(entry & 15);
            if len <= r.count {
                r.bits >>= len;
                r.count -= len;
                return Ok(entry >> 4);
            }
            return Err(DecodeError::Truncated);
        }
        // Slow path, bit by bit.
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= r.bits(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - first < count {
                return self
                    .symbols
                    .get((index + code - first) as usize)
                    .copied()
                    .ok_or(DecodeError::Invalid("bad Huffman code"));
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(DecodeError::Invalid("bad Huffman code"))
    }
}

fn reverse_bits(code: u32, len: u32) -> u32 {
    code.reverse_bits() >> (32 - len)
}

/// Inflate a zlib stream whose decompressed size is known exactly. Stops as
/// soon as `expected_len` bytes are produced; never produces more. Fails if
/// the stream ends short, and checks the Adler-32 trailer when it is reached.
pub fn inflate_zlib(data: &[u8], expected_len: usize) -> Result<Vec<u8>, DecodeError> {
    if data.len() < 2 {
        return Err(DecodeError::Truncated);
    }
    let (cmf, flg) = (data[0], data[1]);
    if cmf & 0x0F != 8 || cmf >> 4 > 7 || (u16::from(cmf) << 8 | u16::from(flg)) % 31 != 0 {
        return Err(DecodeError::Invalid("bad zlib header"));
    }
    if flg & 0x20 != 0 {
        return Err(DecodeError::Unsupported("zlib preset dictionary"));
    }
    let mut out = Vec::with_capacity(expected_len);
    let mut r = BitReader::new(&data[2..]);
    let finished = inflate_blocks(&mut r, &mut out, expected_len)?;
    if out.len() < expected_len {
        return Err(DecodeError::Truncated);
    }
    if finished {
        r.align();
        let at = r.byte_pos();
        r.reset_to(at);
        // A missing trailer is tolerated (some encoders omit it); a wrong
        // one means corrupt data.
        if let Ok(hi) = r.bits(16) {
            if let Ok(lo) = r.bits(16) {
                let stored = u32::from_be_bytes([hi as u8, (hi >> 8) as u8, lo as u8, (lo >> 8) as u8]);
                if stored != adler32(&out) {
                    return Err(DecodeError::Invalid("zlib checksum mismatch"));
                }
            }
        }
    }
    Ok(out)
}

/// Raw DEFLATE. Returns true when the final block ended, so the trailer
/// follows; false when output reached `limit` with more data still coming
/// (extra data is ignored, as libpng and zlib-based readers do).
fn inflate_blocks(r: &mut BitReader<'_>, out: &mut Vec<u8>, limit: usize) -> Result<bool, DecodeError> {
    loop {
        let full = out.len() == limit;
        let block = (|| -> Result<(bool, bool), DecodeError> {
            let last = r.bits(1)? == 1;
            let ended = match r.bits(2)? {
                0 => {
                    r.align();
                    let len = r.bits(16)?;
                    let nlen = r.bits(16)?;
                    if len != !nlen & 0xFFFF {
                        return Err(DecodeError::Invalid("bad stored block length"));
                    }
                    let start = r.byte_pos();
                    let end = start.checked_add(len as usize).ok_or(DecodeError::Truncated)?;
                    let bytes = r.data.get(start..end).ok_or(DecodeError::Truncated)?;
                    let take = bytes.len().min(limit - out.len());
                    out.extend_from_slice(&bytes[..take]);
                    r.reset_to(end);
                    take == bytes.len()
                }
                1 => {
                    let lit = Huffman::new(&fixed_litlen_lengths())?;
                    let dist = Huffman::new(&[5u8; 30])?;
                    inflate_codes(r, out, limit, &lit, &dist)?
                }
                2 => {
                    let (lit, dist) = read_dynamic_tables(r)?;
                    inflate_codes(r, out, limit, &lit, &dist)?
                }
                _ => return Err(DecodeError::Invalid("bad deflate block type")),
            };
            Ok((last, ended))
        })();
        match block {
            Ok((last, true)) if last => return Ok(true),
            Ok((_, true)) => {}
            Ok((_, false)) => return Ok(false),
            // With all the output we need, whatever follows does not matter.
            Err(_) if full => return Ok(false),
            Err(e) => return Err(e),
        }
    }
}

fn read_dynamic_tables(r: &mut BitReader<'_>) -> Result<(Huffman, Huffman), DecodeError> {
    let hlit = r.bits(5)? as usize + 257;
    let hdist = r.bits(5)? as usize + 1;
    let hclen = r.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(DecodeError::Invalid("bad dynamic block header"));
    }
    let mut clen = [0u8; 19];
    for &i in &CLEN_ORDER[..hclen] {
        clen[i] = r.bits(3)? as u8;
    }
    let clen_code = Huffman::new(&clen)?;
    let mut lengths = vec![0u8; hlit + hdist];
    let mut i = 0;
    while i < lengths.len() {
        let sym = clen_code.decode(r)?;
        let (value, repeat) = match sym {
            0..=15 => (sym as u8, 1),
            16 => {
                let prev = *lengths.get(i.wrapping_sub(1)).ok_or(DecodeError::Invalid("repeat with no length"))?;
                (prev, 3 + r.bits(2)? as usize)
            }
            17 => (0, 3 + r.bits(3)? as usize),
            18 => (0, 11 + r.bits(7)? as usize),
            _ => return Err(DecodeError::Invalid("bad code length")),
        };
        if i + repeat > lengths.len() {
            return Err(DecodeError::Invalid("code lengths overflow"));
        }
        lengths[i..i + repeat].fill(value);
        i += repeat;
    }
    if lengths[256] == 0 {
        return Err(DecodeError::Invalid("no end-of-block code"));
    }
    Ok((Huffman::new(&lengths[..hlit])?, Huffman::new(&lengths[hlit..])?))
}

/// Decode one Huffman-coded block. Returns true at the end-of-block code;
/// false if output reached `limit` and the block has more data.
fn inflate_codes(
    r: &mut BitReader<'_>,
    out: &mut Vec<u8>,
    limit: usize,
    lit: &Huffman,
    dist: &Huffman,
) -> Result<bool, DecodeError> {
    loop {
        let sym = match lit.decode(r) {
            Ok(s) => s,
            Err(_) if out.len() == limit => return Ok(false),
            Err(e) => return Err(e),
        };
        if sym == 256 {
            return Ok(true);
        }
        if out.len() == limit {
            return Ok(false);
        }
        match sym {
            0..=255 => out.push(sym as u8),
            257..=285 => {
                let i = usize::from(sym - 257);
                let len = usize::from(LENGTH_BASE[i]) + r.bits(u32::from(LENGTH_EXTRA[i]))? as usize;
                let d = usize::from(dist.decode(r)?);
                if d >= 30 {
                    return Err(DecodeError::Invalid("bad distance code"));
                }
                let distance = usize::from(DIST_BASE[d]) + r.bits(u32::from(DIST_EXTRA[d]))? as usize;
                if distance > out.len() {
                    return Err(DecodeError::Invalid("distance too far back"));
                }
                let take = len.min(limit - out.len());
                let start = out.len() - distance;
                for k in 0..take {
                    let b = out[start + k];
                    out.push(b);
                }
            }
            _ => return Err(DecodeError::Invalid("bad literal/length code")),
        }
    }
}

// ------------------------------------------------------------ deflate

struct BitWriter {
    out: Vec<u8>,
    bits: u64,
    count: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, n: u32) {
        self.bits |= u64::from(value) << self.count;
        self.count += n;
        while self.count >= 8 {
            self.out.push(self.bits as u8);
            self.bits >>= 8;
            self.count -= 8;
        }
    }

    fn align(&mut self) {
        if self.count > 0 {
            self.out.push(self.bits as u8);
            self.bits = 0;
            self.count = 0;
        }
    }
}

const WINDOW: usize = 32 * 1024;
const HASH_BITS: u32 = 15;
const MAX_MATCH: usize = 258;
const MIN_MATCH: usize = 3;
// Roughly zlib level 6.
const MAX_CHAIN: usize = 128;
const NICE_LENGTH: usize = 128;
const GOOD_LENGTH: usize = 8;
const LAZY_LENGTH: usize = 16;
/// Symbols buffered per block before choosing its coding.
const BLOCK_SYMBOLS: usize = 64 * 1024;

#[derive(Clone, Copy)]
enum Token {
    Literal(u8),
    Match { len: u16, dist: u16 },
}

fn length_code(len: usize) -> usize {
    // Largest base not exceeding len.
    LENGTH_BASE.iter().rposition(|&b| usize::from(b) <= len).unwrap_or(0)
}

fn dist_code(dist: usize) -> usize {
    DIST_BASE.iter().rposition(|&b| usize::from(b) <= dist).unwrap_or(0)
}

/// Compress `data` to a zlib stream.
pub fn deflate_zlib(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter { out: vec![0x78, 0x9C], bits: 0, count: 0 };
    let mut tokens: Vec<Token> = Vec::with_capacity(BLOCK_SYMBOLS);
    let mut block_start = 0usize;

    let mut head = vec![u32::MAX; 1 << HASH_BITS];
    let mut prev = vec![u32::MAX; WINDOW];
    let hash = |i: usize| -> usize {
        let v = (u32::from(data[i]) << 16) | (u32::from(data[i + 1]) << 8) | u32::from(data[i + 2]);
        (v.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize
    };
    let insert = |i: usize, head: &mut Vec<u32>, prev: &mut Vec<u32>| {
        if i + MIN_MATCH <= data.len() {
            let h = hash(i);
            prev[i % WINDOW] = head[h];
            head[h] = i as u32;
        }
    };
    let longest = |i: usize, head: &Vec<u32>, prev: &Vec<u32>, prev_len: usize| -> (usize, usize) {
        if i + MIN_MATCH > data.len() {
            return (0, 0);
        }
        let max = MAX_MATCH.min(data.len() - i);
        let mut chain = if prev_len >= GOOD_LENGTH { MAX_CHAIN / 4 } else { MAX_CHAIN };
        let mut best = (0usize, 0usize);
        let mut cand = head[hash(i)];
        while cand != u32::MAX && chain > 0 {
            let c = cand as usize;
            if c >= i || i - c > WINDOW - MAX_MATCH {
                break;
            }
            if data[c + best.0.min(max - 1)] == data[i + best.0.min(max - 1)] {
                let mut l = 0;
                while l < max && data[c + l] == data[i + l] {
                    l += 1;
                }
                if l > best.0 {
                    best = (l, i - c);
                    if l >= NICE_LENGTH || l == max {
                        break;
                    }
                }
            }
            let next = prev[c % WINDOW];
            if next == u32::MAX || next as usize >= c {
                break;
            }
            cand = next;
            chain -= 1;
        }
        if best.0 >= MIN_MATCH { best } else { (0, 0) }
    };

    let mut i = 0usize;
    // Lazy matching: a match found at i is only taken if the one at i+1 is
    // not longer.
    let mut pending: Option<(usize, usize)> = None;
    while i < data.len() {
        let current = match pending.take() {
            Some(m) => m,
            None => longest(i, &head, &prev, 0),
        };
        insert(i, &mut head, &mut prev);
        if current.0 >= MIN_MATCH && current.0 < LAZY_LENGTH && i + 1 < data.len() {
            let next = longest(i + 1, &head, &prev, current.0);
            if next.0 > current.0 {
                tokens.push(Token::Literal(data[i]));
                i += 1;
                pending = Some(next);
                continue;
            }
        }
        if current.0 >= MIN_MATCH {
            tokens.push(Token::Match { len: current.0 as u16, dist: current.1 as u16 });
            for k in 1..current.0 {
                insert(i + k, &mut head, &mut prev);
            }
            i += current.0;
        } else {
            tokens.push(Token::Literal(data[i]));
            i += 1;
        }
        if tokens.len() >= BLOCK_SYMBOLS {
            write_block(&mut w, &tokens, &data[block_start..i], false);
            tokens.clear();
            block_start = i;
        }
    }
    write_block(&mut w, &tokens, &data[block_start..], true);
    w.align();
    w.out.extend_from_slice(&adler32(data).to_be_bytes());
    w.out
}

/// Code lengths limited to `max_bits` for the given symbol frequencies
/// (Moffat-Katajainen, then the Kraft-sum fix-up used by miniz).
fn huffman_lengths(freqs: &[u32], max_bits: usize) -> Vec<u8> {
    let mut used: Vec<(u32, usize)> = freqs.iter().enumerate().filter(|(_, f)| **f > 0).map(|(s, f)| (*f, s)).collect();
    let mut lengths = vec![0u8; freqs.len()];
    // Always at least two codes, so every decoder accepts the table.
    for s in 0..freqs.len() {
        if used.len() >= 2 {
            break;
        }
        if freqs[s] == 0 {
            used.push((1, s));
        }
    }
    used.sort_unstable();
    let n = used.len();
    let mut a: Vec<u32> = used.iter().map(|&(f, _)| f).collect();
    if n == 1 {
        a[0] = 1;
    } else {
        a[0] += a[1];
        let (mut root, mut leaf) = (0usize, 2usize);
        for next in 1..n - 1 {
            if leaf >= n || a[root] < a[leaf] {
                a[next] = a[root];
                a[root] = next as u32;
                root += 1;
            } else {
                a[next] = a[leaf];
                leaf += 1;
            }
            if leaf >= n || (root < next && a[root] < a[leaf]) {
                a[next] += a[root];
                a[root] = next as u32;
                root += 1;
            } else {
                a[next] += a[leaf];
                leaf += 1;
            }
        }
        a[n - 2] = 0;
        for next in (0..n.saturating_sub(2)).rev() {
            a[next] = a[a[next] as usize] + 1;
        }
        let (mut avail, mut used_count, mut depth) = (1i64, 0i64, 0u32);
        let mut root = n as i64 - 2;
        let mut next = n as i64 - 1;
        while avail > 0 {
            while root >= 0 && a[root as usize] == depth {
                used_count += 1;
                root -= 1;
            }
            while avail > used_count {
                a[next as usize] = depth;
                next -= 1;
                avail -= 1;
            }
            avail = 2 * used_count;
            depth += 1;
            used_count = 0;
        }
    }
    // Count codes per length, capping at max_bits.
    let mut num = [0u32; 33];
    for &l in &a {
        num[(l as usize).min(32)] += 1;
    }
    for l in max_bits + 1..num.len() {
        num[max_bits] += num[l];
        num[l] = 0;
    }
    let mut total: u64 = (1..=max_bits).map(|l| u64::from(num[l]) << (max_bits - l)).sum();
    while total != 1u64 << max_bits {
        num[max_bits] -= 1;
        for l in (1..max_bits).rev() {
            if num[l] > 0 {
                num[l] -= 1;
                num[l + 1] += 2;
                break;
            }
        }
        total -= 1;
    }
    // Most frequent symbols get the shortest codes.
    let mut j = n;
    for l in 1..=max_bits {
        for _ in 0..num[l] {
            j -= 1;
            lengths[used[j].1] = l as u8;
        }
    }
    lengths
}

/// Canonical codes, bit-reversed for LSB-first output.
fn canonical_codes(lengths: &[u8]) -> Vec<u32> {
    let mut count = [0u32; 16];
    for &l in lengths {
        count[usize::from(l)] += 1;
    }
    count[0] = 0;
    let mut next = [0u32; 16];
    let mut code = 0;
    for bits in 1..16 {
        code = (code + count[bits - 1]) << 1;
        next[bits] = code;
    }
    lengths
        .iter()
        .map(|&l| {
            if l == 0 {
                0
            } else {
                let c = next[usize::from(l)];
                next[usize::from(l)] += 1;
                reverse_bits(c, u32::from(l))
            }
        })
        .collect()
}

/// Run-length encode code lengths: (symbol, extra bits value, extra bit count).
fn rle_lengths(lengths: &[u8]) -> Vec<(u8, u32, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lengths.len() {
        let l = lengths[i];
        let mut run = 1;
        while i + run < lengths.len() && lengths[i + run] == l {
            run += 1;
        }
        let mut left = run;
        if l == 0 {
            while left >= 11 {
                let n = left.min(138);
                out.push((18, (n - 11) as u32, 7));
                left -= n;
            }
            if left >= 3 {
                out.push((17, (left - 3) as u32, 3));
                left = 0;
            }
        } else {
            out.push((l, 0, 0));
            left -= 1;
            while left >= 3 {
                let n = left.min(6);
                out.push((16, (n - 3) as u32, 2));
                left -= n;
            }
        }
        for _ in 0..left {
            out.push((l, 0, 0));
        }
        i += run;
    }
    out
}

fn write_block(w: &mut BitWriter, tokens: &[Token], raw: &[u8], last: bool) {
    let mut lit_freq = vec![0u32; 286];
    let mut dist_freq = vec![0u32; 30];
    for t in tokens {
        match *t {
            Token::Literal(b) => lit_freq[usize::from(b)] += 1,
            Token::Match { len, dist } => {
                lit_freq[257 + length_code(usize::from(len))] += 1;
                dist_freq[dist_code(usize::from(dist))] += 1;
            }
        }
    }
    lit_freq[256] = 1;

    let lit_len = huffman_lengths(&lit_freq, 15);
    let dist_len = huffman_lengths(&dist_freq, 15);
    let hlit = (257..=286).rev().find(|&n| lit_len[n - 1] != 0).unwrap_or(257);
    let hdist = (1..=30).rev().find(|&n| dist_len[n - 1] != 0).unwrap_or(1);
    let mut all = lit_len[..hlit].to_vec();
    all.extend_from_slice(&dist_len[..hdist]);
    let rle = rle_lengths(&all);
    let mut clen_freq = vec![0u32; 19];
    for &(s, _, _) in &rle {
        clen_freq[usize::from(s)] += 1;
    }
    let clen_len = huffman_lengths(&clen_freq, 7);
    let hclen = (4..=19).rev().find(|&n| clen_len[CLEN_ORDER[n - 1]] != 0).unwrap_or(4);

    let payload_bits = |ll: &[u8], dl: &[u8]| -> u64 {
        let mut bits = u64::from(ll[256]);
        for t in tokens {
            bits += match *t {
                Token::Literal(b) => u64::from(ll[usize::from(b)]),
                Token::Match { len, dist } => {
                    let lc = length_code(usize::from(len));
                    let dc = dist_code(usize::from(dist));
                    u64::from(ll[257 + lc])
                        + u64::from(LENGTH_EXTRA[lc])
                        + u64::from(dl[dc])
                        + u64::from(DIST_EXTRA[dc])
                }
            };
        }
        bits
    };
    let dynamic_bits = 3
        + 14
        + 3 * hclen as u64
        + rle.iter().map(|&(s, _, n)| u64::from(clen_len[usize::from(s)]) + u64::from(n)).sum::<u64>()
        + payload_bits(&lit_len, &dist_len);
    let fixed_lit = fixed_litlen_lengths();
    let fixed_bits = 3 + payload_bits(&fixed_lit, &[5u8; 30]);
    let stored_bits = (raw.len() as u64).div_ceil(65_535).max(1) * 40 + raw.len() as u64 * 8 + 7;

    if stored_bits <= dynamic_bits.min(fixed_bits) {
        let chunks: Vec<&[u8]> = if raw.is_empty() { vec![&[][..]] } else { raw.chunks(65_535).collect() };
        let count = chunks.len();
        for (k, chunk) in chunks.into_iter().enumerate() {
            w.put(u32::from(last && k + 1 == count), 1);
            w.put(0, 2);
            w.align();
            let len = chunk.len() as u16;
            w.out.extend_from_slice(&len.to_le_bytes());
            w.out.extend_from_slice(&(!len).to_le_bytes());
            w.out.extend_from_slice(chunk);
        }
        return;
    }

    let (ll, dl): (Vec<u8>, Vec<u8>) = if dynamic_bits < fixed_bits {
        w.put(u32::from(last), 1);
        w.put(2, 2);
        w.put((hlit - 257) as u32, 5);
        w.put((hdist - 1) as u32, 5);
        w.put((hclen - 4) as u32, 4);
        for &i in &CLEN_ORDER[..hclen] {
            w.put(u32::from(clen_len[i]), 3);
        }
        let clen_codes = canonical_codes(&clen_len);
        for &(s, extra, n) in &rle {
            w.put(clen_codes[usize::from(s)], u32::from(clen_len[usize::from(s)]));
            if n > 0 {
                w.put(extra, n);
            }
        }
        (lit_len, dist_len)
    } else {
        w.put(u32::from(last), 1);
        w.put(1, 2);
        (fixed_lit.to_vec(), vec![5u8; 30])
    };
    let lc = canonical_codes(&ll);
    let dc = canonical_codes(&dl);
    for t in tokens {
        match *t {
            Token::Literal(b) => w.put(lc[usize::from(b)], u32::from(ll[usize::from(b)])),
            Token::Match { len, dist } => {
                let (len, dist) = (usize::from(len), usize::from(dist));
                let l = length_code(len);
                w.put(lc[257 + l], u32::from(ll[257 + l]));
                if LENGTH_EXTRA[l] > 0 {
                    w.put((len - usize::from(LENGTH_BASE[l])) as u32, u32::from(LENGTH_EXTRA[l]));
                }
                let d = dist_code(dist);
                w.put(dc[d], u32::from(dl[d]));
                if DIST_EXTRA[d] > 0 {
                    w.put((dist - usize::from(DIST_BASE[d])) as u32, u32::from(DIST_EXTRA[d]));
                }
            }
        }
    }
    w.put(lc[256], u32::from(ll[256]));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(data: &[u8]) {
        let z = deflate_zlib(data);
        assert_eq!(inflate_zlib(&z, data.len()).unwrap(), data, "len {}", data.len());
    }

    fn pseudo_random(n: usize, seed: u32) -> Vec<u8> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    #[test]
    fn checksums() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(&vec![0xFF; 100_000]), {
            let (mut a, mut b) = (1u64, 0u64);
            for _ in 0..100_000 {
                a = (a + 255) % 65521;
                b = (b + a) % 65521;
            }
            ((b << 16) | a) as u32
        });
    }

    #[test]
    fn round_trips() {
        roundtrip(b"");
        roundtrip(b"a");
        roundtrip(b"abcabcabcabcabcabcabcabcabc");
        roundtrip(&vec![0u8; 100_000]);
        roundtrip(&pseudo_random(70_000, 7));
        let mut mixed = Vec::new();
        for i in 0..2_000_000usize {
            mixed.push(if (i / 4500) % 3 == 0 { (i % 7) as u8 } else { ((i * 31) >> 3) as u8 });
        }
        roundtrip(&mixed);
    }

    #[test]
    fn known_streams() {
        // From Python: zlib.compress(b"hello hello hello", 9) and zlib.compress(b"hello", 0).
        let z9 = [120, 218, 203, 72, 205, 201, 201, 87, 200, 64, 144, 0, 58, 46, 6, 125];
        assert_eq!(inflate_zlib(&z9, 17).unwrap(), b"hello hello hello");
        let mut wrong = z9;
        wrong[15] ^= 1;
        assert_eq!(inflate_zlib(&wrong, 17), Err(DecodeError::Invalid("zlib checksum mismatch")));
        let z0 = [0x78, 0x01, 0x01, 0x05, 0x00, 0xFA, 0xFF, b'h', b'e', b'l', b'l', b'o', 0x06, 0x2C, 0x02, 0x15];
        assert_eq!(inflate_zlib(&z0, 5).unwrap(), b"hello");
    }

    #[test]
    fn bounded_and_robust() {
        let z = deflate_zlib(&vec![0u8; 1_000_000]);
        // Asking for less never produces more.
        assert_eq!(inflate_zlib(&z, 10).unwrap().len(), 10);
        assert_eq!(inflate_zlib(&z, 2_000_000), Err(DecodeError::Truncated));
        let mut bad = z.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        assert!(inflate_zlib(&bad, 1_000_000).is_err());
        let sample = deflate_zlib(&pseudo_random(5000, 3));
        for cut in 0..sample.len() {
            let _ = inflate_zlib(&sample[..cut], 5000);
        }
        let mut x = 1u32;
        for _ in 0..3000 {
            let mut m = sample.clone();
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let pos = (x >> 8) as usize % m.len();
            m[pos] ^= (x >> 24) as u8 | 1;
            let _ = inflate_zlib(&m, 5000);
        }
    }
}
