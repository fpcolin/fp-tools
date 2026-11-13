//! Tidying a page's columns once their text has been read.
//!
//! * A column whose every row holds the same kinds of things in the same
//!   order ("12'00 x 179'02": a number, a letter, a number) is split into
//!   one column for each, as a person would want it in a spreadsheet.
//! * In a column of values that share a shape (feet and inches such as
//!   179'02, or 12/31), a value read with the mark wrong or missing is
//!   corrected to that shape: 12100 or 12"00 becomes 12'00 when every other
//!   value in the column looks like 12'00. Where a mark could have been
//!   misread or dropped (18105: 18'05 or 181'05?), the width of the printed
//!   value decides.
//! * A column of one word in different capitals ("x", "X") takes the most
//!   common spelling.

use super::table;

/// A cell and what was read in it.
#[derive(Clone, Debug)]
pub struct Read {
    pub cell: table::Cell,
    pub text: String,
    /// For a cell with one line of text: each character and where it starts
    /// on the page (spaces included), and the left and right edge of the ink.
    pub chars: Vec<(char, f32)>,
    pub span: Option<(f32, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Number,
    Word,
    Mixed,
}

/// A run of tokens of one kind, with where it is.
struct Group {
    kind: Kind,
    /// Character range in `Read::chars`.
    from: usize,
    to: usize,
}

fn kind_of(token: &str) -> Option<Kind> {
    let digits = token.chars().any(|c| c.is_ascii_digit());
    let letters = token.chars().any(char::is_alphabetic);
    match (digits, letters) {
        (true, true) => Some(Kind::Mixed),
        (true, false) => Some(Kind::Number),
        (false, true) => Some(Kind::Word),
        // Punctuation on its own ("-", "&") goes with its neighbours.
        (false, false) => None,
    }
}

/// The cell's text in groups of same-kind tokens.
fn groups(r: &Read) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    let chars = &r.chars;
    let mut i = 0;
    while i < chars.len() {
        if chars[i].0 == ' ' {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].0 != ' ' {
            i += 1;
        }
        let token: String = chars[start..i].iter().map(|c| c.0).collect();
        match (kind_of(&token), out.last_mut()) {
            (Some(k), Some(g)) if g.kind == k => g.to = i,
            (None, Some(g)) => g.to = i,
            (Some(k), _) => out.push(Group { kind: k, from: start, to: i }),
            (None, None) => out.push(Group { kind: Kind::Word, from: start, to: i }),
        }
    }
    out
}

fn signature(r: &Read) -> Vec<Kind> {
    groups(r).iter().map(|g| g.kind).collect()
}

fn words_in(chars: &[(char, f32)]) -> usize {
    chars.iter().map(|c| c.0).collect::<String>().split_whitespace().count()
}

/// Split a cell's words into parts of the given sizes (None if the total
/// differs).
fn by_word_counts(r: &Read, counts: &[usize]) -> Option<Vec<Group>> {
    let mut starts: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < r.chars.len() {
        if r.chars[i].0 == ' ' {
            i += 1;
            continue;
        }
        let start = i;
        while i < r.chars.len() && r.chars[i].0 != ' ' {
            i += 1;
        }
        starts.push((start, i));
    }
    if starts.len() != counts.iter().sum::<usize>() {
        return None;
    }
    let mut out = Vec::new();
    let mut k = 0;
    for &n in counts {
        out.push(Group { kind: Kind::Word, from: starts[k].0, to: starts[k + n - 1].1 });
        k += n;
    }
    Some(out)
}

/// The tables (blocks) on the page.
fn blocks(cells: &[Read]) -> Vec<usize> {
    let mut b: Vec<usize> = cells.iter().filter_map(|r| r.cell.block).collect();
    b.sort_unstable();
    b.dedup();
    b
}

