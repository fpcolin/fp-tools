//! Cleaning up a scanned page before reading it: separating ink from paper,
//! finding the text size, spotting sideways pages, straightening, and
//! finding (then erasing) the ruled lines of tables. (Upside-down pages are
//! spotted by the text recogniser: see `ocr::is_upside_down`.)

use super::gray::Gray;

/// Ink (1) or paper (0) per pixel.
#[derive(Clone, Debug)]
pub struct Binary {
    pub width: usize,
    pub height: usize,
    pub ink: Vec<u8>,
}

impl Binary {
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> bool {
        self.ink[y * self.width + x] != 0
    }

    /// Shrink by an integer factor; a pixel is ink if any source pixel is.
    pub fn shrink(&self, factor: usize) -> Binary {
        let (w, h) = (self.width.div_ceil(factor), self.height.div_ceil(factor));
        let mut ink = vec![0u8; w * h];
        for y in 0..self.height {
            let row = &self.ink[y * self.width..(y + 1) * self.width];
            let out = &mut ink[(y / factor) * w..(y / factor + 1) * w];
            for (x, &v) in row.iter().enumerate() {
                out[x / factor] |= v;
            }
        }
        Binary { width: w, height: h, ink }
    }
}

/// Adaptive threshold (Bradley and Roth): a pixel is ink when it is clearly
/// darker than the average of its neighbourhood, which copes with shading,
/// coloured paper and uneven lighting.
pub fn binarize(g: &Gray) -> Binary {
    let (w, h) = (g.width, g.height);
    let mut integral = vec![0u64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row_sum = 0u64;
        for x in 0..w {
            row_sum += u64::from(g.px[y * w + x]);
            integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row_sum;
        }
    }
    let r = (w.max(h) / 32).max(8);
    let mut ink = vec![0u8; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let count = ((y1 - y0) * (x1 - x0)) as u64;
            let sum = integral[y1 * (w + 1) + x1] + integral[y0 * (w + 1) + x0]
                - integral[y0 * (w + 1) + x1]
                - integral[y1 * (w + 1) + x0];
            let v = u64::from(g.px[y * w + x]);
            // Darker than 85% of the local mean, and not near-white.
            if v * count * 100 < sum * 85 && v < 230 {
                ink[y * w + x] = 1;
            }
        }
    }
    Binary { width: w, height: h, ink }
}

/// A connected blob of ink.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Blob {
    pub left: usize,
    pub top: usize,
    pub right: usize,
    pub bottom: usize,
    pub pixels: usize,
}

impl Blob {
    pub fn width(&self) -> usize {
        self.right - self.left + 1
    }
    pub fn height(&self) -> usize {
        self.bottom - self.top + 1
    }
}

/// 8-connected components of the ink.
pub fn blobs(b: &Binary) -> Vec<Blob> {
    let (w, h) = (b.width, b.height);
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if b.ink[start] == 0 || seen[start] {
            continue;
        }
        let mut blob = Blob { left: usize::MAX, top: usize::MAX, right: 0, bottom: 0, pixels: 0 };
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            blob.left = blob.left.min(x);
            blob.right = blob.right.max(x);
            blob.top = blob.top.min(y);
            blob.bottom = blob.bottom.max(y);
            blob.pixels += 1;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if b.ink[j] != 0 && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push(blob);
    }
    out
}

/// Blobs that look like characters (not specks, lines or pictures), for a
/// page whose text is roughly `max_h` pixels tall at most.
pub fn char_blobs(all: &[Blob], max_h: usize) -> Vec<Blob> {
    all.iter()
        .copied()
        .filter(|b| {
            let (bw, bh) = (b.width(), b.height());
            bh >= 4 && bh <= max_h && bw <= bh * 4 && bw * 8 >= bh && b.pixels >= 8
        })
        .collect()
}

