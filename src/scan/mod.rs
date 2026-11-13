//! Scan to Excel: read the tables on a scanned page and write them to an
//! Excel workbook.
//!
//! For each page: find the text size, turn the page upright and straighten
//! it ([`prep`]), find and erase ruled lines, find the words ([`ocr`]), work
//! out rows and columns ([`table`]), read each cell's text ([`ocr`]), decide
//! what is a number or date ([`values`]), and write the workbook ([`xlsx`]).

pub mod ccitt;
pub mod gray;
pub mod input;
pub mod ocr;
pub mod pdf;
pub mod prep;
pub mod table;
pub mod tiff;
pub mod values;
pub mod xlsx;

use std::sync::atomic::{AtomicBool, Ordering};

use gray::Gray;
use ocr::Bounds;
use values::Value;

/// One page's table, ready to become a worksheet.
#[derive(Clone, Debug, Default)]
pub struct PageTable {
    pub rows: usize,
    pub cols: usize,
    pub cells: Vec<TableCell>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableCell {
    pub row: usize,
    pub col: usize,
    pub rows: usize,
    pub cols: usize,
    pub value: Option<Value>,
    pub boxed: bool,
}

impl TableCell {
    /// The cell's text as read (numbers as printed).
    pub fn text(&self) -> String {
        match &self.value {
            Some(Value::Text(t)) => t.clone(),
            Some(Value::Number(v, f)) => display_number(*v, f),
            None => String::new(),
        }
    }
}

/// A number shown roughly as Excel would with its format (for previews).
pub fn display_number(v: f64, format: &str) -> String {
    if values::formats::DATES.contains(&format) {
        // Back from Excel's day count to the date as printed.
        let days = v as i64 + values::days_from_civil(1899, 12, 30);
        let (y, m, d) = civil_from_days(days);
        let y = if format.ends_with("yyyy") { y.to_string() } else { format!("{:02}", y % 100) };
        return if format.starts_with("mm") { format!("{m:02}/{d:02}/{y}") } else { format!("{m}/{d}/{y}") };
    }
    let percent = format.contains('%');
    let shown = if percent { v * 100.0 } else { v };
    let decimals = format
        .split(';')
        .next()
        .unwrap_or("")
        .rsplit_once('.')
        .map_or(0, |(_, f)| f.chars().take_while(|&c| c == '0').count());
    let mut s =
        if format == values::formats::GENERAL { format!("{shown}") } else { format!("{:.*}", decimals, shown.abs()) };
    if format.contains(',') {
        let (int, frac) = s.split_once('.').map_or((s.as_str(), None), |(i, f)| (i, Some(f)));
        let mut grouped = String::new();
        for (i, ch) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        s = match frac {
            Some(f) => format!("{grouped}.{f}"),
            None => grouped,
        };
    }
    if format.contains('$') {
        s = format!("${s}");
    }
    if percent {
        s.push('%');
    }
    if format != values::formats::GENERAL && v < 0.0 {
        s = if format.contains('(') { format!("({s})") } else { format!("-{s}") };
    }
    s
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Why a scan could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    Cancelled,
    /// A message for the user.
    Failed(String),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::Cancelled => f.write_str("Cancelled."),
            ScanError::Failed(m) => f.write_str(m),
        }
    }
}

impl From<String> for ScanError {
    fn from(m: String) -> Self {
        ScanError::Failed(m)
    }
}

/// The largest page worked on, in pixels; bigger scans are scaled down.
const MAX_PIXELS: usize = 30_000_000;
/// Text height (pixels) pages are scaled towards when far from it.
const TARGET_TEXT_H: f32 = 30.0;

fn check(cancel: &AtomicBool) -> Result<(), ScanError> {
    if cancel.load(Ordering::Relaxed) { Err(ScanError::Cancelled) } else { Ok(()) }
}

