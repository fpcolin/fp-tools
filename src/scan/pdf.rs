//! Reading the scanned images out of a PDF.
//!
//! Scanner PDFs hold one picture per page (sometimes several strips, or a
//! picture plus a black-and-white text layer). This finds the pictures each
//! page draws and paints them onto a greyscale page, as a PDF viewer would,
//! without any of the PDF's text or drawing. It reads PDFs with or without
//! cross-reference tables, with object streams and incremental updates.

use std::collections::HashMap;

use super::ccitt;
use super::gray::{Gray, luma};
use crate::image::zlib;

#[derive(Clone, Debug, PartialEq)]
pub enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(Vec<u8>),
    Str(Vec<u8>),
    Array(Vec<Obj>),
    Dict(Dict),
    Ref(u32, u16),
    /// A stream: its dictionary and where its data is in the file.
    Stream(Dict, usize, usize),
    /// Only seen while parsing content streams.
    Op(Vec<u8>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dict(Vec<(Vec<u8>, Obj)>);

impl Dict {
    pub fn get(&self, key: &str) -> Option<&Obj> {
        self.0.iter().find(|(k, _)| k == key.as_bytes()).map(|(_, v)| v)
    }
}

impl Obj {
    fn as_f64(&self) -> Option<f64> {
        match self {
            Obj::Int(i) => Some(*i as f64),
            Obj::Real(r) => Some(*r),
            _ => None,
        }
    }
    fn as_name(&self) -> Option<&[u8]> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }
}