fn median(mut v: Vec<usize>) -> Option<usize> {
    if v.is_empty() {
        return None;
    }
    let mid = v.len() / 2;
    Some(*v.select_nth_unstable(mid).1)
}

/// Typical height of a character in pixels (between x-height and capital
/// height for ordinary text), or None if there is hardly any text.
pub fn text_height(b: &Binary) -> Option<f32> {
    // Work on a half-size copy: quicker, and plenty for this.
    let small = b.shrink(2);
    let all = blobs(&small);
    let chars = char_blobs(&all, small.height / 8);
    if chars.len() < 20 {
        return None;
    }
    let m = median(chars.iter().map(Blob::height).collect())?;
    Some(m as f32 * 2.0)
}

/// Neighbouring characters sit side by side in upright (or upside-down)
/// text and one above the other in sideways text. Returns true if the page
/// clearly reads sideways.
fn is_sideways(chars: &[Blob]) -> bool {
    if chars.len() < 40 {
        return false;
    }
    let cell = 64usize;
    let mut grid: std::collections::HashMap<(usize, usize), Vec<usize>> = std::collections::HashMap::new();
    for (i, c) in chars.iter().enumerate() {
        grid.entry(((c.left + c.right) / 2 / cell, (c.top + c.bottom) / 2 / cell)).or_default().push(i);
    }
    let (mut across, mut down) = (0usize, 0usize);
    for (i, c) in chars.iter().enumerate() {
        let (gx, gy) = ((c.left + c.right) / 2 / cell, (c.top + c.bottom) / 2 / cell);
        let size = c.height().max(c.width()) as i64;
        let mut best: Option<(i64, bool)> = None;
        for ny in gy.saturating_sub(1)..=gy + 1 {
            for nx in gx.saturating_sub(1)..=gx + 1 {
                for &j in grid.get(&(nx, ny)).into_iter().flatten() {
                    if j == i {
                        continue;
                    }
                    let o = &chars[j];
                    let overlap_y = c.bottom.min(o.bottom) as i64 - c.top.max(o.top) as i64;
                    let overlap_x = c.right.min(o.right) as i64 - c.left.max(o.left) as i64;
                    let gap_x = (o.left as i64 - c.right as i64).max(c.left as i64 - o.right as i64);
                    let gap_y = (o.top as i64 - c.bottom as i64).max(c.top as i64 - o.bottom as i64);
                    let candidate = if overlap_y * 2 > size && gap_x >= 0 {
                        Some((gap_x, true))
                    } else if overlap_x * 2 > size && gap_y >= 0 {
                        Some((gap_y, false))
                    } else {
                        None
                    };
                    if let Some(cand) = candidate
                        && best.is_none_or(|b| cand.0 < b.0)
                    {
                        best = Some(cand);
                    }
                }
            }
        }
        if let Some((gap, side_by_side)) = best
            && gap <= size
        {
            if side_by_side {
                across += 1;
            } else {
                down += 1;
            }
        }
    }
    down > 30 && down > across * 2
}

/// Whether the page's text runs sideways (a quarter turn either way).
pub fn is_page_sideways(b: &Binary, text_h: f32) -> bool {
    let factor = if text_h > 24.0 { 2 } else { 1 };
    let small = b.shrink(factor);
    let max_h = ((text_h * 3.0) as usize / factor).max(8);
    is_sideways(&char_blobs(&blobs(&small), max_h))
}