/// Read the table(s) on one page.
pub fn read_page(page: Gray, cancel: &AtomicBool) -> Result<PageTable, ScanError> {
    let mut g = if page.is_bilevel() { page.soften() } else { page };
    if g.width * g.height > MAX_PIXELS {
        let f = (MAX_PIXELS as f32 / (g.width * g.height) as f32).sqrt();
        g = g.scale(f);
    }
    let mut b = prep::binarize(&g);
    let Some(mut th) = prep::text_height(&b) else { return Ok(PageTable::default()) };
    // Very small or very large text: rescale towards the usual size.
    if !(22.0..=48.0).contains(&th) {
        let f = (TARGET_TEXT_H / th).clamp(0.25, 3.0);
        if (g.width * g.height) as f32 * f * f <= MAX_PIXELS as f32 * 1.5 {
            g = g.scale(f);
            th *= f;
            b = prep::binarize(&g);
        }
    }
    check(cancel)?;
    if prep::is_page_sideways(&b, th) {
        g = g.rotate90(1);
        b = prep::binarize(&g);
    }
    let skew = prep::skew_angle(&b);
    if skew != 0.0 {
        g = g.rotate_small(-skew);
        b = prep::binarize(&g);
    }
    check(cancel)?;
    let (mut h_rules, mut v_rules) = prep::find_rules(&b, th);
    prep::erase_rules(&mut g, &h_rules, &v_rules);
    let mut clean = prep::binarize(&g);
    check(cancel)?;

    let mut words: Vec<Bounds> =
        ocr::detect_words(&g, th)?.into_iter().filter_map(|w| tighten(&clean, &w, th)).collect();
    check(cancel)?;
    // Upside down? Turn the page, and what was found on it, round.
    if ocr::is_upside_down(&g, &words, th)? {
        let (w, h) = (g.width as f32, g.height as f32);
        g = g.rotate90(2);
        clean = prep::binarize(&g);
        let turn = |b: &Bounds| Bounds { left: w - b.right, right: w - b.left, top: h - b.bottom, bottom: h - b.top };
        words = words.iter().map(turn).collect();
        let flip = |r: &prep::Rule, along: f32, across: f32| prep::Rule {
            pos: across - r.pos,
            start: along - r.end,
            end: along - r.start,
            thickness: r.thickness,
        };
        h_rules = h_rules.iter().map(|r| flip(r, w, h)).collect();
        v_rules = v_rules.iter().map(|r| flip(r, h, w)).collect();
    }
    // Dashes standing between words ("Labor - install") are not picked up
    // as words, nor read reliably; find them on the page instead.
    let blobs = prep::blobs(&clean);
    // Single characters ("Y", "1") the detector can miss.
    let missed = missed_characters(&blobs, &words, th);
    words.extend(missed);
    absorb_marks(&blobs, &mut words, th);
    let dashes = find_dashes(&blobs, &words, th);
    words.extend(dashes.iter().filter(|d| !d.in_word).map(|d| d.at));
    check(cancel)?;
    let layout = table::layout(&words, &h_rules, &v_rules, th);

    // Read every line of every cell in one go (the engine batches them).
    let pad = th * 0.15;
    let mut boxes: Vec<Bounds> = Vec::new();
    for cell in &layout.cells {
        for l in &cell.lines {
            boxes.push(Bounds {
                left: (l.left - pad).max(0.0),
                top: (l.top - pad).max(0.0),
                right: (l.right + pad).min(g.width as f32),
                bottom: (l.bottom + pad).min(g.height as f32),
            });
        }
    }
    let prepared = ocr::prepare(&g)?;
    let texts = if boxes.is_empty() { Vec::new() } else { ocr::read_lines(&prepared, &boxes)? };
    check(cancel)?;

    let mut texts = texts.into_iter();
    let mut cells: Vec<(table::Cell, String)> = Vec::new();
    for cell in layout.cells {
        let text = cell
            .lines
            .iter()
            .filter_map(|line| {
                let chars = texts.next()?;
                let inside: Vec<&Dash> = dashes.iter().filter(|d| line_has(line, &d.at)).collect();
                Some(with_dashes(chars, &inside))
            })
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        cells.push((cell, text));
    }
    Ok(typed(layout.rows, layout.cols, cells))
}

