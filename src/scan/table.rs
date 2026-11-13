//! Working out a page's rows and columns from where its words are and the
//! ruled lines between them.
//!
//! Four kinds of table are recognised:
//! * ruled grids (lines between rows and columns), whose cells come straight
//!   from the lines, including cells that span several rows or columns;
//! * lined tables (lines between rows only), whose columns are worked out
//!   from how the words line up;
//! * printouts whose column headings are underlined with separate dashes or
//!   lines ("ORDER/LINE ROLL NO." over "----------- ------------"), as ERP
//!   and accounting systems print them: the underlines give the columns;
//! * tables without lines, whose columns come from how the words line up.
//!
//! Everything else on the page (titles, addresses, notes) becomes rows too,
//! with each phrase placed in the column it sits over.

use super::ocr::Bounds;
use super::prep::{Binary, Rule};

/// One cell of the output sheet and where its text is on the page.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
    /// Rows and columns covered (1 x 1 unless merged).
    pub rows: usize,
    pub cols: usize,
    /// The cell's lines of text, top to bottom, each to be read separately.
    pub lines: Vec<Bounds>,
    /// Part of a ruled grid: drawn with borders in the sheet.
    pub boxed: bool,
    /// Which table the cell belongs to (the page itself when it has no
    /// tables); None for text around tables. Tidying a column only looks
    /// at cells of one table.
    pub block: Option<usize>,
}

/// A page laid out as a sheet.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub cells: Vec<Cell>,
    pub rows: usize,
    pub cols: usize,
    /// The block cells are being added for.
    block: Option<usize>,
}

/// Lay out a page: `words` are tight boxes around the words, `h`/`v` the
/// ruled lines, `dashes` the dash-shaped marks on the page (which may be
/// column underlines), `th` the typical text height in pixels.
#[allow(clippy::too_many_arguments)]
pub fn layout(
    words: &[Bounds],
    h: &[Rule],
    v: &[Rule],
    dashes: &[Bounds],
    ink: Option<&Binary>,
    pitch: Option<f32>,
    th: f32,
) -> Layout {
    let tol = (th * 0.5).max(6.0);
    let (gap, free_gap) = word_gap(&text_lines(words), pitch, th);
    let mut tables = find_grids(h, v, tol, th);
    tables.extend(find_lined(h, &tables, tol, th));
    let (rulers, separators) = find_rulers(h, dashes, &tables, th);
    // Words on an underline row are the dashes themselves.
    let on_ruler = |w: &Bounds| {
        let (cx, cy) = w.center();
        rulers
            .iter()
            .chain(&separators)
            .any(|r| (cy - r.y).abs() < th * 0.5 && cx >= r.runs[0].0 - th && cx <= r.runs[r.runs.len() - 1].1 + th)
    };
    let words: Vec<Bounds> = words.iter().copied().filter(|w| !on_ruler(w)).collect();
    tables.extend(ruler_tables(&rulers, &text_lines(&words), &tables, th));
    tables.sort_by(|a, b| a.region.top.total_cmp(&b.region.top));

    // Words belong to the table they sit in.
    let mut table_words: Vec<Vec<Bounds>> = vec![Vec::new(); tables.len()];
    let mut free: Vec<Bounds> = Vec::new();
    for w in &words {
        let (cx, cy) = w.center();
        match tables.iter().position(|t| contains(&t.region, cx, cy)) {
            Some(i) => table_words[i].push(*w),
            None => free.push(*w),
        }
    }

    // Columns for lined tables, and for the text outside tables.
    for (t, tw) in tables.iter_mut().zip(&table_words) {
        if t.cuts.is_empty() && t.grid.is_none() {
            let lines = text_lines(tw);
            t.cuts = infer_cuts(&lines, th, gap);
        }
    }
    let free_lines = text_lines(&free);
    let page_cuts = if tables.is_empty() { infer_cuts(&free_lines, th, gap) } else { Vec::new() };

    // Blocks in reading order: each free line, and each table.
    enum Block<'a> {
        Line(&'a [Bounds]),
        Table(usize),
    }
    let mut blocks: Vec<(f32, Block)> = free_lines.iter().map(|l| (line_top(l), Block::Line(l))).collect();
    blocks.extend(tables.iter().enumerate().map(|(i, t)| (t.region.top, Block::Table(i))));
    blocks.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut out = Layout::default();
    for (_, block) in blocks {
        match block {
            Block::Line(words) => {
                out.block = if tables.is_empty() { Some(0) } else { None };
                // Use the columns of the nearest table, if any.
                let y = line_top(words);
                let nearest =
                    tables.iter().min_by(|a, b| distance_y(&a.region, y).total_cmp(&distance_y(&b.region, y)));
                let cuts = nearest.map_or(&page_cuts, |t| &t.cuts);
                let row = out.rows;
                // Text around a table keeps its phrases together.
                let g = if nearest.is_some() { free_gap } else { gap };
                let how = if nearest.is_some() { Place::AroundTable } else { Place::Columns };
                place_line(&mut out, row, words, cuts, th, g, how);
                out.rows += 1;
            }
            Block::Table(i) => {
                out.block = Some(i);
                let t = &tables[i];
                let first_row = out.rows;
                match &t.grid {
                    Some(grid) => place_grid(&mut out, first_row, grid, &table_words[i], th),
                    None if t.row_cuts.is_empty() => {
                        // Underlined columns: a row per line of text. A word
                        // the detector ran across a column edge is split
                        // where the ink has a gap ("DYELOT: 2638043").
                        for line in text_lines(&table_words[i]) {
                            let line: Vec<Bounds> = match ink {
                                Some(b) => line.iter().flat_map(|w| split_at_cuts(b, w, &t.cuts, th)).collect(),
                                None => line,
                            };
                            let row = out.rows;
                            place_line(&mut out, row, &line, &t.cuts, th, gap, Place::Strict);
                            out.rows += 1;
                        }
                    }
                    None => place_lined(&mut out, first_row, t, &table_words[i], th, gap),
                }
            }
        }
    }
    out.cols = out.cells.iter().map(|c| c.col + c.cols).max().unwrap_or(0);
    out
}