/// The page's tilt in degrees (positive = text runs downhill to the right),
/// found by trying angles and keeping the one whose rows of ink line up
/// best. Zero if there is too little ink to tell.
pub fn skew_angle(b: &Binary) -> f32 {
    let factor = 4;
    let small = b.shrink(factor);
    let mut points: Vec<(f32, f32)> = Vec::new();
    for y in 0..small.height {
        for x in 0..small.width {
            if small.ink[y * small.width + x] != 0 {
                points.push((x as f32, y as f32));
            }
        }
    }
    if points.len() < 200 {
        return 0.0;
    }
    // Keep it quick on very busy pages.
    if points.len() > 150_000 {
        let step = points.len() / 150_000 + 1;
        points = points.into_iter().step_by(step).collect();
    }
    let height = small.height as f32;
    let width = small.width as f32;
    let score = |deg: f32| -> f64 {
        let t = deg.to_radians().tan();
        let offset = width * t.abs() + 2.0;
        let bins = (height + 2.0 * offset) as usize + 2;
        let mut hist = vec![0u32; bins];
        for &(x, y) in &points {
            let v = y - x * t + offset;
            hist[v as usize] += 1;
        }
        hist.iter().map(|&c| f64::from(c) * f64::from(c)).sum()
    };
    let search = |from: f32, to: f32, step: f32| -> f32 {
        let mut best = (f64::MIN, 0.0f32);
        let mut a = from;
        while a <= to + 1e-4 {
            let s = score(a);
            if s > best.0 {
                best = (s, a);
            }
            a += step;
        }
        best.1
    };
    let coarse = search(-6.0, 6.0, 0.25);
    let fine = search(coarse - 0.25, coarse + 0.25, 0.025);
    if fine.abs() < 0.05 { 0.0 } else { fine }
}

/// A ruled line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    /// Centre: y for a horizontal line, x for a vertical one.
    pub pos: f32,
    /// Extent along the line.
    pub start: f32,
    pub end: f32,
    pub thickness: f32,
}

impl Rule {
    pub fn len(&self) -> f32 {
        self.end - self.start
    }
}

/// Ruled lines on the page: (horizontal, vertical).
pub fn find_rules(b: &Binary, text_h: f32) -> (Vec<Rule>, Vec<Rule>) {
    let h_min = (text_h * 4.0).max(60.0) as usize;
    let v_min = (text_h * 2.2).max(50.0) as usize;
    let max_thick = (text_h * 0.3).max(5.0) + 2.0;
    let horizontal = scan_rules(b.width, b.height, |x, y| b.at(x, y), h_min, max_thick);
    let vertical = scan_rules(b.height, b.width, |y, x| b.at(x, y), v_min, max_thick);
    (horizontal, vertical)
}