/// Letter-sized marks outside every detected word, grouped into words.
fn missed_characters(blobs: &[prep::Blob], words: &[Bounds], th: f32) -> Vec<Bounds> {
    let mut found: Vec<Bounds> = blobs
        .iter()
        .filter(|b| {
            let (w, h) = (b.width() as f32, b.height() as f32);
            (th * 0.55..=th * 1.8).contains(&h) && (th * 0.1..=th * 1.5).contains(&w) && b.pixels as f32 >= w * h * 0.12
        })
        .map(|b| Bounds {
            left: b.left as f32,
            top: b.top as f32,
            right: (b.right + 1) as f32,
            bottom: (b.bottom + 1) as f32,
        })
        .filter(|b| {
            let (cx, cy) = b.center();
            !words
                .iter()
                .any(|w| cx >= w.left - 2.0 && cx <= w.right + 2.0 && cy >= w.top - 2.0 && cy <= w.bottom + 2.0)
        })
        .collect();
    found.sort_by(|a, b| a.left.total_cmp(&b.left));
    let mut out: Vec<Bounds> = Vec::new();
    for b in found {
        let near = out.iter_mut().rev().find(|o| {
            let overlap = b.bottom.min(o.bottom) - b.top.max(o.top);
            overlap >= b.height().min(o.height()) * 0.5 && b.left - o.right <= th * 0.35
        });
        match near {
            Some(o) => *o = o.union(&b),
            None => out.push(b),
        }
    }
    out
}

/// Commas and full stops just outside a word belong to it ("2," in
/// "2,770"; "Inc."): the detector sometimes leaves them out, which splits
/// numbers and loses the mark. At most two per word, so leader dots
/// ("Total ........ 12.00") do not join up the columns.
fn absorb_marks(blobs: &[prep::Blob], words: &mut [Bounds], th: f32) {
    let original: Vec<Bounds> = words.to_vec();
    let mut taken = vec![0u8; words.len()];
    for b in blobs {
        let (w, h) = (b.width() as f32, b.height() as f32);
        if h >= th * 0.55 || w >= th * 0.6 || b.pixels < 3 {
            continue;
        }
        let at = Bounds {
            left: b.left as f32,
            top: b.top as f32,
            right: (b.right + 1) as f32,
            bottom: (b.bottom + 1) as f32,
        };
        let (cx, cy) = at.center();
        if original.iter().any(|o| cx >= o.left && cx <= o.right && cy >= o.top && cy <= o.bottom) {
            continue;
        }
        let best = original
            .iter()
            .enumerate()
            .filter(|(i, o)| {
                let slack = o.height() * 0.35;
                taken[*i] < 2 && cy >= o.top - slack && cy <= o.bottom + slack
            })
            .map(|(i, o)| (i, (at.left - o.right).max(o.left - at.right)))
            .filter(|&(_, gap)| gap <= th * 0.45)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = best {
            words[i] = words[i].union(&at);
            taken[i] += 1;
        }
    }
}

/// A dash or hyphen found on the page.
#[derive(Clone, Copy, Debug)]
struct Dash {
    at: Bounds,
    /// With space on both sides ("Labor - install"), not "T-mold".
    spaced: bool,
    /// Inside a detected word already.
    in_word: bool,
}

/// Short level strokes at mid-height between letters: dashes and hyphens,
/// which the recogniser sometimes leaves out.
fn find_dashes(all: &[prep::Blob], words: &[Bounds], th: f32) -> Vec<Dash> {
    // Letters, sorted by left edge, to look for neighbours.
    let mut letters: Vec<&prep::Blob> =
        all.iter().filter(|c| c.height() as f32 >= th * 0.5 && c.height() as f32 <= th * 2.5).collect();
    letters.sort_by_key(|c| c.left);
    let mut out = Vec::new();
    for blob in all {
        let (w, h) = (blob.width() as f32, blob.height() as f32);
        if !(w >= h * 1.6 && h <= th * 0.35 && w >= th * 0.2 && w <= th * 1.6) {
            continue;
        }
        let at = Bounds {
            left: blob.left as f32,
            top: blob.top as f32,
            right: (blob.right + 1) as f32,
            bottom: (blob.bottom + 1) as f32,
        };
        let (_, cy) = at.center();
        // The nearest letter on each side, on the same line, with the stroke
        // between its middle and bottom thirds.
        let from = letters.partition_point(|c| (c.left as f32) < at.left - th * 3.0);
        let (mut left_gap, mut right_gap) = (f32::MAX, f32::MAX);
        for c in &letters[from..] {
            if c.left as f32 > at.right + th * 1.5 {
                break;
            }
            let (top, height) = (c.top as f32, c.height() as f32);
            if cy - top < height * 0.25 || cy - top > height * 0.85 {
                continue;
            }
            if (c.right as f32) < at.left {
                left_gap = left_gap.min(at.left - c.right as f32);
            } else if c.left as f32 > at.right {
                right_gap = right_gap.min(c.left as f32 - at.right);
            }
        }
        if left_gap > th * 1.2 || right_gap > th * 1.2 {
            continue;
        }
        let (cx, cy) = at.center();
        let in_word = words.iter().any(|wd| cx >= wd.left && cx <= wd.right && cy >= wd.top && cy <= wd.bottom);
        let spaced = left_gap.min(right_gap) >= th * 0.2;
        // Inside a word, only a dash with space around it ("Statement -
        // October" read as one word); a stroke between touching letters
        // could be part of one.
        if in_word && !spaced {
            continue;
        }
        out.push(Dash { at, spaced, in_word });
    }
    out
}