struct Table {
    region: Bounds,
    /// Column boundaries inside the table, left to right.
    cuts: Vec<f32>,
    /// For a ruled grid; None for a lined table.
    grid: Option<Grid>,
    /// For a lined table: row boundaries. Empty for underlined columns,
    /// whose rows are its lines of text.
    row_cuts: Vec<f32>,
}

/// A row of separate underlines (or one long line, a separator).
#[derive(Clone, Debug)]
struct Ruler {
    y: f32,
    /// Left to right.
    runs: Vec<(f32, f32)>,
}

struct Grid {
    xs: Vec<f32>,
    ys: Vec<f32>,
    /// Merged areas: (row, col, rows, cols).
    spans: Vec<(usize, usize, usize, usize)>,
}

fn contains(b: &Bounds, x: f32, y: f32) -> bool {
    x >= b.left && x <= b.right && y >= b.top && y <= b.bottom
}

fn distance_y(b: &Bounds, y: f32) -> f32 {
    if y < b.top {
        b.top - y
    } else if y > b.bottom {
        y - b.bottom
    } else {
        0.0
    }
}

fn line_top(words: &[Bounds]) -> f32 {
    words.iter().map(|w| w.top).fold(f32::MAX, f32::min)
}

/// Positions within `tol` of each other, averaged.
fn cluster(mut values: Vec<f32>, tol: f32) -> Vec<f32> {
    values.sort_by(f32::total_cmp);
    let mut out: Vec<(f32, usize)> = Vec::new();
    for v in values {
        match out.last_mut() {
            Some((sum, n)) if v - *sum / *n as f32 <= tol => {
                *sum += v;
                *n += 1;
            }
            _ => out.push((v, 1)),
        }
    }
    out.into_iter().map(|(s, n)| s / n as f32).collect()
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind((0..n).collect())
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

/// Ruled grids: groups of horizontal and vertical lines that cross.
fn find_grids(h: &[Rule], v: &[Rule], tol: f32, th: f32) -> Vec<Table> {
    let n = h.len() + v.len();
    let mut uf = UnionFind::new(n);
    for (i, hr) in h.iter().enumerate() {
        for (j, vr) in v.iter().enumerate() {
            let crosses = vr.pos >= hr.start - tol
                && vr.pos <= hr.end + tol
                && hr.pos >= vr.start - tol
                && hr.pos <= vr.end + tol;
            if crosses {
                uf.union(i, h.len() + j);
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, (Vec<Rule>, Vec<Rule>)> = Default::default();
    for i in 0..n {
        let root = uf.find(i);
        let entry = groups.entry(root).or_default();
        if i < h.len() {
            entry.0.push(h[i]);
        } else {
            entry.1.push(v[i - h.len()]);
        }
    }
    let mut out = Vec::new();
    for (_, (hs, vs)) in groups {
        if hs.len() < 2 || vs.len() < 2 {
            continue;
        }
        let left =
            hs.iter().map(|r| r.start).fold(f32::MAX, f32::min).min(vs.iter().map(|r| r.pos).fold(f32::MAX, f32::min));
        let right =
            hs.iter().map(|r| r.end).fold(f32::MIN, f32::max).max(vs.iter().map(|r| r.pos).fold(f32::MIN, f32::max));
        let top =
            vs.iter().map(|r| r.start).fold(f32::MAX, f32::min).min(hs.iter().map(|r| r.pos).fold(f32::MAX, f32::min));
        let bottom =
            vs.iter().map(|r| r.end).fold(f32::MIN, f32::max).max(hs.iter().map(|r| r.pos).fold(f32::MIN, f32::max));
        let snap = (th * 0.6).max(tol);
        let mut xs = cluster(vs.iter().map(|r| r.pos).collect(), snap);
        if xs[0] - left > snap {
            xs.insert(0, left);
        }
        if right - xs[xs.len() - 1] > snap {
            xs.push(right);
        }
        let mut ys = cluster(hs.iter().map(|r| r.pos).collect(), snap);
        if ys[0] - top > snap {
            ys.insert(0, top);
        }
        if bottom - ys[ys.len() - 1] > snap {
            ys.push(bottom);
        }
        if xs.len() < 2 || ys.len() < 2 {
            continue;
        }
        let spans = merged_cells(&xs, &ys, &hs, &vs, tol);
        let region = Bounds {
            left: xs[0] - tol,
            top: ys[0] - tol,
            right: xs[xs.len() - 1] + tol,
            bottom: ys[ys.len() - 1] + tol,
        };
        let cuts = xs[1..xs.len() - 1].to_vec();
        out.push(Table { region, cuts, grid: Some(Grid { xs, ys, spans }), row_cuts: Vec::new() });
    }
    out
}

/// How much of `from..to` the rules at `pos` (within `tol`) cover, 0..1.
fn coverage(rules: &[Rule], pos: f32, from: f32, to: f32, tol: f32) -> f32 {
    let (a, b) = (from + tol, to - tol);
    if b <= a {
        return 1.0;
    }
    let mut spans: Vec<(f32, f32)> = rules
        .iter()
        .filter(|r| (r.pos - pos).abs() <= tol)
        .map(|r| (r.start.max(a), r.end.min(b)))
        .filter(|(s, e)| e > s)
        .collect();
    spans.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut covered = 0.0;
    let mut reach = a;
    for (s, e) in spans {
        if e > reach {
            covered += e - s.max(reach);
            reach = e;
        }
    }
    covered / (b - a)
}

/// Cells of the grid that have no line between them are one merged cell.
fn merged_cells(xs: &[f32], ys: &[f32], hs: &[Rule], vs: &[Rule], tol: f32) -> Vec<(usize, usize, usize, usize)> {
    let (rows, cols) = (ys.len() - 1, xs.len() - 1);
    let mut uf = UnionFind::new(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            if c + 1 < cols && coverage(vs, xs[c + 1], ys[r], ys[r + 1], tol) < 0.6 {
                uf.union(r * cols + c, r * cols + c + 1);
            }
            if r + 1 < rows && coverage(hs, ys[r + 1], xs[c], xs[c + 1], tol) < 0.6 {
                uf.union(r * cols + c, (r + 1) * cols + c);
            }
        }
    }
    // Bounding rectangle of each merged group; overlapping rectangles join
    // (Excel can only merge rectangles).
    let mut rects: std::collections::BTreeMap<usize, (usize, usize, usize, usize)> = Default::default();
    for r in 0..rows {
        for c in 0..cols {
            let root = uf.find(r * cols + c);
            let e = rects.entry(root).or_insert((r, c, r, c));
            e.0 = e.0.min(r);
            e.1 = e.1.min(c);
            e.2 = e.2.max(r);
            e.3 = e.3.max(c);
        }
    }
    let mut list: Vec<(usize, usize, usize, usize)> = rects.into_values().collect();
    loop {
        let mut changed = false;
        'outer: for i in 0..list.len() {
            for j in i + 1..list.len() {
                let (a, b) = (list[i], list[j]);
                if a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3 {
                    list[i] = (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3));
                    list.remove(j);
                    changed = true;
                    break 'outer;
                }
            }
        }
        if !changed {
            break;
        }
    }
    list.into_iter()
        .filter(|&(r0, c0, r1, c1)| r1 > r0 || c1 > c0)
        .map(|(r0, c0, r1, c1)| (r0, c0, r1 - r0 + 1, c1 - c0 + 1))
        .collect()
}

/// Lined tables: three or more horizontal lines of about the same length,
/// one under the other, not part of a grid.
fn find_lined(h: &[Rule], grids: &[Table], tol: f32, th: f32) -> Vec<Table> {
    let mut free: Vec<Rule> = h
        .iter()
        .copied()
        .filter(|r| !grids.iter().any(|t| contains(&t.region, (r.start + r.end) / 2.0, r.pos)))
        .collect();
    free.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    let mut out = Vec::new();
    let mut i = 0;
    while i < free.len() {
        let mut j = i + 1;
        while j < free.len() {
            let (prev, next) = (free[j - 1], free[j]);
            let similar = (next.start - free[i].start).abs() < th * 3.0 && (next.end - free[i].end).abs() < th * 3.0;
            if !similar || next.pos - prev.pos > th * 8.0 || next.pos - prev.pos < th * 0.8 {
                break;
            }
            j += 1;
        }
        if j - i >= 3 {
            let set = &free[i..j];
            let region = Bounds {
                left: set.iter().map(|r| r.start).fold(f32::MAX, f32::min) - tol,
                right: set.iter().map(|r| r.end).fold(f32::MIN, f32::max) + tol,
                top: set[0].pos,
                bottom: set[set.len() - 1].pos,
            };
            let row_cuts = set.iter().map(|r| r.pos).collect();
            out.push(Table { region, cuts: Vec::new(), grid: None, row_cuts });
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// The gap between words that separates phrases (and so, possibly,
/// columns), for tables and for the text around them.
///
/// Ordinary type has narrow spaces, well under the text height. On a
/// typewriter-style page (characters `pitch` apart) a single space leaves a
/// gap of one to two characters' width and columns are two or more spaces
/// apart; headings are often spaced out ("B I L L  O F  L A D I N G",
/// "PAGE:  1"), so text around tables only splits at three or more spaces.
fn word_gap(lines: &[Vec<Bounds>], pitch: Option<f32>, th: f32) -> (f32, f32) {
    let ordinary = (0.8 * th, 0.8 * th);
    let Some(p) = pitch else { return ordinary };
    let mut single: Vec<f32> = lines
        .iter()
        .flat_map(|l| l.windows(2).map(|w| w[1].left - w[0].right))
        .filter(|&g| g >= p * 0.8 && g <= p * 1.85)
        .collect();
    if single.len() < 5 {
        return ordinary;
    }
    single.sort_by(f32::total_cmp);
    let space = single[single.len() / 2];
    (space + 0.5 * p, space + 1.5 * p)
}

/// Split a word box that crosses column edges at a clear gap in its ink
/// near each edge; boxes are tightened to their ink.
fn split_at_cuts(ink: &Binary, w: &Bounds, cuts: &[f32], th: f32) -> Vec<Bounds> {
    let mut pieces = vec![*w];
    for &cut in cuts {
        let Some(last) = pieces.last().copied() else { break };
        if cut - last.left < th * 0.3 || last.right - cut < th * 0.3 {
            continue;
        }
        let (y0, y1) = (last.top.max(0.0) as usize, (last.bottom as usize).min(ink.height));
        let blank = |x: usize| (y0..y1).all(|y| !ink.at(x, y));
        // The blank column nearest the edge, within a character or so.
        let reach = th as usize + 2;
        let c = cut.round().max(0.0) as usize;
        let found = (0..=reach).find_map(|d| {
            [c.checked_sub(d), Some(c + d)]
                .into_iter()
                .flatten()
                .find(|&x| (x as f32) > last.left + 1.0 && (x as f32) < last.right - 1.0 && x < ink.width && blank(x))
        });
        let Some(x) = found else { continue };
        let left = tighten(ink, &Bounds { right: x as f32, ..last });
        let right = tighten(ink, &Bounds { left: x as f32 + 1.0, ..last });
        if let (Some(l), Some(r)) = (left, right) {
            pieces.pop();
            pieces.push(l);
            pieces.push(r);
        }
    }
    pieces
}

/// The box around the ink inside `b`, if any.
fn tighten(ink: &Binary, b: &Bounds) -> Option<Bounds> {
    let (x0, y0) = (b.left.max(0.0) as usize, b.top.max(0.0) as usize);
    let (x1, y1) = ((b.right.max(0.0) as usize).min(ink.width), (b.bottom.max(0.0) as usize).min(ink.height));
    let mut out: Option<Bounds> = None;
    for y in y0..y1 {
        for x in x0..x1 {
            if ink.at(x, y) {
                let p = Bounds { left: x as f32, top: y as f32, right: x as f32 + 1.0, bottom: y as f32 + 1.0 };
                out = Some(out.map_or(p, |o| o.union(&p)));
            }
        }
    }
    out
}

/// Rows of separate underlines (column rulers) and single long lines
/// (separators), from ruled lines not part of a table and from dashes.
fn find_rulers(h: &[Rule], dashes: &[Bounds], tables: &[Table], th: f32) -> (Vec<Ruler>, Vec<Ruler>) {
    let mut segments: Vec<(f32, f32, f32)> = h
        .iter()
        .filter(|r| !tables.iter().any(|t| contains(&t.region, (r.start + r.end) / 2.0, r.pos)))
        .map(|r| (r.pos, r.start, r.end))
        .collect();
    // Dashes in a row ("-----"), each printed separately.
    let mut marks: Vec<Bounds> = dashes.to_vec();
    marks.sort_by(|a, b| a.center().1.total_cmp(&b.center().1));
    let mut rows: Vec<Vec<Bounds>> = Vec::new();
    for m in marks {
        match rows.last_mut() {
            Some(r) if (m.center().1 - r[0].center().1).abs() <= th * 0.3 => r.push(m),
            _ => rows.push(vec![m]),
        }
    }
    for mut row in rows {
        row.sort_by(|a, b| a.left.total_cmp(&b.left));
        let mut run: Vec<Bounds> = Vec::new();
        let flush = |run: &mut Vec<Bounds>, segments: &mut Vec<(f32, f32, f32)>| {
            if run.len() >= 3 {
                let y = run.iter().map(|b| b.center().1).sum::<f32>() / run.len() as f32;
                segments.push((y, run[0].left, run[run.len() - 1].right));
            }
            run.clear();
        };
        for m in row {
            if run.last().is_some_and(|l| m.left - l.right > th * 0.6) {
                flush(&mut run, &mut segments);
            }
            run.push(m);
        }
        flush(&mut run, &mut segments);
    }
    // Segments at the same height make a row.
    segments.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut groups: Vec<Vec<(f32, f32, f32)>> = Vec::new();
    for s in segments {
        match groups.last_mut() {
            Some(g) if (s.0 - g[0].0).abs() <= th * 0.4 => g.push(s),
            _ => groups.push(vec![s]),
        }
    }
    let (mut rulers, mut separators) = (Vec::new(), Vec::new());
    for mut g in groups {
        g.sort_by(|a, b| a.1.total_cmp(&b.1));
        let y = g.iter().map(|s| s.0).sum::<f32>() / g.len() as f32;
        let runs: Vec<(f32, f32)> = g.iter().map(|s| (s.1, s.2)).filter(|(a, b)| b - a >= th * 1.2).collect();
        let apart = runs.windows(2).all(|w| w[1].0 - w[0].1 >= th * 0.3);
        if runs.len() >= 2 && apart {
            rulers.push(Ruler { y, runs });
        } else if runs.len() == 1 && runs[0].1 - runs[0].0 >= th * 10.0 {
            separators.push(Ruler { y, runs });
        }
    }
    (rulers, separators)
}

/// A table for each column ruler that has a heading line just above it and
/// lines of text below. It runs from the heading down to the next table (or
/// the end of the page); its columns are split halfway between underlines.
fn ruler_tables(rulers: &[Ruler], lines: &[Vec<Bounds>], tables: &[Table], th: f32) -> Vec<Table> {
    let mut out: Vec<Table> = Vec::new();
    for (i, r) in rulers.iter().enumerate() {
        let heading = lines.iter().filter(|l| {
            let bottom = l.iter().map(|w| w.bottom).fold(f32::MIN, f32::max);
            bottom <= r.y && r.y - bottom <= th * 3.0
        });
        let Some(top) = heading.map(|l| line_top(l)).min_by(f32::total_cmp) else { continue };
        let below = lines.iter().filter(|l| line_top(l) > r.y && line_top(l) - r.y < th * 8.0).count();
        if below == 0 {
            continue;
        }
        let next = rulers[i + 1..]
            .iter()
            .map(|n| n.y - th * 2.5)
            .chain(tables.iter().map(|t| t.region.top).filter(|&t| t > r.y))
            .fold(f32::MAX, f32::min);
        let cuts = r.runs.windows(2).map(|w| (w[0].1 + w[1].0) / 2.0).collect();
        out.push(Table {
            region: Bounds { left: f32::MIN, top, right: f32::MAX, bottom: next },
            cuts,
            grid: None,
            row_cuts: Vec::new(),
        });
    }
    out
}

/// Group words into lines of text, top to bottom, each left to right.
pub fn text_lines(words: &[Bounds]) -> Vec<Vec<Bounds>> {
    let mut sorted: Vec<Bounds> = words.to_vec();
    sorted.sort_by(|a, b| a.top.total_cmp(&b.top));
    // Each line keeps the average top and bottom of its words, so one tall
    // word does not make it reach into the next line.
    struct Line {
        top: f32,
        bottom: f32,
        words: Vec<Bounds>,
    }
    let mut lines: Vec<Line> = Vec::new();
    for w in sorted {
        // Join the line this word overlaps most, by at least half its height.
        let best = lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let overlap = w.bottom.min(l.bottom) - w.top.max(l.top);
                (i, overlap / w.height().min(l.bottom - l.top).max(1.0))
            })
            .filter(|&(_, f)| f >= 0.5)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, _)) => {
                let l = &mut lines[i];
                let n = l.words.len() as f32;
                l.top = (l.top * n + w.top) / (n + 1.0);
                l.bottom = (l.bottom * n + w.bottom) / (n + 1.0);
                l.words.push(w);
            }
            None => lines.push(Line { top: w.top, bottom: w.bottom, words: vec![w] }),
        }
    }
    let mut out: Vec<Vec<Bounds>> = lines
        .into_iter()
        .map(|l| {
            let mut ws = l.words;
            ws.sort_by(|a, b| a.left.total_cmp(&b.left));
            ws
        })
        .collect();
    out.sort_by(|a, b| line_top(a).total_cmp(&line_top(b)));
    out
}