/// One table's column of single cells (not merged ones), optionally
/// leaving out a ruled grid's.
fn column(cells: &[Read], col: usize, block: usize, grids_too: bool) -> Vec<usize> {
    (0..cells.len())
        .filter(|&i| {
            let c = &cells[i].cell;
            c.col == col
                && c.block == Some(block)
                && c.cols == 1
                && (grids_too || !c.boxed)
                && !cells[i].text.is_empty()
        })
        .collect()
}

/// Split columns whose rows all hold the same kinds of things.
pub fn split(cells: &mut Vec<Read>, cols: &mut usize) {
    for block in blocks(cells) {
        split_block(cells, cols, block);
    }
}

fn split_block(cells: &mut Vec<Read>, cols: &mut usize, block: usize) {
    for col in (0..*cols).rev() {
        let members = column(cells, col, block, false);
        if members.len() < 3 || members.iter().any(|&i| cells[i].chars.is_empty()) {
            continue;
        }
        let sigs: Vec<Vec<Kind>> = members.iter().map(|&i| signature(&cells[i])).collect();
        let mut best: Option<(&Vec<Kind>, usize)> = None;
        for s in &sigs {
            let n = sigs.iter().filter(|o| *o == s).count();
            if best.is_none_or(|(_, m)| n > m) {
                best = Some((s, n));
            }
        }
        let Some((sig, n)) = best else { continue };
        let k = sig.len();
        if k < 2 || n < 3 || n * 10 < members.len() * 6 {
            continue;
        }
        let sig = sig.clone();
        // How many words each part has in (nearly) every row: a fixed
        // format, unlike "Invoice 20431" and "Credit memo 118". A row with
        // that many words is split the same way even if one was misread
        // ("12'00 ? 170'08" for "12'00 x 170'08").
        let counts: Vec<Vec<usize>> = members
            .iter()
            .zip(&sigs)
            .filter(|(_, s)| **s == sig)
            .map(|(&i, _)| groups(&cells[i]).iter().map(|g| words_in(&cells[i].chars[g.from..g.to])).collect())
            .collect();
        let mut usual: Option<(&Vec<usize>, usize)> = None;
        for c in &counts {
            let m = counts.iter().filter(|o| *o == c).count();
            if usual.is_none_or(|(_, u)| m > u) {
                usual = Some((c, m));
            }
        }
        let Some((usual, m)) = usual else { continue };
        if m * 10 < counts.len() * 8 {
            continue;
        }
        let words_per_part = Some(usual.clone());
        // Make room: later columns move right, merged cells across this one grow.
        for r in cells.iter_mut() {
            let c = &mut r.cell;
            if c.col > col {
                c.col += k - 1;
            } else if c.cols > 1 && c.col <= col && col < c.col + c.cols {
                c.cols += k - 1;
            }
        }
        let mut added = Vec::new();
        for (&i, s) in members.iter().zip(&sigs) {
            let r = cells[i].clone();
            let gs = if *s == sig {
                groups(&r)
            } else {
                match words_per_part.as_ref().and_then(|w| by_word_counts(&r, w)) {
                    Some(gs) => gs,
                    None => continue,
                }
            };
            let last = gs.len() - 1;
            for (j, g) in gs.iter().enumerate() {
                let chars: Vec<(char, f32)> = r.chars[g.from..g.to].to_vec();
                let text: String = chars.iter().map(|c| c.0).collect();
                let (line_left, line_right) = r.span.unwrap_or((0.0, 0.0));
                let left = if j == 0 { line_left } else { r.chars[g.from].1 };
                let right = if j == last { line_right } else { r.chars[g.to].1 };
                let mut cell = r.cell.clone();
                cell.col += j;
                let piece = Read { cell, text, chars, span: Some((left, right)) };
                if j == 0 {
                    cells[i] = piece;
                } else {
                    added.push(piece);
                }
            }
        }
        cells.extend(added);
        *cols += k - 1;
    }
}

