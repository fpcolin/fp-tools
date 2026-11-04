//! A small JSON reader and writer.
//!
//! Enough for the settings file and the update manifest. Output matches
//! Python's `json.dumps(obj, indent=2)` byte for byte, so a settings file
//! written here can still be read by the older Python builds and vice versa.

use std::fmt::Write as _;

/// Nesting deeper than this is rejected rather than risking the stack.
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    /// Keys keep their file order. Duplicate keys are kept; `get` returns the
    /// last one, which is what Python's `json.loads` does.
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Replace the value of `key`, or append it. Objects only.
    pub fn set(&mut self, key: &str, value: Value) {
        if let Value::Object(members) = self {
            if let Some(slot) = members.iter_mut().rev().find(|(k, _)| k == key) {
                slot.1 = value;
            } else {
                members.push((key.to_string(), value));
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub message: &'static str,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid JSON at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parse a complete JSON document. A leading UTF-8 byte-order mark is allowed.
pub fn parse(text: &str) -> Result<Value, ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    p.ws();
    let value = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(p.err("unexpected data after the document"));
    }
    Ok(value)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, message: &'static str) -> ParseError {
        ParseError { offset: self.i, message }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.i += 1;
        }
    }

    fn eat(&mut self, literal: &str) -> bool {
        if self.s[self.i..].starts_with(literal.as_bytes()) {
            self.i += literal.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deeply"));
        }
        match self.peek() {
            None => Err(self.err("unexpected end of data")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) if self.eat("true") => Ok(Value::Bool(true)),
            Some(_) if self.eat("false") => Ok(Value::Bool(false)),
            Some(_) if self.eat("null") => Ok(Value::Null),
            Some(_) => Err(self.err("expected a value")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.i += 1; // '{'
        let mut members = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Object(members));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected a property name"));
            }
            let key = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected ':'"));
            }
            self.i += 1;
            self.ws();
            let value = self.value(depth + 1)?;
            members.push((key, value));
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Object(members));
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.i += 1; // '['
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn number(&mut self) -> Result<Value, ParseError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        match self.peek() {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                while let Some(b'0'..=b'9') = self.peek() {
                    self.i += 1;
                }
            }
            _ => return Err(self.err("invalid number")),
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("invalid number"));
            }
            while let Some(b'0'..=b'9') = self.peek() {
                self.i += 1;
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.i += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.i += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("invalid number"));
            }
            while let Some(b'0'..=b'9') = self.peek() {
                self.i += 1;
            }
        }
        // The slice is ASCII digits and signs only, so this cannot fail on
        // UTF-8 grounds; parse failure would only be an overflow to inf.
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| self.err("invalid number"))?;
        let n: f64 = text.parse().map_err(|_| self.err("invalid number"))?;
        if !n.is_finite() {
            return Err(self.err("number out of range"));
        }
        Ok(Value::Number(n))
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let digits = self.s.get(self.i..self.i + 4).ok_or_else(|| self.err("truncated \\u escape"))?;
        let mut v = 0u32;
        for &d in digits {
            let n = (d as char).to_digit(16).ok_or_else(|| self.err("invalid \\u escape"))?;
            v = v * 16 + n;
        }
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            // Copy the longest run with nothing special in it.
            let run_start = self.i;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.i += 1;
            }
            // The input is a &str and we stopped on an ASCII byte, so the run
            // is on character boundaries.
            out.push_str(std::str::from_utf8(&self.s[run_start..self.i]).map_err(|_| self.err("invalid UTF-8"))?);
            match self.peek() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let escape = self.peek().ok_or_else(|| self.err("unterminated string"))?;
                    self.i += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) && self.s[self.i..].starts_with(b"\\u") {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                                } else {
                                    self.i = save;
                                    hi
                                }
                            } else {
                                hi
                            };
                            // A lone surrogate cannot live in a Rust string.
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        _ => return Err(self.err("invalid escape")),
                    }
                }
                Some(_) => return Err(self.err("control character in string")),
            }
        }
    }
}

/// Serialise like Python's `json.dumps(value, indent=2)` (ensure_ascii on).
pub fn to_string_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    out
}

fn write_value(out: &mut String, value: &Value, level: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => write_number(out, *n),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, level + 1);
                write_value(out, item, level + 1);
            }
            newline(out, level);
            out.push(']');
        }
        Value::Object(members) => {
            if members.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, item)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, level + 1);
                write_string(out, key);
                out.push_str(": ");
                write_value(out, item, level + 1);
            }
            newline(out, level);
            out.push('}');
        }
    }
}

fn newline(out: &mut String, level: usize) {
    out.push('\n');
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn write_number(out: &mut String, n: f64) {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        let _ = write!(out, "{}", n as i64);
    } else if n.is_finite() {
        let _ = write!(out, "{n:?}");
    } else {
        out.push_str("null");
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                // Everything outside printable ASCII, as Python does with
                // ensure_ascii, using surrogate pairs above the BMP.
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_compatible_output() {
        let v = Value::Object(vec![
            ("schema".into(), Value::Number(1.0)),
            ("version".into(), Value::String("3.0.0".into())),
            ("saveto_path".into(), Value::String("C:\\Users\\Zoë\\Downloads".into())),
            ("flag".into(), Value::Bool(false)),
            ("emoji".into(), Value::String("\u{1F600}\u{7f}\"".into())),
            ("empty".into(), Value::Object(vec![])),
            ("list".into(), Value::Array(vec![Value::Null, Value::Number(2.5)])),
        ]);
        // Produced by: json.dumps({...}, indent=2) in CPython 3.12.
        let expected = "{\n  \"schema\": 1,\n  \"version\": \"3.0.0\",\n  \"saveto_path\": \"C:\\\\Users\\\\Zo\\u00eb\\\\Downloads\",\n  \"flag\": false,\n  \"emoji\": \"\\ud83d\\ude00\\u007f\\\"\",\n  \"empty\": {},\n  \"list\": [\n    null,\n    2.5\n  ]\n}";
        assert_eq!(to_string_pretty(&v), expected);
        assert_eq!(parse(expected).unwrap(), v);
    }

    #[test]
    fn parse_edge_cases() {
        assert!(parse(" \u{feff}1").is_err());
        assert_eq!(parse("\u{feff}{\"a\": 1}").unwrap().get("a"), Some(&Value::Number(1.0)));
        assert_eq!(parse("{\"a\":1,\"a\":2}").unwrap().get("a"), Some(&Value::Number(2.0)));
        assert_eq!(parse("\"\\ud800\"").unwrap(), Value::String("\u{fffd}".into()));
        assert_eq!(parse("\"\\ud83d\\ude00\"").unwrap(), Value::String("\u{1F600}".into()));
        for bad in ["", "{", "[1,]", "{\"a\" 1}", "01", "1.", "-", "\"a", "\"\u{1}\"", "nul", "[1] x", "1e999"] {
            assert!(parse(bad).is_err(), "{bad:?} should fail");
        }
        let deep = "[".repeat(100) + &"]".repeat(100);
        assert!(parse(&deep).is_err());
        let ok = "[".repeat(60) + &"]".repeat(60);
        assert!(parse(&ok).is_ok());
    }
}