/// Split a line into phrases: runs of words with ordinary spaces between.
fn phrases(line: &[Bounds], gap: f32) -> Vec<Vec<Bounds>> {
    // Spaces grow with the type: judge gaps by this line's own text size
    // (a large title) but never below the page's.
    let mut heights: Vec<f32> = line.iter().map(Bounds::height).collect();
    heights.sort_by(f32::total_cmp);
    let line_h = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
    let gap = gap.max(0.64 * line_h);
    let mut out: Vec<Vec<Bounds>> = Vec::new();
    for w in line {
        match out.last_mut() {
            Some(p) if w.left - p[p.len() - 1].right < gap => p.push(*w),
            _ => out.push(vec![*w]),
        }
    }
    out
}

fn span(words: &[Bounds]) -> Bounds {
    words.iter().skip(1).fold(words[0], |a, w| a.union(w))
}

/// Column boundaries for lines of text without ruled columns: the gaps that
/// run down through (nearly) all the lines with more than one phrase.
pub fn infer_cuts(lines: &[Vec<Bounds>], th: f32, gap: f32) -> Vec<f32> {
    let multi: Vec<Vec<Bounds>> = lines
        .iter()
        .map(|l| phrases(l, gap).iter().map(|p| span(p)).collect::<Vec<_>>())
        .filter(|p: &Vec<Bounds>| p.len() >= 2)
        .collect();
    if multi.len() < 2 {
        return Vec::new();
    }
    let left = multi.iter().flatten().map(|p| p.left).fold(f32::MAX, f32::min).max(0.0);
    let right = multi.iter().flatten().map(|p| p.right).fold(f32::MIN, f32::max);
    let bin = (th / 4.0).max(1.0);
    let bins = ((right - left) / bin) as usize + 2;
    let mut cover = vec![0usize; bins];
    for line in &multi {
        let mut hit = vec![false; bins];
        for p in line {
            let (a, b) = (((p.left - left) / bin) as usize, ((p.right - left) / bin) as usize);
            for h in hit.iter_mut().take(b.min(bins - 1) + 1).skip(a) {
                *h = true;
            }
        }
        for (c, h) in cover.iter_mut().zip(hit) {
            *c += usize::from(h);
        }
    }
    // A few lines may bridge a gap (a long title, say): allow up to 10%.
    let allowed = multi.len() / 10;
    let min_gap = ((th * 0.5) / bin).ceil() as usize;
    let mut cuts = Vec::new();
    let mut i = 0;
    while i < bins {
        if cover[i] > allowed {
            i += 1;
            continue;
        }
        let start = i;
        while i < bins && cover[i] <= allowed {
            i += 1;
        }
        // Only gaps between text, not the margins.
        if start > 0 && i < bins && i - start >= min_gap {
            // Cut where the fewest lines cross, in the middle of that.
            let lowest = cover[start..i].iter().copied().min().unwrap_or(0);
            let first = start + cover[start..i].iter().position(|&c| c == lowest).unwrap_or(0);
            let last = start + cover[start..i].iter().rposition(|&c| c == lowest).unwrap_or(0);
            cuts.push(left + (first + last + 1) as f32 * bin / 2.0);
        }
    }
    merge_sparse_columns(&multi, cuts)
}