fn line_has(line: &Bounds, d: &Bounds) -> bool {
    let (cx, cy) = d.center();
    cx >= line.left && cx <= line.right && cy >= line.top && cy <= line.bottom
}

/// A line's text, putting back any dash the recogniser left out.
fn with_dashes(chars: Vec<(char, f32)>, dashes: &[&Dash]) -> String {
    let mut chars = chars;
    for d in dashes {
        let (cx, _) = d.at.center();
        // Already read (as - or a similar mark) near there?
        let reach = d.at.width() * 1.5 + 4.0;
        if chars.iter().any(|&(c, x)| matches!(c, '-' | '~' | '_' | '=' | '—' | '–') && (x - cx).abs() < reach) {
            continue;
        }
        let at = chars.iter().position(|&(_, x)| x > cx).unwrap_or(chars.len());
        if d.spaced {
            chars.splice(at..at, [(' ', cx), ('-', cx), (' ', cx)]);
        } else {
            chars.insert(at, ('-', cx));
        }
    }
    let text: String = chars.into_iter().map(|(c, _)| c).collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Shrink a detected word box to the ink inside it; None if it has none
/// (a speck, or what was left of an erased line).
fn tighten(b: &prep::Binary, w: &Bounds, th: f32) -> Option<Bounds> {
    let (x0, y0) = (w.left.max(0.0) as usize, w.top.max(0.0) as usize);
    let (x1, y1) = ((w.right as usize).min(b.width), (w.bottom as usize).min(b.height));
    let (mut l, mut t, mut r, mut bo) = (usize::MAX, usize::MAX, 0, 0);
    let mut count = 0;
    for y in y0..y1 {
        let row = &b.ink[y * b.width + x0..y * b.width + x1.max(x0)];
        for (x, &v) in (x0..).zip(row) {
            if v != 0 {
                l = l.min(x);
                r = r.max(x);
                t = t.min(y);
                bo = bo.max(y);
                count += 1;
            }
        }
    }
    let min_ink = (th * th * 0.02).max(6.0) as usize;
    if count < min_ink || bo < t || (bo - t) as f32 > th * 4.0 {
        return None;
    }
    Some(Bounds { left: l as f32, top: t as f32, right: (r + 1) as f32, bottom: (bo + 1) as f32 })
}

/// Decide each cell's value. In columns that are mostly numbers, letters
/// that look like digits are read as digits.
fn typed(rows: usize, cols: usize, cells: Vec<(table::Cell, String)>) -> PageTable {
    let mut numeric = vec![(0usize, 0usize); cols];
    for (c, text) in &cells {
        if text.is_empty() || c.cols > 1 {
            continue;
        }
        numeric[c.col].1 += 1;
        if matches!(values::parse(text), Value::Number(..)) {
            numeric[c.col].0 += 1;
        }
    }
    let out = cells
        .into_iter()
        .map(|(c, text)| {
            let (nums, all) = numeric[c.col];
            let value = if text.is_empty() {
                None
            } else if c.cols == 1 && all >= 3 && nums * 10 >= all * 6 {
                Some(values::parse_numeric(&text))
            } else {
                Some(values::parse(&text))
            };
            TableCell { row: c.row, col: c.col, rows: c.rows, cols: c.cols, value, boxed: c.boxed }
        })
        .collect();
    PageTable { rows, cols, cells: out }
}

/// Whether a file's name says it is a scan this can read.
pub fn looks_like_scan(path: &std::path::Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "pdf" | "tif" | "tiff" | "jpg" | "jpeg" | "png")
}

/// The largest file accepted (a few hundred scanned pages).
const MAX_FILE: u64 = 400 << 20;
/// Pages read from one file at most.
pub const MAX_PAGES: usize = 200;