/// Letters, digits and other characters, with runs of digits and of
/// letters each shown once: "179'02" -> "9'9", "12 x 4" -> "9 a 9".
fn shape(t: &str) -> String {
    let mut out = String::new();
    for ch in t.chars() {
        let c = if ch.is_ascii_digit() {
            '9'
        } else if ch.is_alphabetic() {
            'a'
        } else {
            ch
        };
        if (c == '9' || c == 'a') && out.ends_with(c) {
            continue;
        }
        out.push(c);
    }
    out
}

/// Characters an apostrophe or similar mark is commonly read as.
const MARK_LOOKALIKES: &str = "\"'`´‘’?|!:;*^~°";

/// Correct values to the shape most of their column shares.
pub fn repair(cells: &mut [Read]) {
    let cols = cells.iter().map(|r| r.cell.col + 1).max().unwrap_or(0);
    for (block, col) in blocks(cells).into_iter().flat_map(|b| (0..cols).map(move |c| (b, c))) {
        let members = column(cells, col, block, true);
        if members.len() < 3 {
            continue;
        }
        normalize_case(cells, &members);
        // The usual shape: digits, one mark, digits ("9'9").
        let shapes: Vec<String> = members.iter().map(|&i| shape(&cells[i].text)).collect();
        let mut best: Option<(&String, usize)> = None;
        for s in &shapes {
            let n = shapes.iter().filter(|o| *o == s).count();
            if best.is_none_or(|(_, m)| n > m) {
                best = Some((s, n));
            }
        }
        let Some((dominant, n)) = best else { continue };
        let d: Vec<char> = dominant.chars().collect();
        // Decimal points and thousands commas are numbers' business.
        if d.len() != 3 || d[0] != '9' || d[2] != '9' || d[1].is_alphanumeric() || " .,".contains(d[1]) {
            continue;
        }
        if n < 3 || n * 10 < members.len() * 6 {
            continue;
        }
        let mark = d[1];
        let good: Vec<usize> = members.iter().zip(&shapes).filter(|(_, s)| *s == dominant).map(|(&i, _)| i).collect();
        let parts = |t: &str| -> Option<(usize, usize)> {
            let (a, b) = t.split_once(mark)?;
            Some((a.chars().count(), b.chars().count()))
        };
        let lens: Vec<(usize, usize)> = good.iter().filter_map(|&i| parts(&cells[i].text)).collect();
        // The leading number varies (5'06, 120'00); the trailing one is
        // usually fixed (inches as two digits), so keep it to what the
        // column shows.
        let (min_a, max_a) = (1, lens.iter().map(|l| l.0).max().unwrap_or(8) + 1);
        let (min_b, max_b) = (lens.iter().map(|l| l.1).min().unwrap_or(1), lens.iter().map(|l| l.1).max().unwrap_or(9));
        let fits = |t: &str| -> bool {
            if t.matches(mark).count() != 1 {
                return false;
            }
            match parts(t) {
                Some((a, b)) => {
                    (min_a..=max_a).contains(&a)
                        && (min_b..=max_b).contains(&b)
                        && t.chars().filter(|&c| c != mark).all(|c| c.is_ascii_digit())
                }
                None => false,
            }
        };
        // Printed width per character, from the good values.
        let widths: Vec<(f32, f32)> = good
            .iter()
            .filter_map(|&i| cells[i].span.map(|(l, r)| (cells[i].text.chars().count() as f32, r - l)))
            .collect();
        let per_char = if widths.is_empty() {
            None
        } else {
            let (n, w): (f32, f32) = widths.iter().fold((0.0, 0.0), |a, x| (a.0 + x.0, a.1 + x.1));
            Some(w / n)
        };
        for &i in &members {
            if fits(&cells[i].text) {
                continue;
            }
            let s: String = cells[i].text.chars().filter(|c| !c.is_whitespace()).collect();
            let chars: Vec<char> = s.chars().collect();
            let mut candidates: Vec<String> = Vec::new();
            if fits(&s) {
                candidates.push(s.clone());
            }
            // One character in place of the mark.
            for (p, &c) in chars.iter().enumerate() {
                if c == mark || !(MARK_LOOKALIKES.contains(c) || c == '1' || c == '7') {
                    continue;
                }
                let mut t = chars.clone();
                t[p] = mark;
                let t: String = t.into_iter().collect();
                if fits(&t) {
                    candidates.push(t);
                }
            }
            // The mark left out.
            if chars.iter().all(char::is_ascii_digit) {
                for p in 1..chars.len() {
                    let t: String = chars[..p].iter().chain(std::iter::once(&mark)).chain(&chars[p..]).collect();
                    if fits(&t) {
                        candidates.push(t);
                    }
                }
            }
            candidates.sort();
            candidates.dedup();
            let chosen = match candidates.len() {
                0 => None,
                1 => candidates.pop(),
                _ => {
                    // Pick by how wide the printed value is.
                    let (Some(pc), Some((l, r))) = (per_char, cells[i].span) else { continue };
                    let actual = r - l;
                    let mut scored: Vec<(f32, String)> =
                        candidates.into_iter().map(|t| (((t.chars().count() as f32) * pc - actual).abs(), t)).collect();
                    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
                    if scored[1].0 - scored[0].0 >= pc * 0.4 { Some(scored.swap_remove(0).1) } else { None }
                }
            };
            if let Some(t) = chosen {
                cells[i].text = t;
            }
        }
    }
}