/// A heading printed to the left of right-aligned numbers ("Credit" over
/// "64.10") can leave a gap of its own: a column with hardly anything in
/// it that never shares a line with its neighbour is part of that
/// neighbour.
fn merge_sparse_columns(lines: &[Vec<Bounds>], mut cuts: Vec<f32>) -> Vec<f32> {
    loop {
        let n = cuts.len() + 1;
        let mut used: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut extent: Vec<(f32, f32)> = vec![(f32::MAX, f32::MIN); n];
        for (li, line) in lines.iter().enumerate() {
            for p in line {
                let c = column_of(&cuts, (p.left + p.right) / 2.0);
                if used[c].last() != Some(&li) {
                    used[c].push(li);
                }
                extent[c] = (extent[c].0.min(p.left), extent[c].1.max(p.right));
            }
        }
        let sparse = (lines.len() * 15 / 100).max(1);
        let mut best: Option<(usize, f32)> = None;
        for c in 0..n {
            if used[c].is_empty() || used[c].len() > sparse {
                continue;
            }
            for other in [c.wrapping_sub(1), c + 1] {
                if other >= n || used[other].iter().any(|l| used[c].contains(l)) || used[other].is_empty() {
                    continue;
                }
                let gap = if other < c { extent[c].0 - extent[other].1 } else { extent[other].0 - extent[c].1 };
                let cut = c.min(other);
                if best.is_none_or(|(_, g)| gap < g) {
                    best = Some((cut, gap));
                }
            }
        }
        match best {
            Some((cut, _)) => {
                cuts.remove(cut);
            }
            None => return cuts,
        }
    }
}

