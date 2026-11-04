//! CSV reading and writing with the exact behaviour of Python's `csv` module
//! (excel dialect), which the previous version used. Matching it matters:
//! the converted file must come out byte-for-byte as before so VendorCafe
//! accepts it, and quirky input must be read the same way.

use std::fmt;

/// Python's default `csv.field_size_limit()`.
pub const FIELD_LIMIT: usize = 131_072;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsvError {
    /// A single field is longer than `FIELD_LIMIT` characters.
    FieldTooLarge { record: usize },
}

impl fmt::Display for CsvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CsvError::FieldTooLarge { record } => {
                write!(f, "row {record} has a field larger than the limit of {FIELD_LIMIT} characters")
            }
        }
    }
}

impl std::error::Error for CsvError {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    StartRecord,
    StartField,
    InField,
    InQuotedField,
    QuoteInQuotedField,
    EatCrNl,
}

/// Split text into lines the way Python does with `newline=''`: each line
/// ends after "\n", "\r\n", or a lone "\r", and keeps its terminator.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    let bytes = text.as_bytes();
    let mut start = 0;
    std::iter::from_fn(move || {
        if start >= bytes.len() {
            return None;
        }
        let mut i = start;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    i += 1;
                    break;
                }
                b'\r' => {
                    i += if bytes.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                    break;
                }
                _ => i += 1,
            }
        }
        let line = &text[start..i];
        start = i;
        Some(line)
    })
}

/// CPython's reader state (Modules/_csv.c), excel dialect, strict off.
struct Parser {
    state: State,
    fields: Vec<String>,
    field: String,
    field_len: usize,
    record_no: usize,
}

impl Parser {
    fn add(&mut self, ch: char) -> Result<(), CsvError> {
        if self.field_len >= FIELD_LIMIT {
            return Err(CsvError::FieldTooLarge { record: self.record_no });
        }
        self.field.push(ch);
        self.field_len += 1;
        Ok(())
    }

    fn save(&mut self) {
        self.fields.push(std::mem::take(&mut self.field));
        self.field_len = 0;
    }

    /// End a record at a line break; `None` is the end-of-line marker.
    fn end_line(&mut self, c: Option<char>) {
        self.save();
        self.state = if c.is_none() { State::StartRecord } else { State::EatCrNl };
    }

    /// One character, or `None` for the end-of-line marker CPython feeds
    /// after each physical line.
    fn step(&mut self, c: Option<char>) -> Result<(), CsvError> {
        let newline = matches!(c, Some('\n' | '\r') | None);
        if self.state == State::StartRecord {
            match c {
                None => return Ok(()), // blank line: an empty record
                Some('\n' | '\r') => {
                    self.state = State::EatCrNl;
                    return Ok(());
                }
                Some(_) => self.state = State::StartField,
            }
        }
        match (self.state, c) {
            (State::StartField, _) if newline => self.end_line(c),
            (State::StartField, Some('"')) => self.state = State::InQuotedField,
            (State::StartField, Some(',')) => self.save(),
            (State::StartField, Some(ch)) => {
                self.add(ch)?;
                self.state = State::InField;
            }
            (State::InField, _) if newline => self.end_line(c),
            (State::InField, Some(',')) => {
                self.save();
                self.state = State::StartField;
            }
            (State::InField, Some(ch)) => self.add(ch)?,
            (State::InQuotedField, None) => {}
            (State::InQuotedField, Some('"')) => self.state = State::QuoteInQuotedField,
            (State::InQuotedField, Some(ch)) => self.add(ch)?,
            (State::QuoteInQuotedField, Some('"')) => {
                self.add('"')?;
                self.state = State::InQuotedField;
            }
            (State::QuoteInQuotedField, Some(',')) => {
                self.save();
                self.state = State::StartField;
            }
            (State::QuoteInQuotedField, _) if newline => self.end_line(c),
            // Not strict: a stray character after a closing quote is kept.
            (State::QuoteInQuotedField, Some(ch)) => {
                self.add(ch)?;
                self.state = State::InField;
            }
            (State::EatCrNl, None) => self.state = State::StartRecord,
            // Lines always end at CR or LF, so nothing else reaches EatCrNl.
            (State::EatCrNl, Some(_)) => {}
            (State::StartRecord, _) | (_, None) => {}
        }
        Ok(())
    }
}

/// Parse every record, including empty ones (`[]`) for blank lines, exactly
/// as `csv.reader` yields them.
pub fn read_records(text: &str) -> Result<Vec<Vec<String>>, CsvError> {
    let mut records = Vec::new();
    let mut p =
        Parser { state: State::StartRecord, fields: Vec::new(), field: String::new(), field_len: 0, record_no: 1 };
    for line in lines(text) {
        p.record_no = records.len() + 1;
        for ch in line.chars() {
            p.step(Some(ch))?;
        }
        p.step(None)?;
        if p.state == State::StartRecord {
            records.push(std::mem::take(&mut p.fields));
        }
    }
    // End of input inside a quoted field: Python (non-strict) keeps what it has.
    if p.field_len != 0 || p.state == State::InQuotedField {
        p.save();
        records.push(p.fields);
    }
    Ok(records)
}

/// Append one record the way `csv.writer` (QUOTE_MINIMAL, CRLF) writes it.
pub fn write_record<S: AsRef<str>>(out: &mut String, fields: &[S]) {
    if fields.len() == 1 && fields[0].as_ref().is_empty() {
        // A lone empty field is quoted so the line is not mistaken for blank.
        out.push_str("\"\"\r\n");
        return;
    }
    for (i, field) in fields.iter().enumerate() {
        let field = field.as_ref();
        if i > 0 {
            out.push(',');
        }
        if field.contains([',', '"', '\r', '\n']) {
            out.push('"');
            out.push_str(&field.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(field);
        }
    }
    out.push_str("\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(text: &str) -> Vec<Vec<String>> {
        read_records(text).unwrap()
    }

    #[test]
    fn reads_like_python() {
        // Expected values produced with list(csv.reader(io.StringIO(text, newline=''))).
        assert_eq!(rec("a,b\r\nc,d"), vec![vec!["a", "b"], vec!["c", "d"]]);
        assert_eq!(rec("a,\"b,\"\"c\"\"\nd\"\r\n\r\n,\n"), vec![vec!["a", "b,\"c\"\nd"], vec![], vec!["", ""]]);
        assert_eq!(rec("\"ab\"c,d\re"), vec![vec!["abc", "d"], vec!["e"]]);
        assert_eq!(rec("a\"b,c"), vec![vec!["a\"b", "c"]]);
        assert_eq!(rec("x,\"unterminated\nmore"), vec![vec!["x", "unterminated\nmore"]]);
        assert_eq!(rec(""), Vec::<Vec<String>>::new());
        assert_eq!(rec("\n"), vec![Vec::<String>::new()]);
    }

    #[test]
    fn writes_like_python() {
        let mut out = String::new();
        write_record(&mut out, &["a", "b c", "d,e", "f\"g", "h\ni", ""]);
        write_record(&mut out, &[""]);
        assert_eq!(out, "a,b c,\"d,e\",\"f\"\"g\",\"h\ni\",\r\n\"\"\r\n");
    }

    #[test]
    fn field_limit() {
        let big = "x".repeat(FIELD_LIMIT + 1);
        assert!(read_records(&big).is_err());
        assert!(read_records(&"x".repeat(FIELD_LIMIT)).is_ok());
    }
}