/// Read every page of a scan. `progress(done, total)` is called before each
/// page and once at the end.
pub fn read_file(
    path: &std::path::Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Result<Vec<PageTable>, ScanError> {
    let data = crate::common::fsutil::read_capped(path, MAX_FILE).map_err(|e| {
        ScanError::Failed(match e.kind() {
            std::io::ErrorKind::NotFound => "The file isn't there any more.".to_string(),
            std::io::ErrorKind::InvalidData => "The file is too large.".to_string(),
            _ => format!("The file couldn't be opened: {e}."),
        })
    })?;
    let doc = input::Document::open(data)?;
    let total = doc.page_count().min(MAX_PAGES);
    let mut pages = Vec::with_capacity(total);
    let mut images = 0;
    for i in 0..total {
        progress(i, total);
        check(cancel)?;
        match doc.page(i)? {
            Some(g) => {
                images += 1;
                pages.push(read_page(g, cancel)?);
            }
            None => pages.push(PageTable::default()),
        }
    }
    if images == 0 {
        return Err(ScanError::Failed(
            "This PDF has no scanned pages in it. It looks like it was made on a computer rather than scanned.".into(),
        ));
    }
    progress(total, total);
    Ok(pages)
}

/// The workbook for the pages, as bytes.
pub fn workbook(pages: &[PageTable], one_sheet: bool) -> Vec<u8> {
    xlsx::workbook(&sheets(pages, one_sheet))
}

/// Worksheets for the pages: one each, or all on one sheet, one after the
/// other with a blank row between pages.
pub fn sheets(pages: &[PageTable], one_sheet: bool) -> Vec<xlsx::Sheet> {
    let mut out: Vec<xlsx::Sheet> = Vec::new();
    let mut offset = 0;
    for (i, page) in pages.iter().enumerate() {
        if !one_sheet || out.is_empty() {
            let taken: Vec<String> = out.iter().map(|s| s.name.clone()).collect();
            let name = if one_sheet { "Scan".to_string() } else { format!("Page {}", i + 1) };
            out.push(xlsx::Sheet { name: xlsx::sheet_name(&name, &taken), ..Default::default() });
            offset = 0;
        }
        let sheet = out.last_mut().expect("just added");
        for c in &page.cells {
            let (r, col) = (c.row + offset, c.col);
            sheet.cells.push(xlsx::Cell { row: r, col, value: c.value.clone(), border: c.boxed });
            if c.rows > 1 || c.cols > 1 {
                sheet.merges.push((r, col, r + c.rows - 1, col + c.cols - 1));
                if c.boxed {
                    // Borders of a merged cell come from all the cells it covers.
                    for rr in r..r + c.rows {
                        for cc in col..col + c.cols {
                            if (rr, cc) != (r, col) {
                                sheet.cells.push(xlsx::Cell { row: rr, col: cc, value: None, border: true });
                            }
                        }
                    }
                }
            }
        }
        offset += page.rows + 1;
    }
    if out.is_empty() {
        out.push(xlsx::Sheet { name: "Scan".into(), ..Default::default() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_display_as_formatted() {
        assert_eq!(display_number(1608.8, "#,##0.00"), "1,608.80");
        assert_eq!(display_number(2.45, "0.00"), "2.45");
        assert_eq!(display_number(120.0, "General"), "120");
        assert_eq!(display_number(-45.0, "#,##0.00;(#,##0.00)"), "(45.00)");
        assert_eq!(display_number(0.125, "0.0%"), "12.5%");
        assert_eq!(display_number(1234567.0, "\"$\"#,##0"), "$1,234,567");
        assert_eq!(display_number(46297.0, "m/d/yyyy"), "10/2/2026");
    }

    #[test]
    fn pages_on_one_sheet_follow_each_other() {
        let cell = |row, col, t: &str| TableCell {
            row,
            col,
            rows: 1,
            cols: 1,
            value: Some(Value::Text(t.into())),
            boxed: false,
        };
        let p1 = PageTable { rows: 2, cols: 1, cells: vec![cell(0, 0, "a"), cell(1, 0, "b")] };
        let p2 = PageTable { rows: 1, cols: 1, cells: vec![cell(0, 0, "c")] };
        let one = sheets(&[p1.clone(), p2.clone()], true);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].cells.iter().map(|c| c.row).collect::<Vec<_>>(), vec![0, 1, 3]);
        let many = sheets(&[p1, p2], false);
        assert_eq!(many.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Page 1", "Page 2"]);
    }
}