fn column_of(cuts: &[f32], x: f32) -> usize {
    cuts.iter().filter(|&&c| c <= x).count()
}

/// The column a phrase belongs to: the one it starts in, unless only a
/// sliver of it is there (a wide number reaching left out of its column),
/// then the one it overlaps most.
fn phrase_column(cuts: &[f32], p: &Bounds, th: f32) -> usize {
    let start = column_of(cuts, p.left);
    let end = column_of(cuts, p.right);
    if start == end {
        return start;
    }
    let edge = |i: usize| -> (f32, f32) {
        (if i == 0 { f32::MIN } else { cuts[i - 1] }, if i == cuts.len() { f32::MAX } else { cuts[i] })
    };
    // Where it starts, unless it only just starts there (a wide number
    // reaching left out of its column).
    let (a, b) = edge(start);
    let in_start = p.right.min(b) - p.left.max(a);
    if in_start >= p.width() * 0.3 || in_start >= th * 1.5 {
        return start;
    }
    (start..=end)
        .map(|i| {
            let (a, b) = edge(i);
            (i, p.right.min(b) - p.left.max(a))
        })
        .max_by(|x, y| x.1.total_cmp(&y.1))
        .map_or(start, |(i, _)| i)
}

/// Recognition boxes for the words of one cell: one per line of text.
fn cell_lines(words: &[Bounds]) -> Vec<Bounds> {
    text_lines(words).iter().map(|l| span(l)).collect()
}