fn is_white(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0')
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// A tokenizer and object parser over PDF syntax.
struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Parser { data, pos, depth: 0 }
    }

    fn skip_space(&mut self) {
        while let Some(&b) = self.data.get(self.pos) {
            if is_white(b) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(&c) = self.data.get(self.pos) {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    /// A bare word (keyword or number) at the current position.
    fn word(&mut self) -> &'a [u8] {
        let start = self.pos;
        while let Some(&b) = self.data.get(self.pos) {
            if is_white(b) || is_delim(b) {
                break;
            }
            self.pos += 1;
        }
        &self.data[start..self.pos]
    }

    fn object(&mut self) -> Option<Obj> {
        self.skip_space();
        let b = *self.data.get(self.pos)?;
        self.depth += 1;
        if self.depth > 64 {
            return None;
        }
        let result = match b {
            b'/' => {
                self.pos += 1;
                let raw = self.word();
                let mut name = Vec::with_capacity(raw.len());
                let mut i = 0;
                while i < raw.len() {
                    if raw[i] == b'#'
                        && i + 2 < raw.len()
                        && let Some(v) = raw
                            .get(i + 1..i + 3)
                            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok())
                    {
                        name.push(v);
                        i += 3;
                        continue;
                    }
                    name.push(raw[i]);
                    i += 1;
                }
                Some(Obj::Name(name))
            }
            b'(' => self.literal_string().map(Obj::Str),
            b'<' if self.data.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                let mut entries = Vec::new();
                loop {
                    self.skip_space();
                    match self.data.get(self.pos) {
                        Some(b'>') => {
                            self.pos += 2;
                            break;
                        }
                        None => return None,
                        _ => {}
                    }
                    let key = match self.object()? {
                        Obj::Name(n) => n,
                        _ => return None,
                    };
                    let value = self.object()?;
                    entries.push((key, value));
                }
                Some(Obj::Dict(Dict(entries)))
            }
            b'<' => {
                self.pos += 1;
                let mut hex = Vec::new();
                while let Some(&c) = self.data.get(self.pos) {
                    self.pos += 1;
                    if c == b'>' {
                        break;
                    }
                    if c.is_ascii_hexdigit() {
                        hex.push(c);
                    }
                }
                if hex.len() % 2 == 1 {
                    hex.push(b'0');
                }
                Some(Obj::Str(
                    hex.chunks(2)
                        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap_or("0"), 16).unwrap_or(0))
                        .collect(),
                ))
            }
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space();
                    match self.data.get(self.pos) {
                        Some(b']') => {
                            self.pos += 1;
                            break;
                        }
                        None => return None,
                        _ => {}
                    }
                    items.push(self.object()?);
                }
                Some(Obj::Array(items))
            }
            b']' | b'>' | b')' | b'{' | b'}' => {
                self.pos += 1;
                Some(Obj::Op(vec![b]))
            }
            _ => {
                let w = self.word();
                if w.is_empty() {
                    self.pos += 1;
                    return Some(Obj::Op(vec![b]));
                }
                Some(self.number_or_ref(w))
            }
        };
        self.depth -= 1;
        result
    }

    fn number_or_ref(&mut self, w: &[u8]) -> Obj {
        let text = std::str::from_utf8(w).unwrap_or("");
        if let Ok(i) = text.parse::<i64>() {
            // "12 0 R"?
            let save = self.pos;
            self.skip_space();
            let g = self.word();
            if let Ok(generation) = std::str::from_utf8(g).unwrap_or("").parse::<u16>() {
                self.skip_space();
                if self.data.get(self.pos) == Some(&b'R')
                    && self.data.get(self.pos + 1).is_none_or(|&c| is_white(c) || is_delim(c))
                    && (0..=u32::MAX as i64).contains(&i)
                {
                    self.pos += 1;
                    return Obj::Ref(i as u32, generation);
                }
            }
            self.pos = save;
            return Obj::Int(i);
        }
        if let Ok(r) = text.parse::<f64>() {
            return Obj::Real(r);
        }
        match w {
            b"true" => Obj::Bool(true),
            b"false" => Obj::Bool(false),
            b"null" => Obj::Null,
            _ => Obj::Op(w.to_vec()),
        }
    }

    fn literal_string(&mut self) -> Option<Vec<u8>> {
        self.pos += 1;
        let mut out = Vec::new();
        let mut nest = 1;
        loop {
            let c = *self.data.get(self.pos)?;
            self.pos += 1;
            match c {
                b'\\' => {
                    let e = *self.data.get(self.pos)?;
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.data.get(self.pos) {
                                    Some(&d @ b'0'..=b'7') => {
                                        v = v * 8 + u32::from(d - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v as u8);
                        }
                        b'\r' => {
                            if self.data.get(self.pos) == Some(&b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                b'(' => {
                    nest += 1;
                    out.push(c);
                }
                b')' => {
                    nest -= 1;
                    if nest == 0 {
                        return Some(out);
                    }
                    out.push(c);
                }
                _ => out.push(c),
            }
        }
    }
}

/// A PDF file opened for reading its pages' images.
pub struct Pdf {
    data: Vec<u8>,
    /// Object number -> the object (streams point into `data`).
    objects: HashMap<u32, Obj>,
    /// Decoded object streams' contents are parsed into `objects` directly.
    pages: Vec<Page>,
}

#[derive(Clone)]
struct Page {
    dict: Dict,
    resources: Option<Obj>,
    media_box: [f64; 4],
    rotate: i64,
}

const MAX_STREAM: usize = 256 << 20;

impl Pdf {
    pub fn open(data: Vec<u8>) -> Result<Pdf, String> {
        if !data.starts_with(b"%PDF") && !data[..data.len().min(1024)].windows(4).any(|w| w == b"%PDF") {
            return Err("not a PDF file".into());
        }
        let mut pdf = Pdf { data, objects: HashMap::new(), pages: Vec::new() };
        let mut trailers: Vec<Dict> = Vec::new();
        pdf.scan_objects(&mut trailers);
        pdf.load_object_streams();
        // The catalog, from the last trailer (or cross-reference stream).
        let root = trailers
            .iter()
            .rev()
            .find_map(|t| t.get("Root").cloned())
            .or_else(|| {
                pdf.objects.iter().find_map(|(&n, o)| match o {
                    Obj::Dict(d) if d.get("Type").and_then(Obj::as_name) == Some(b"Catalog") => Some(Obj::Ref(n, 0)),
                    _ => None,
                })
            })
            .ok_or("The PDF file is damaged (no catalogue).")?;
        if trailers.iter().any(|t| t.get("Encrypt").is_some()) {
            return Err("This PDF is password-protected or encrypted. Save it without protection, or scan to TIFF or JPEG instead.".into());
        }
        let root = pdf.resolve(&root);
        let pages_root = match &root {
            Obj::Dict(d) => d.get("Pages").cloned(),
            _ => None,
        }
        .ok_or("The PDF file is damaged (no pages).")?;
        let mut pages = Vec::new();
        pdf.collect_pages(&pages_root, None, [0.0, 0.0, 612.0, 792.0], 0, 0, &mut pages, &mut Vec::new());
        if pages.is_empty() {
            return Err("The PDF file has no pages.".into());
        }
        pdf.pages = pages;
        Ok(pdf)
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Find every "n g obj" in the file, skipping over stream data, and
    /// every trailer dictionary.
    fn scan_objects(&mut self, trailers: &mut Vec<Dict>) {
        let data = &self.data;
        let mut objects: HashMap<u32, Obj> = HashMap::new();
        let mut pos = 0;
        let find = |from: usize, pat: &[u8]| {
            data.get(from..).and_then(|d| d.windows(pat.len()).position(|w| w == pat)).map(|p| p + from)
        };
        while pos < data.len() {
            let next_obj = find(pos, b"obj");
            let next_trailer = find(pos, b"trailer");
            let Some(at) = [next_obj, next_trailer].into_iter().flatten().min() else { break };
            if Some(at) == next_trailer {
                let mut p = Parser::new(data, at + 7);
                if let Some(Obj::Dict(d)) = p.object() {
                    trailers.push(d);
                    pos = p.pos;
                } else {
                    pos = at + 7;
                }
                continue;
            }
            pos = at + 3;
            // "<num> <gen> obj", at the start of a line or after whitespace.
            let mut s = at;
            let back = |s: &mut usize, digits: bool| -> bool {
                let end = *s;
                while *s > 0 && (if digits { data[*s - 1].is_ascii_digit() } else { is_white(data[*s - 1]) }) {
                    *s -= 1;
                }
                *s < end
            };
            if !back(&mut s, false) || !back(&mut s, true) {
                continue;
            }
            let gen_start = s;
            if !back(&mut s, false) || !back(&mut s, true) {
                continue;
            }
            if s > 0 && !is_white(data[s - 1]) && !is_delim(data[s - 1]) {
                continue;
            }
            let num_text = std::str::from_utf8(&data[s..gen_start]).unwrap_or("").trim();
            let Ok(num) = num_text.parse::<u32>() else { continue };
            let mut p = Parser::new(data, at + 3);
            let Some(obj) = p.object() else { continue };
            // A stream follows its dictionary.
            let after = p.pos;
            let mut q = Parser::new(data, after);
            q.skip_space();
            let obj = match obj {
                Obj::Dict(d) if data[q.pos..].starts_with(b"stream") => {
                    let mut start = q.pos + 6;
                    if data.get(start) == Some(&b'\r') {
                        start += 1;
                    }
                    if data.get(start) == Some(&b'\n') {
                        start += 1;
                    }
                    let by_length = match d.get("Length") {
                        Some(Obj::Int(n)) if *n >= 0 => {
                            let end = start.saturating_add(*n as usize);
                            let tail = data.get(end..end + 20).unwrap_or(&[]);
                            let mut t = 0;
                            while t < tail.len() && is_white(tail[t]) {
                                t += 1;
                            }
                            tail[t..].starts_with(b"endstream").then_some(end)
                        }
                        _ => None,
                    };
                    let end = by_length.or_else(|| {
                        find(start, b"endstream").map(|e| {
                            let mut e = e;
                            // Drop the end-of-line before "endstream".
                            if e > start && data[e - 1] == b'\n' {
                                e -= 1;
                            }
                            if e > start && data[e - 1] == b'\r' {
                                e -= 1;
                            }
                            e
                        })
                    });
                    let Some(end) = end else { continue };
                    pos = end;
                    if d.get("Type").and_then(Obj::as_name) == Some(b"XRef") {
                        trailers.push(d.clone());
                    }
                    Obj::Stream(d, start, end)
                }
                other => {
                    pos = after;
                    other
                }
            };
            objects.insert(num, obj);
        }
        self.objects = objects;
    }

    /// Objects stored inside compressed object streams (PDF 1.5).
    fn load_object_streams(&mut self) {
        let streams: Vec<(Dict, usize, usize)> = self
            .objects
            .values()
            .filter_map(|o| match o {
                Obj::Stream(d, s, e) if d.get("Type").and_then(Obj::as_name) == Some(b"ObjStm") => {
                    Some((d.clone(), *s, *e))
                }
                _ => None,
            })
            .collect();
        for (d, s, e) in streams {
            let Ok(body) = self.decode_stream(&d, s, e, None) else { continue };
            let n = d.get("N").and_then(Obj::as_f64).unwrap_or(0.0) as usize;
            let first = d.get("First").and_then(Obj::as_f64).unwrap_or(0.0) as usize;
            let mut header = Parser::new(&body, 0);
            let mut entries = Vec::new();
            for _ in 0..n.min(100_000) {
                match (header.object(), header.object()) {
                    (Some(Obj::Int(num)), Some(Obj::Int(off))) if num >= 0 && off >= 0 => {
                        entries.push((num as u32, off as usize))
                    }
                    _ => break,
                }
            }
            for (num, off) in entries {
                // A regular object of the same number (a later edit) wins.
                if self.objects.contains_key(&num) {
                    continue;
                }
                let mut p = Parser::new(&body, first + off);
                if let Some(obj) = p.object() {
                    self.objects.insert(num, obj);
                }
            }
        }
    }

    fn resolve(&self, o: &Obj) -> Obj {
        let mut o = o.clone();
        for _ in 0..16 {
            match o {
                Obj::Ref(n, _) => o = self.objects.get(&n).cloned().unwrap_or(Obj::Null),
                other => return other,
            }
        }
        Obj::Null
    }

    fn dict_of(&self, o: &Obj) -> Option<Dict> {
        match self.resolve(o) {
            Obj::Dict(d) | Obj::Stream(d, _, _) => Some(d),
            _ => None,
        }
    }

    fn number(&self, o: Option<&Obj>) -> Option<f64> {
        o.map(|o| self.resolve(o)).and_then(|o| o.as_f64())
    }

    fn rect(&self, o: Option<&Obj>) -> Option<[f64; 4]> {
        match o.map(|o| self.resolve(o))? {
            Obj::Array(a) if a.len() == 4 => {
                let v: Vec<f64> = a.iter().filter_map(|x| self.number(Some(x))).collect();
                (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
            }
            _ => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_pages(
        &self,
        node: &Obj,
        resources: Option<Obj>,
        media_box: [f64; 4],
        rotate: i64,
        depth: usize,
        out: &mut Vec<Page>,
        seen: &mut Vec<u32>,
    ) {
        if depth > 32 || out.len() > 5000 {
            return;
        }
        if let Obj::Ref(n, _) = node {
            if seen.contains(n) {
                return;
            }
            seen.push(*n);
        }
        let Some(d) = self.dict_of(node) else { return };
        let resources = d.get("Resources").cloned().or(resources);
        let media_box = self.rect(d.get("CropBox")).or_else(|| self.rect(d.get("MediaBox"))).unwrap_or(media_box);
        let rotate = self.number(d.get("Rotate")).map_or(rotate, |r| r as i64);
        match d.get("Kids").map(|k| self.resolve(k)) {
            Some(Obj::Array(kids)) => {
                for kid in &kids {
                    self.collect_pages(kid, resources.clone(), media_box, rotate, depth + 1, out, seen);
                }
            }
            _ => out.push(Page { dict: d, resources, media_box, rotate }),
        }
    }

    /// A stream's data with its filters applied, except a final image codec
    /// (DCT, CCITT, JBIG2, JPX), whose name and parameters are returned in
    /// `codec` instead when asked for.
    fn decode_stream(
        &self,
        d: &Dict,
        start: usize,
        end: usize,
        codec: Option<&mut Option<(Vec<u8>, Dict)>>,
    ) -> Result<Vec<u8>, String> {
        let raw = self.data.get(start..end).ok_or("damaged stream")?;
        let filters: Vec<Vec<u8>> = match d.get("Filter").map(|f| self.resolve(f)) {
            Some(Obj::Name(n)) => vec![n],
            Some(Obj::Array(a)) => a.iter().filter_map(|f| self.resolve(f).as_name().map(<[u8]>::to_vec)).collect(),
            _ => Vec::new(),
        };
        let params: Vec<Dict> = match d.get("DecodeParms").or_else(|| d.get("DP")).map(|p| self.resolve(p)) {
            Some(Obj::Dict(p)) => vec![p],
            Some(Obj::Array(a)) => a.iter().map(|p| self.dict_of(p).unwrap_or_default()).collect(),
            _ => Vec::new(),
        };
        let mut data = raw.to_vec();
        let mut codec = codec;
        for (i, f) in filters.iter().enumerate() {
            let p = params.get(i).cloned().unwrap_or_default();
            let name = std::str::from_utf8(f).unwrap_or("");
            data = match name {
                "FlateDecode" | "Fl" => {
                    let inflated = zlib::inflate_zlib_bounded(&data, MAX_STREAM).map_err(|e| e.to_string())?;
                    self.predict(inflated, &p)?
                }
                "LZWDecode" | "LZW" => {
                    let early = self.number(p.get("EarlyChange")).unwrap_or(1.0) != 0.0;
                    let out = super::tiff::lzw(&data, MAX_STREAM, early)?;
                    self.predict(out, &p)?
                }
                "ASCIIHexDecode" | "AHx" => ascii_hex(&data),
                "ASCII85Decode" | "A85" => ascii85(&data),
                "RunLengthDecode" | "RL" => run_length(&data),
                "DCTDecode" | "DCT" | "CCITTFaxDecode" | "CCF" | "JBIG2Decode" | "JPXDecode"
                    if i == filters.len() - 1 =>
                {
                    match codec.as_deref_mut() {
                        Some(slot) => {
                            *slot = Some((f.clone(), p));
                            return Ok(data);
                        }
                        None => return Err("unexpected image data".into()),
                    }
                }
                other => return Err(format!("This PDF uses a kind of compression ({other}) that isn't supported.")),
            };
        }
        Ok(data)
    }

    /// PNG and TIFF predictors after Flate or LZW.
    fn predict(&self, data: Vec<u8>, p: &Dict) -> Result<Vec<u8>, String> {
        let predictor = self.number(p.get("Predictor")).unwrap_or(1.0) as i64;
        if predictor <= 1 {
            return Ok(data);
        }
        let colors = self.number(p.get("Colors")).unwrap_or(1.0).clamp(1.0, 32.0) as usize;
        let bpc = self.number(p.get("BitsPerComponent")).unwrap_or(8.0).clamp(1.0, 16.0) as usize;
        let columns = self.number(p.get("Columns")).unwrap_or(1.0).clamp(1.0, 1e6) as usize;
        let row = (colors * bpc * columns).div_ceil(8);
        let bpp = (colors * bpc).div_ceil(8).max(1);
        if predictor == 2 {
            let mut out = data;
            if bpc == 8 {
                for r in out.chunks_mut(row) {
                    for i in bpp..r.len() {
                        r[i] = r[i].wrapping_add(r[i - bpp]);
                    }
                }
            }
            return Ok(out);
        }
        // PNG: each row starts with its filter type.
        let mut out = Vec::with_capacity(data.len());
        let mut prev = vec![0u8; row];
        for chunk in data.chunks(row + 1) {
            if chunk.len() < 2 {
                break;
            }
            let filter = chunk[0];
            let mut cur = chunk[1..].to_vec();
            cur.resize(row, 0);
            for i in 0..row {
                let a = if i >= bpp { cur[i - bpp] } else { 0 };
                let b = prev[i];
                let c = if i >= bpp { prev[i - bpp] } else { 0 };
                cur[i] = cur[i].wrapping_add(match filter {
                    1 => a,
                    2 => b,
                    3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                    4 => {
                        let p = i16::from(a) + i16::from(b) - i16::from(c);
                        let (pa, pb, pc) =
                            ((p - i16::from(a)).abs(), (p - i16::from(b)).abs(), (p - i16::from(c)).abs());
                        if pa <= pb && pa <= pc {
                            a
                        } else if pb <= pc {
                            b
                        } else {
                            c
                        }
                    }
                    _ => 0,
                });
            }
            out.extend_from_slice(&cur);
            prev = cur;
        }
        Ok(out)
    }

    /// Page `index` drawn in grey, with its resolution in dots per inch.
    pub fn page(&self, index: usize, max_pixels: usize) -> Result<(Gray, Option<f32>), String> {
        let page = self.pages.get(index).ok_or("no such page")?.clone();
        let mut placed: Vec<(Obj, [f64; 6])> = Vec::new();
        let contents = page.dict.get("Contents").map(|c| self.resolve(c));
        let streams: Vec<Obj> = match contents {
            Some(Obj::Array(a)) => a.iter().map(|c| self.resolve(c)).collect(),
            Some(s @ Obj::Stream(..)) => vec![s],
            _ => Vec::new(),
        };
        let mut content = Vec::new();
        for s in streams {
            if let Obj::Stream(d, a, b) = s
                && let Ok(data) = self.decode_stream(&d, a, b, None)
            {
                content.extend_from_slice(&data);
                content.push(b'\n');
            }
        }
        self.images_in(&content, page.resources.as_ref(), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0, &mut placed);
        if placed.is_empty() {
            return Err("no image".into());
        }
        // Resolution: that of the sharpest image, at most 400 dpi.
        let [x0, y0, x1, y1] = page.media_box;
        let (pw, ph) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        let mut decoded: Vec<(Gray, bool, [f64; 6])> = Vec::new();
        let mut density = 0f64;
        for (obj, m) in &placed {
            let Some((img, mask)) = self.image(obj, max_pixels)? else { continue };
            let (sx, sy) = ((m[0] * m[0] + m[1] * m[1]).sqrt(), (m[2] * m[2] + m[3] * m[3]).sqrt());
            if sx < 1.0 || sy < 1.0 {
                continue;
            }
            // Pixels per point along the image's own axes.
            let d = (img.width as f64 / sx).max(img.height as f64 / sy);
            density = density.max(d);
            decoded.push((img, mask, *m));
        }
        if decoded.is_empty() {
            return Err("no image".into());
        }
        // Between 72 and 400 dpi.
        density = density.clamp(1.0, 400.0 / 72.0);
        let (mut cw, mut ch) = ((pw * density).round() as usize, (ph * density).round() as usize);
        if cw * ch > max_pixels {
            let f = (max_pixels as f64 / (cw * ch) as f64).sqrt();
            density *= f;
            cw = (pw * density) as usize;
            ch = (ph * density) as usize;
        }
        let mut canvas = Gray::new(cw.max(1), ch.max(1), 255);
        for (img, mask, m) in decoded {
            paint(&mut canvas, &img, mask, m, [x0, y0, x1, y1], density);
        }
        let turns = (page.rotate.rem_euclid(360) / 90) as u32;
        Ok((canvas.rotate90(turns), Some((density * 72.0) as f32)))
    }

    /// Images drawn by a content stream, with their transformation.
    fn images_in(
        &self,
        content: &[u8],
        resources: Option<&Obj>,
        ctm: [f64; 6],
        depth: usize,
        out: &mut Vec<(Obj, [f64; 6])>,
    ) {
        if depth > 8 {
            return;
        }
        let xobjects =
            resources.and_then(|r| self.dict_of(r)).and_then(|r| r.get("XObject").and_then(|x| self.dict_of(x)));
        let mut p = Parser::new(content, 0);
        let mut stack: Vec<[f64; 6]> = Vec::new();
        let mut m = ctm;
        let mut operands: Vec<Obj> = Vec::new();
        while let Some(o) = p.object() {
            let Obj::Op(op) = o else {
                operands.push(o);
                if operands.len() > 64 {
                    operands.remove(0);
                }
                continue;
            };
            match op.as_slice() {
                b"q" => {
                    if stack.len() < 64 {
                        stack.push(m);
                    }
                }
                b"Q" => m = stack.pop().unwrap_or(ctm),
                b"cm" if operands.len() >= 6 => {
                    let v: Vec<f64> =
                        operands[operands.len() - 6..].iter().map(|o| o.as_f64().unwrap_or(0.0)).collect();
                    m = multiply([v[0], v[1], v[2], v[3], v[4], v[5]], m);
                }
                b"Do" => {
                    if let (Some(Obj::Name(name)), Some(xo)) = (operands.last(), &xobjects)
                        && let Some(r) = xo.0.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
                    {
                        self.draw_xobject(self.resolve(&r), m, resources, depth, out);
                    }
                }
                b"BI" => {
                    // Inline image: skip its data (to "EI").
                    let rest = &content[p.pos..];
                    if let Some(id) = rest.windows(3).position(|w| w == b"ID " || w == b"ID\n" || w == b"ID\r") {
                        let from = p.pos + id + 3;
                        let after = content[from..]
                            .windows(4)
                            .position(|w| is_white(w[0]) && &w[1..3] == b"EI" && (is_white(w[3]) || is_delim(w[3])))
                            .map_or(content.len(), |e| from + e + 3);
                        p.pos = after;
                    }
                }
                _ => {}
            }
            operands.clear();
        }
    }

    /// An XObject drawn with transformation `m`: an image is recorded, a
    /// form's own content is followed.
    fn draw_xobject(
        &self,
        target: Obj,
        m: [f64; 6],
        resources: Option<&Obj>,
        depth: usize,
        out: &mut Vec<(Obj, [f64; 6])>,
    ) {
        let Obj::Stream(d, a, b) = &target else { return };
        match d.get("Subtype").and_then(Obj::as_name) {
            Some(b"Image") => out.push((target.clone(), m)),
            Some(b"Form") => {
                let fm = match d.get("Matrix").map(|x| self.resolve(x)) {
                    Some(Obj::Array(v)) if v.len() == 6 => {
                        let v: Vec<f64> = v.iter().map(|x| self.number(Some(x)).unwrap_or(0.0)).collect();
                        [v[0], v[1], v[2], v[3], v[4], v[5]]
                    }
                    _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                };
                if let Ok(inner) = self.decode_stream(d, *a, *b, None) {
                    let res = d.get("Resources").cloned().or_else(|| resources.cloned());
                    self.images_in(&inner, res.as_ref(), multiply(fm, m), depth + 1, out);
                }
            }
            _ => {}
        }
    }

    /// An image XObject as grey, and whether it is a stencil mask.
    fn image(&self, obj: &Obj, max_pixels: usize) -> Result<Option<(Gray, bool)>, String> {
        let Obj::Stream(d, a, b) = obj else { return Ok(None) };
        let w = self.number(d.get("Width").or_else(|| d.get("W"))).unwrap_or(0.0) as usize;
        let h = self.number(d.get("Height").or_else(|| d.get("H"))).unwrap_or(0.0) as usize;
        if w == 0 || h == 0 {
            return Ok(None);
        }
        if w > 30_000 || h > 30_000 || w * h > max_pixels * 2 {
            return Err(format!("A scanned page in the PDF is too large ({w} x {h} pixels)."));
        }
        let mask = matches!(d.get("ImageMask").map(|m| self.resolve(m)), Some(Obj::Bool(true)));
        let bpc = if mask { 1 } else { self.number(d.get("BitsPerComponent")).unwrap_or(8.0) as usize };
        let inverted = match d.get("Decode").map(|x| self.resolve(x)) {
            Some(Obj::Array(v)) if v.len() >= 2 => {
                self.number(v.first()).unwrap_or(0.0) > self.number(v.get(1)).unwrap_or(1.0)
            }
            _ => false,
        };
        let mut codec = None;
        let data = self.decode_stream(d, *a, *b, Some(&mut codec))?;
        let gray = match codec {
            Some((name, p)) => match name.as_slice() {
                b"DCTDecode" | b"DCT" => {
                    let limits = crate::image::Limits {
                        max_width: 30_000,
                        max_height: 30_000,
                        max_pixels: (max_pixels * 2) as u64,
                    };
                    let img = crate::image::jpeg::decode(&data, &limits)
                        .map_err(|e| format!("A scanned page in the PDF couldn't be read ({e})."))?;
                    let mut g = Gray::from_rgba(&img);
                    if inverted {
                        g.px.iter_mut().for_each(|v| *v = 255 - *v);
                    }
                    g
                }
                b"CCITTFaxDecode" | b"CCF" => {
                    let num = |k: &str, default: f64| self.number(p.get(k)).unwrap_or(default);
                    let params = ccitt::Params {
                        k: num("K", 0.0) as i32,
                        columns: num("Columns", 1728.0) as usize,
                        rows: num("Rows", h as f64) as usize,
                        byte_align: matches!(p.get("EncodedByteAlign"), Some(Obj::Bool(true))),
                        no_eol: false,
                        lsb_first: false,
                    };
                    let (px, rows) = ccitt::decode(&data, &params, h)
                        .map_err(|e| format!("A scanned page in the PDF couldn't be read ({e})."))?;
                    let black_is_1 = matches!(p.get("BlackIs1"), Some(Obj::Bool(true)));
                    // Decoded "white" is sample value 1 unless BlackIs1; the
                    // image then shows 1 as white unless Decode is [1 0].
                    let flip = black_is_1 != inverted;
                    let cols = params.columns;
                    let mut g = Gray::new(w, h, 255);
                    for y in 0..rows.min(h) {
                        for x in 0..w.min(cols) {
                            let v = px[y * cols + x];
                            g.px[y * w + x] = if flip { 255 - v } else { v };
                        }
                    }
                    g
                }
                b"JBIG2Decode" => {
                    return Err("This PDF uses JBIG2 compression for its black-and-white pages, which isn't supported. Scan in greyscale or colour, or save the scan as TIFF instead.".into());
                }
                _ => {
                    return Err("This PDF uses JPEG 2000 compression, which isn't supported. Scan with ordinary JPEG compression, or save the scan as TIFF instead.".into());
                }
            },
            None => {
                let (comps, palette, kind) = self.color_space(d.get("ColorSpace").or_else(|| d.get("CS")), mask);
                let row_bytes = (w * comps * bpc).div_ceil(8);
                let mut raw = data;
                raw.resize(row_bytes * h, 0);
                let photometric = match kind {
                    Space::Cmyk => 5,
                    Space::Rgb => 2,
                    Space::Indexed => 3,
                    Space::Ink => 0,
                    Space::Gray => 1,
                };
                let mut g = super::tiff::samples_to_gray(&raw, w, h, comps, bpc, photometric, palette)?;
                if inverted {
                    g.px.iter_mut().for_each(|v| *v = 255 - *v);
                }
                g
            }
        };
        Ok(Some((gray, mask)))
    }

    /// Components, palette (as grey levels) and kind of a colour space.
    fn color_space(&self, cs: Option<&Obj>, mask: bool) -> (usize, Option<Vec<u8>>, Space) {
        if mask {
            return (1, None, Space::Gray);
        }
        let cs = cs.map(|c| self.resolve(c)).unwrap_or(Obj::Name(b"DeviceGray".to_vec()));
        let name_space = |n: &[u8]| match n {
            b"DeviceRGB" | b"RGB" | b"CalRGB" | b"Lab" => (3, None, Space::Rgb),
            b"DeviceCMYK" | b"CMYK" => (4, None, Space::Cmyk),
            _ => (1, None, Space::Gray),
        };
        match cs {
            Obj::Name(n) => name_space(&n),
            Obj::Array(a) if !a.is_empty() => {
                let kind = self.resolve(&a[0]).as_name().map(<[u8]>::to_vec).unwrap_or_default();
                match kind.as_slice() {
                    b"ICCBased" => {
                        let n =
                            a.get(1).and_then(|s| self.dict_of(s)).and_then(|d| self.number(d.get("N"))).unwrap_or(3.0)
                                as usize;
                        match n {
                            1 => (1, None, Space::Gray),
                            4 => (4, None, Space::Cmyk),
                            _ => (3, None, Space::Rgb),
                        }
                    }
                    b"Indexed" | b"I" if a.len() >= 4 => {
                        let (base, _, base_kind) = self.color_space(a.get(1), false);
                        let hival = self.number(a.get(2)).unwrap_or(255.0).clamp(0.0, 255.0) as usize;
                        let lookup = match self.resolve(&a[3]) {
                            Obj::Str(s) => s,
                            Obj::Stream(d, s, e) => self.decode_stream(&d, s, e, None).unwrap_or_default(),
                            _ => Vec::new(),
                        };
                        let palette: Vec<u8> = (0..=255usize)
                            .map(|i| {
                                let i = i.min(hival);
                                let entry = lookup.get(i * base..i * base + base).unwrap_or(&[]);
                                match (base_kind, entry) {
                                    (Space::Rgb, [r, g, b]) => luma(*r, *g, *b),
                                    (Space::Cmyk, [c, m, y, k]) => {
                                        let ink = |v: u8| 255 - (255u16.min(u16::from(v) + u16::from(*k)) as u8);
                                        luma(ink(*c), ink(*m), ink(*y))
                                    }
                                    (_, [v, ..]) => *v,
                                    _ => 255,
                                }
                            })
                            .collect();
                        (1, Some(palette), Space::Indexed)
                    }
                    b"Separation" => (1, None, Space::Ink),
                    b"DeviceN" => {
                        let n = match a.get(1).map(|x| self.resolve(x)) {
                            Some(Obj::Array(names)) => names.len().max(1),
                            _ => 1,
                        };
                        (n, None, if n == 1 { Space::Ink } else { Space::Gray })
                    }
                    other => name_space(other),
                }
            }
            _ => (1, None, Space::Gray),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    Indexed,
    /// One ink: 1 is full ink (dark).
    Ink,
}

/// `a` then `b`: the matrix for applying a's transform before b's.
fn multiply(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// Draw an image (placed by `m`, which maps the unit square to the page)
/// onto the canvas, which shows `page_box` at `density` pixels per point.
fn paint(canvas: &mut Gray, img: &Gray, mask: bool, m: [f64; 6], page_box: [f64; 4], density: f64) {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-9 {
        return;
    }
    // Shrink large images first, so sampling averages rather than skips.
    let (sx, sy) = ((m[0] * m[0] + m[1] * m[1]).sqrt() * density, (m[2] * m[2] + m[3] * m[3]).sqrt() * density);
    let img = if (img.width as f64) > sx * 1.5 && (img.height as f64) > sy * 1.5 && !mask {
        img.resize(sx.round().max(1.0) as usize, sy.round().max(1.0) as usize)
    } else {
        img.clone()
    };
    let (iw, ih) = (img.width as f64, img.height as f64);
    let [x0, _, _, y1] = page_box;
    // The image's corners on the canvas, to limit the work.
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|(u, v)| {
        let (x, y) = (m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5]);
        ((x - x0) * density, (y1 - y) * density)
    });
    let min_x = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min).floor().max(0.0) as usize;
    let max_x = (corners.iter().map(|c| c.0).fold(f64::MIN, f64::max).ceil().max(0.0) as usize).min(canvas.width);
    let min_y = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min).floor().max(0.0) as usize;
    let max_y = (corners.iter().map(|c| c.1).fold(f64::MIN, f64::max).ceil().max(0.0) as usize).min(canvas.height);
    let inv = [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det];
    for cy in min_y..max_y {
        let y = y1 - (cy as f64 + 0.5) / density;
        for cx in min_x..max_x {
            let x = x0 + (cx as f64 + 0.5) / density;
            let (dx, dy) = (x - m[4], y - m[5]);
            let u = inv[0] * dx + inv[2] * dy;
            let v = inv[1] * dx + inv[3] * dy;
            if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                continue;
            }
            let px = ((u * iw) as usize).min(img.width - 1);
            let py = (((1.0 - v) * ih) as usize).min(img.height - 1);
            let value = img.px[py * img.width + px];
            let target = &mut canvas.px[cy * canvas.width + cx];
            if mask {
                // Stencil: 0 samples are painted (in black, as scanners'
                // text layers are).
                if value < 128 {
                    *target = 0;
                }
            } else {
                *target = value;
            }
        }
    }
}

fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let digits: Vec<u8> = data.iter().copied().take_while(|&c| c != b'>').filter(u8::is_ascii_hexdigit).collect();
    digits
        .chunks(2)
        .map(|p| {
            let s = std::str::from_utf8(p).unwrap_or("0");
            let v = u8::from_str_radix(s, 16).unwrap_or(0);
            if p.len() == 1 { v << 4 } else { v }
        })
        .collect()
}

fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut group = Vec::with_capacity(5);
    for &c in data {
        match c {
            b'~' => break,
            b'z' if group.is_empty() => out.extend_from_slice(&[0, 0, 0, 0]),
            b'!'..=b'u' => {
                group.push(u32::from(c - b'!'));
                if group.len() == 5 {
                    let v = group.iter().fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
                    out.extend_from_slice(&v.to_be_bytes());
                    group.clear();
                }
            }
            _ => {}
        }
    }
    if group.len() > 1 {
        let n = group.len();
        while group.len() < 5 {
            group.push(84);
        }
        let v = group.iter().fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    out
}

fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let n = data[i];
        i += 1;
        if n == 128 {
            break;
        }
        if n < 128 {
            let end = (i + n as usize + 1).min(data.len());
            out.extend_from_slice(&data[i..end]);
            i = end;
        } else if let Some(&b) = data.get(i) {
            out.extend(std::iter::repeat_n(b, 257 - n as usize));
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_objects() {
        let mut p = Parser::new(b"<< /Type /XObject /W 10 /Ref 12 0 R /A [1 2.5 (x\\)y)] /N#20x <414243> >>", 0);
        let Some(Obj::Dict(d)) = p.object() else { panic!() };
        assert_eq!(d.get("Type"), Some(&Obj::Name(b"XObject".to_vec())));
        assert_eq!(d.get("W"), Some(&Obj::Int(10)));
        assert_eq!(d.get("Ref"), Some(&Obj::Ref(12, 0)));
        assert_eq!(d.get("A"), Some(&Obj::Array(vec![Obj::Int(1), Obj::Real(2.5), Obj::Str(b"x)y".to_vec())])));
        assert_eq!(d.get("N x"), Some(&Obj::Str(b"ABC".to_vec())));
    }

    #[test]
    fn filters() {
        assert_eq!(ascii_hex(b"41 42 4>"), b"AB@");
        assert_eq!(ascii85(b"87cURD]i,\"Ebo80~>"), b"Hello World!");
        assert_eq!(run_length(&[2, b'a', b'b', b'c', 254, b'z', 128]), b"abczzz");
    }

    #[test]
    fn matrices() {
        let scale = [2.0, 0.0, 0.0, 3.0, 0.0, 0.0];
        let shift = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
        // Scale, then shift.
        assert_eq!(multiply(scale, shift), [2.0, 0.0, 0.0, 3.0, 10.0, 20.0]);
        // Shift, then scale.
        assert_eq!(multiply(shift, scale), [2.0, 0.0, 0.0, 3.0, 20.0, 60.0]);
    }
}