/// Find long thin runs along the first axis (`len` long, `across` lines of
/// them), joining runs on neighbouring lines into one rule.
fn scan_rules(
    len: usize,
    across: usize,
    ink: impl Fn(usize, usize) -> bool,
    min_len: usize,
    max_thick: f32,
) -> Vec<Rule> {
    const GAP: usize = 3;
    struct Open {
        first: usize,
        last: usize,
        start: usize,
        end: usize,
        /// Sum of line index weighted by run length, and total length.
        weighted: f64,
        total: f64,
    }
    let mut open: Vec<Open> = Vec::new();
    let mut done: Vec<Open> = Vec::new();
    for line in 0..across {
        // Runs of ink along this line, bridging small gaps.
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < len {
            if !ink(i, line) {
                i += 1;
                continue;
            }
            let start = i;
            let mut end = i;
            let mut gap = 0;
            while i < len {
                if ink(i, line) {
                    end = i;
                    gap = 0;
                } else {
                    gap += 1;
                    if gap > GAP {
                        break;
                    }
                }
                i += 1;
            }
            if end + 1 - start >= min_len {
                runs.push((start, end));
            }
        }
        // Extend open rules with overlapping runs; close the rest.
        let mut next: Vec<Open> = Vec::new();
        for (s, e) in runs {
            let overlapping = open.iter().position(|o| {
                let ov = e.min(o.end) as i64 - s.max(o.start) as i64;
                ov > ((e - s).min(o.end - o.start) / 2) as i64
            });
            if let Some(k) = overlapping {
                let mut o = open.swap_remove(k);
                o.last = line;
                o.start = o.start.min(s);
                o.end = o.end.max(e);
                o.weighted += line as f64 * (e - s + 1) as f64;
                o.total += (e - s + 1) as f64;
                next.push(o);
            } else {
                next.push(Open {
                    first: line,
                    last: line,
                    start: s,
                    end: e,
                    weighted: line as f64 * (e - s + 1) as f64,
                    total: (e - s + 1) as f64,
                });
            }
        }
        // Rules that skipped this line stay open for one more line.
        for o in open.drain(..) {
            if o.last + 1 >= line {
                next.push(o);
            } else {
                done.push(o);
            }
        }
        open = next;
    }
    done.extend(open);
    // A rule's soft edges can come out as short side-by-side pieces: merge
    // anything that runs alongside and overlaps.
    done.sort_by_key(|o| o.first);
    let mut merged: Vec<Open> = Vec::new();
    for o in done {
        let joined = merged
            .iter_mut()
            .rev()
            .find(|m| o.first <= m.last + 2 && o.last + 2 >= m.first && o.start <= m.end && o.end >= m.start);
        match joined {
            Some(m) => {
                m.first = m.first.min(o.first);
                m.last = m.last.max(o.last);
                m.start = m.start.min(o.start);
                m.end = m.end.max(o.end);
                m.weighted += o.weighted;
                m.total += o.total;
            }
            None => merged.push(o),
        }
    }
    merged
        .into_iter()
        .filter(|o| ((o.last - o.first + 1) as f32) <= max_thick)
        .map(|o| Rule {
            pos: (o.weighted / o.total) as f32 + 0.5,
            start: o.start as f32,
            end: o.end as f32 + 1.0,
            thickness: (o.last - o.first + 1) as f32,
        })
        .collect()
}

/// Paint ruled lines white, so they are not mistaken for text.
pub fn erase_rules(g: &mut Gray, horizontal: &[Rule], vertical: &[Rule]) {
    let (w, h) = (g.width as i64, g.height as i64);
    for r in horizontal {
        let half = r.thickness / 2.0 + 2.0;
        let (y0, y1) = (((r.pos - half).floor() as i64).max(0), ((r.pos + half).ceil() as i64).min(h - 1));
        let (x0, x1) = ((r.start as i64 - 2).max(0), (r.end as i64 + 2).min(w - 1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                g.px[(y * w + x) as usize] = 255;
            }
        }
    }
    for r in vertical {
        let half = r.thickness / 2.0 + 2.0;
        let (x0, x1) = (((r.pos - half).floor() as i64).max(0), ((r.pos + half).ceil() as i64).min(w - 1));
        let (y0, y1) = ((r.start as i64 - 2).max(0), (r.end as i64 + 2).min(h - 1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                g.px[(y * w + x) as usize] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_with(lines: &[(usize, usize, usize, usize)]) -> Gray {
        let mut g = Gray::new(400, 300, 250);
        for &(x0, y0, x1, y1) in lines {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    g.px[y * 400 + x] = 10;
                }
            }
        }
        g
    }

    #[test]
    fn finds_ruled_lines() {
        let g = page_with(&[(20, 50, 380, 52), (20, 150, 380, 151), (20, 50, 22, 151), (379, 50, 380, 151)]);
        let b = binarize(&g);
        let (h, v) = find_rules(&b, 20.0);
        assert_eq!(h.len(), 2, "{h:?}");
        assert!((h[0].pos - 51.5).abs() < 1.0 && h[0].len() > 350.0);
        assert_eq!(v.len(), 2, "{v:?}");
        assert!((v[0].pos - 21.5).abs() < 1.0 && v[0].len() > 95.0);
    }

    #[test]
    fn short_marks_are_not_lines() {
        let g = page_with(&[(20, 50, 60, 52), (100, 100, 101, 120)]);
        let (h, v) = find_rules(&binarize(&g), 20.0);
        assert!(h.is_empty() && v.is_empty());
    }
}