fn push_cell(out: &mut Layout, row: usize, col: usize, words: &[Bounds], boxed: bool) {
    if words.is_empty() {
        return;
    }
    // Two phrases in one cell (they fell in the same column): add to it.
    if let Some(c) = out.cells.iter_mut().find(|c| c.row == row && c.col == col) {
        let mut all: Vec<Bounds> = c.lines.clone();
        all.extend_from_slice(words);
        c.lines = cell_lines(&all);
        return;
    }
    out.cells.push(Cell { row, col, rows: 1, cols: 1, lines: cell_lines(words), boxed, block: out.block });
}

/// A line of text outside any table.
/// How a line's phrases go into columns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Text around a table: phrases that fall in the same column move along
    /// to the next free one (a heading line such as "Invoice 20517   Date
    /// 10/02/2026   PO 4471-B" above a table); a lone phrase stays whole.
    AroundTable,
    /// A row of a table without lines; a lone phrase (a title) stays whole.
    Columns,
    /// A row of a table with underlined columns: every column edge splits.
    Strict,
}

#[allow(clippy::too_many_arguments)]
fn place_line(out: &mut Layout, row: usize, words: &[Bounds], cuts: &[f32], th: f32, gap: f32, how: Place) {
    let spread = how == Place::AroundTable;
    let ps = phrases(words, gap);
    if ps.len() == 1 && how != Place::Strict {
        let col = phrase_column(cuts, &span(&ps[0]), th);
        push_cell(out, row, col, &ps[0], false);
        return;
    }
    let mut by_col: std::collections::BTreeMap<usize, Vec<Bounds>> = Default::default();
    let mut next_free = 0;
    for p in &ps {
        let s = span(p);
        let whole = column_of(cuts, s.left) == column_of(cuts, s.right);
        if whole || spread {
            let mut col = phrase_column(cuts, &s, th);
            if spread {
                col = col.max(next_free);
                next_free = col + 1;
            }
            by_col.entry(col).or_default().extend_from_slice(p);
        } else {
            // A phrase across columns: word by word, so the columns split it.
            for w in p {
                by_col.entry(column_of(cuts, w.center().0)).or_default().push(*w);
            }
        }
    }
    for (col, ws) in by_col {
        push_cell(out, row, col, &ws, false);
    }
}