/// "x" and "X" in a column of mostly "x": all "x".
fn normalize_case(cells: &mut [Read], members: &[usize]) {
    let mut best: Option<(String, usize)> = None;
    for &i in members {
        let t = &cells[i].text;
        let n = members.iter().filter(|&&j| cells[j].text == *t).count();
        if best.as_ref().is_none_or(|(_, m)| n > *m) {
            best = Some((t.clone(), n));
        }
    }
    let Some((word, n)) = best else { return };
    if n * 10 < members.len() * 6 || !word.chars().any(char::is_alphabetic) {
        return;
    }
    // In a column of one letter ("x" between width and length), a lone
    // mark is that letter misread.
    let same = members.iter().filter(|&&j| cells[j].text.eq_ignore_ascii_case(&word)).count();
    let single = word.chars().count() == 1 && same * 10 >= members.len() * 8;
    for &i in members {
        let t = &cells[i].text;
        let garbage = single && t.chars().count() == 1 && t.chars().all(|c| "?*%#\"'`^~|!".contains(c));
        if t.eq_ignore_ascii_case(&word) || garbage {
            cells[i].text = word.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::ocr::Bounds;

    /// A cell of one line, each character 10 px wide from x = 0.
    fn read(row: usize, col: usize, text: &str) -> Read {
        let chars: Vec<(char, f32)> = text.chars().enumerate().map(|(i, c)| (c, i as f32 * 10.0)).collect();
        let w = text.chars().count() as f32 * 10.0;
        Read {
            cell: table::Cell {
                row,
                col,
                rows: 1,
                cols: 1,
                lines: vec![Bounds { left: 0.0, top: 0.0, right: w, bottom: 10.0 }],
                boxed: false,
                block: Some(0),
            },
            text: text.into(),
            chars,
            span: Some((0.0, w)),
        }
    }

    fn texts(cells: &[Read], col: usize) -> Vec<String> {
        let mut v: Vec<(usize, String)> =
            cells.iter().filter(|r| r.cell.col == col).map(|r| (r.cell.row, r.text.clone())).collect();
        v.sort();
        v.into_iter().map(|(_, t)| t).collect()
    }

    #[test]
    fn splits_columns_with_the_same_layout() {
        let mut cells = vec![
            read(0, 0, "WIDTH - LENGTH"),
            read(0, 1, "WGT"),
            read(1, 0, "12'00 x 179'02"),
            read(1, 1, "779"),
            read(2, 0, "12'00 x 73'10"),
            read(2, 1, "321"),
            read(3, 0, "12'00 x 141'01"),
            read(3, 1, "613"),
            read(4, 0, "12'00 ? 170'08"),
            read(4, 1, "742"),
        ];
        let mut cols = 2;
        split(&mut cells, &mut cols);
        assert_eq!(cols, 4);
        // The heading has three words too, so it is split the same way.
        assert_eq!(texts(&cells, 0), vec!["WIDTH", "12'00", "12'00", "12'00", "12'00"]);
        assert_eq!(texts(&cells, 1), vec!["-", "x", "x", "x", "?"]);
        assert_eq!(texts(&cells, 2), vec!["LENGTH", "179'02", "73'10", "141'01", "170'08"]);
        assert_eq!(texts(&cells, 3), vec!["WGT", "779", "321", "613", "742"]);
    }

    #[test]
    fn a_column_left_whole_does_not_move_the_others() {
        // Same kinds of words but different numbers of them: not split.
        let mut cells = vec![
            read(0, 0, "Invoice 20431"),
            read(0, 1, "10.00"),
            read(1, 0, "Credit memo 118"),
            read(1, 1, "5.00"),
            read(2, 0, "Invoice 20502"),
            read(2, 1, "7.50"),
        ];
        let mut cols = 2;
        split(&mut cells, &mut cols);
        assert_eq!(cols, 2);
        assert_eq!(texts(&cells, 1), vec!["10.00", "5.00", "7.50"]);
    }

    #[test]
    fn leaves_varied_columns_alone() {
        let mut cells = vec![
            read(0, 0, "Opening balance"),
            read(1, 0, "Invoice 20431"),
            read(2, 0, "Payment - thank you"),
            read(3, 0, "Invoice 20502"),
            read(4, 0, "Credit memo 118"),
        ];
        let mut cols = 1;
        split(&mut cells, &mut cols);
        assert_eq!(cols, 1);
        assert_eq!(cells.len(), 5);
    }

    #[test]
    fn repairs_marks_to_the_column_shape() {
        let mut cells: Vec<Read> =
            ["12'00", "12100", "12 '00", "12\"00", "12'00", "12?00", "12'00", "12'00", "12'00", "12'00", "12'00"]
                .iter()
                .enumerate()
                .map(|(r, t)| read(r, 0, t))
                .collect();
        repair(&mut cells);
        assert!(cells.iter().all(|r| r.text == "12'00"), "{:?}", texts(&cells, 0));

        // Lengths of 2 or 3 digits before the mark: a dropped mark is put
        // back; "18105" is printed six characters wide, so it was 181'05.
        let mut cells: Vec<Read> = ["179'02", "73?10", "141'01", "17008", "113'02", "172'03", "116'06"]
            .iter()
            .enumerate()
            .map(|(r, t)| read(r, 0, t))
            .collect();
        let mut wide = read(7, 0, "18105");
        wide.span = Some((0.0, 60.0));
        cells.push(wide);
        repair(&mut cells);
        assert_eq!(
            texts(&cells, 0),
            vec!["179'02", "73'10", "141'01", "170'08", "113'02", "172'03", "116'06", "181'05"]
        );
    }

    #[test]
    fn closes_up_a_space_inside_a_value() {
        let mut cells: Vec<Read> = ["170'03", "113'06", "118'04", "158'10", "160'11", "90 '01", "152'04", "190'05"]
            .iter()
            .enumerate()
            .map(|(r, t)| read(r, 0, t))
            .collect();
        repair(&mut cells);
        assert_eq!(texts(&cells, 0)[5], "90'01");
    }

    #[test]
    fn evens_out_capitals() {
        let mut cells: Vec<Read> =
            ["x", "X", "x", "x", "X", "x", "?", "x", "x", "x"].iter().enumerate().map(|(r, t)| read(r, 0, t)).collect();
        repair(&mut cells);
        assert!(cells.iter().all(|r| r.text == "x"), "{:?}", texts(&cells, 0));
    }
}