fn place_grid(out: &mut Layout, first_row: usize, grid: &Grid, words: &[Bounds], th: f32) {
    let (rows, cols) = (grid.ys.len() - 1, grid.xs.len() - 1);
    // Which merged area (if any) each grid cell belongs to.
    let owner = |r: usize, c: usize| -> (usize, usize, usize, usize) {
        grid.spans
            .iter()
            .copied()
            .find(|&(r0, c0, nr, nc)| r >= r0 && r < r0 + nr && c >= c0 && c < c0 + nc)
            .unwrap_or((r, c, 1, 1))
    };
    let mut contents: std::collections::BTreeMap<(usize, usize), Vec<Bounds>> = Default::default();
    for w in words {
        let (cx, cy) = w.center();
        let c = column_of(&grid.xs[1..cols], cx);
        let r = column_of(&grid.ys[1..rows], cy);
        let (r0, c0, nr, nc) = owner(r, c);
        // Keep each word inside its cell (text can touch a line).
        let cell = Bounds { left: grid.xs[c0], top: grid.ys[r0], right: grid.xs[c0 + nc], bottom: grid.ys[r0 + nr] };
        let pad = th * 0.1;
        let clipped = Bounds {
            left: w.left.max(cell.left + pad),
            top: w.top.max(cell.top + pad),
            right: w.right.min(cell.right - pad),
            bottom: w.bottom.min(cell.bottom - pad),
        };
        if clipped.width() > 1.0 && clipped.height() > 1.0 {
            contents.entry((r0, c0)).or_default().push(clipped);
        }
    }
    for r in 0..rows {
        for c in 0..cols {
            let (r0, c0, nr, nc) = owner(r, c);
            if (r0, c0) != (r, c) {
                continue;
            }
            let lines = contents.get(&(r, c)).map(|ws| cell_lines(ws)).unwrap_or_default();
            out.cells.push(Cell {
                row: first_row + r,
                col: c,
                rows: nr,
                cols: nc,
                lines,
                boxed: true,
                block: out.block,
            });
        }
    }
    out.rows = first_row + rows;
}

fn place_lined(out: &mut Layout, first_row: usize, t: &Table, words: &[Bounds], th: f32, gap: f32) {
    // Rows between the lines; text above the first or below the last line
    // within the region is not expected (the region ends at the lines).
    let bands = t.row_cuts.len().saturating_sub(1);
    let mut rows: Vec<Vec<Bounds>> = vec![Vec::new(); bands];
    for w in words {
        let r = column_of(&t.row_cuts[1..t.row_cuts.len() - 1], w.center().1);
        rows[r.min(bands - 1)].push(*w);
    }
    let mut row = first_row;
    for ws in rows {
        if ws.is_empty() {
            continue;
        }
        let mut by_col: std::collections::BTreeMap<usize, Vec<Bounds>> = Default::default();
        for line in text_lines(&ws) {
            for p in phrases(&line, gap) {
                let whole = column_of(&t.cuts, p[0].left) == column_of(&t.cuts, p[p.len() - 1].right);
                for w in &p {
                    let c =
                        if whole { phrase_column(&t.cuts, &span(&p), th) } else { column_of(&t.cuts, w.center().0) };
                    by_col.entry(c).or_default().push(*w);
                }
            }
        }
        for (col, ws) in by_col {
            push_cell(out, row, col, &ws, false);
        }
        row += 1;
    }
    out.rows = row;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(l: f32, t: f32, r: f32, bo: f32) -> Bounds {
        Bounds { left: l, top: t, right: r, bottom: bo }
    }
    fn rule(pos: f32, start: f32, end: f32) -> Rule {
        Rule { pos, start, end, thickness: 3.0 }
    }

    #[test]
    fn ruled_grid_with_a_merged_header() {
        // 3 columns x 3 rows; the first row has no line between columns 1 and 2.
        let h: Vec<Rule> = [100.0, 200.0, 300.0, 400.0].iter().map(|&y| rule(y, 100.0, 700.0)).collect();
        let v = vec![
            rule(100.0, 100.0, 400.0),
            rule(300.0, 100.0, 400.0),
            rule(500.0, 200.0, 400.0),
            rule(700.0, 100.0, 400.0),
        ];
        let words = vec![
            b(120.0, 140.0, 200.0, 170.0),
            b(320.0, 140.0, 420.0, 170.0),
            b(120.0, 240.0, 200.0, 270.0),
            b(620.0, 340.0, 690.0, 370.0),
        ];
        let l = layout(&words, &h, &v, &[], None, None, 30.0);
        assert_eq!((l.rows, l.cols), (3, 3));
        let header = l.cells.iter().find(|c| c.row == 0 && c.col == 1).unwrap();
        assert_eq!((header.rows, header.cols), (1, 2));
        assert_eq!(header.lines.len(), 1);
        assert!(l.cells.iter().all(|c| c.boxed));
        assert!(!l.cells.iter().any(|c| c.row == 0 && c.col == 2));
        let last = l.cells.iter().find(|c| c.row == 2 && c.col == 2).unwrap();
        assert_eq!(last.lines.len(), 1);
    }

    #[test]
    fn columns_from_alignment() {
        // Three columns: left-aligned text, right-aligned numbers.
        let mut words = Vec::new();
        for i in 0..6 {
            let y = 100.0 + i as f32 * 50.0;
            words.push(b(100.0, y, 180.0, y + 30.0));
            words.push(b(190.0, y, 260.0 + (i % 3) as f32 * 40.0, y + 30.0));
            let w = 60.0 + (i % 2) as f32 * 40.0;
            words.push(b(700.0 - w, y, 700.0, y + 30.0));
            words.push(b(900.0 - w, y, 900.0, y + 30.0));
        }
        // A title across the top.
        words.push(b(100.0, 20.0, 600.0, 60.0));
        let l = layout(&words, &[], &[], &[], None, None, 30.0);
        assert_eq!(l.cols, 3, "{:?}", l.cells);
        assert_eq!(l.rows, 7);
        let title = l.cells.iter().find(|c| c.row == 0).unwrap();
        assert_eq!(title.col, 0);
        for r in 1..7 {
            let row: Vec<usize> = l.cells.iter().filter(|c| c.row == r).map(|c| c.col).collect();
            assert_eq!(row, vec![0, 1, 2]);
        }
    }
}
