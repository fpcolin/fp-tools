//! Turning the text read from a cell into what Excel should store: numbers
//! as numbers (formatted as they were printed), dates as dates, and
//! everything else, including codes with leading zeros, as text.

/// A cell's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    /// A number and the Excel number format that shows it as printed.
    Number(f64, &'static str),
}

/// Excel number formats used here (built-in ones are mapped to their ids
/// when the workbook is written).
pub mod formats {
    pub const GENERAL: &str = "General";
    pub const DATE: &str = "m/d/yyyy";
    /// Dates as printed: with or without leading zeros, 4- or 2-digit year.
    pub const DATES: [&str; 4] = ["m/d/yyyy", "mm/dd/yyyy", "m/d/yy", "mm/dd/yy"];
    pub const DECIMALS: [&str; 5] = ["0", "0.0", "0.00", "0.000", "0.0000"];
    pub const GROUPED: [&str; 5] = ["#,##0", "#,##0.0", "#,##0.00", "#,##0.000", "#,##0.0000"];
    pub const CURRENCY: [&str; 5] =
        ["\"$\"#,##0", "\"$\"#,##0.0", "\"$\"#,##0.00", "\"$\"#,##0.000", "\"$\"#,##0.0000"];
    pub const ACCOUNTING: [&str; 5] = [
        "#,##0;(#,##0)",
        "#,##0.0;(#,##0.0)",
        "#,##0.00;(#,##0.00)",
        "#,##0.000;(#,##0.000)",
        "#,##0.0000;(#,##0.0000)",
    ];
    pub const ACCOUNTING_CURRENCY: [&str; 5] = [
        "\"$\"#,##0;(\"$\"#,##0)",
        "\"$\"#,##0.0;(\"$\"#,##0.0)",
        "\"$\"#,##0.00;(\"$\"#,##0.00)",
        "\"$\"#,##0.000;(\"$\"#,##0.000)",
        "\"$\"#,##0.0000;(\"$\"#,##0.0000)",
    ];
    pub const PERCENT: [&str; 5] = ["0%", "0.0%", "0.00%", "0.000%", "0.0000%"];
    pub const PERCENT_PARENS: [&str; 5] =
        ["0%;(0%)", "0.0%;(0.0%)", "0.00%;(0.00%)", "0.000%;(0.000%)", "0.0000%;(0.0000%)"];
}

/// What a cell's text means.
pub fn parse(text: &str) -> Value {
    let t = text.trim();
    if let Some(v) = number(t).or_else(|| date(t)) {
        return v;
    }
    Value::Text(t.to_string())
}

/// Like [`parse`], but first undoing the usual misreadings of digits as
/// letters (O for 0, l or I for 1...). For cells in a column of numbers.
pub fn parse_numeric(text: &str) -> Value {
    let t = text.trim();
    if let v @ Value::Number(..) = parse(t) {
        return v;
    }
    // Specks read as marks at either end ("702,115.00_"), and a colon or
    // semicolon read for a comma ("2:927.15").
    let trimmed = t.trim_matches(|c: char| "_|`'\"~^*:;".contains(c)).trim();
    for candidate in [trimmed.to_string(), trimmed.replace([':', ';'], ","), trimmed.replace([':', ';'], ".")] {
        if let v @ Value::Number(..) = parse(&candidate) {
            return v;
        }
    }
    // Only letters that look like digits, and at least one real digit.
    let confusable = |c: char| "OoDQlIi|!SsBZzG".contains(c);
    if !t.chars().any(|c| c.is_ascii_digit()) || t.chars().any(|c| c.is_alphabetic() && !confusable(c)) {
        return Value::Text(t.to_string());
    }
    let fixed: String = t
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            'O' | 'o' | 'D' | 'Q' => '0',
            'l' | 'I' | '|' | 'i' | '!' => '1',
            'S' | 's' => '5',
            'B' => '8',
            'Z' | 'z' => '2',
            'G' => '6',
            other => other,
        })
        .collect();
    match parse(&fixed) {
        v @ Value::Number(..) => v,
        _ => Value::Text(t.to_string()),
    }
}

fn number(t: &str) -> Option<Value> {
    let mut s = t.replace(['\u{2212}', '\u{2013}'], "-");
    // Spaces read inside a number ("1, 608.80").
    if s.chars().any(char::is_whitespace) {
        let joined: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        if joined.chars().filter(char::is_ascii_digit).count() * 2 >= joined.len() {
            s = joined;
        } else {
            return None;
        }
    }
    let mut negative = false;
    let mut parens = false;
    if s.starts_with('(') && s.ends_with(')') && s.len() > 2 {
        parens = true;
        negative = true;
        s = s[1..s.len() - 1].to_string();
    }
    if let Some(rest) = s.strip_prefix('-') {
        negative = !negative;
        s = rest.to_string();
    } else if let Some(rest) = s.strip_suffix('-') {
        // Trailing minus, as on some statements.
        negative = !negative;
        s = rest.to_string();
    } else if let Some(rest) = s.strip_prefix('+') {
        s = rest.to_string();
    }
    let mut currency = false;
    if let Some(rest) = s.strip_prefix('$') {
        currency = true;
        s = rest.to_string();
        if let Some(rest) = s.strip_prefix('-') {
            negative = !negative;
            s = rest.to_string();
        }
    }
    let mut percent = false;
    if let Some(rest) = s.strip_suffix('%') {
        percent = true;
        s = rest.to_string();
    }
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '.') {
        return None;
    }
    if !s.starts_with(|c: char| c.is_ascii_digit()) && !s.starts_with('.') {
        return None;
    }
    // Several dots: all but the last are thousands separators if they are
    // followed by three digits ("1.608.80" read for "1,608.80").
    let dots = s.matches('.').count();
    if dots > 1 {
        let last = s.rfind('.')?;
        let (head, tail) = s.split_at(last);
        let groups: Vec<&str> = head.split('.').collect();
        if groups[1..].iter().all(|g| g.len() == 3 && !g.contains(',')) && !groups[0].is_empty() {
            s = format!("{}{}", groups.join(","), tail);
        } else {
            return None;
        }
    }
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s.as_str(), None),
    };
    if let Some(f) = frac_part
        && (f.contains(',') || f.is_empty() && int_part.is_empty())
    {
        return None;
    }
    let grouped = int_part.contains(',');
    if grouped {
        let groups: Vec<&str> = int_part.split(',').collect();
        if groups[0].is_empty() || groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3) {
            return None;
        }
    }
    let digits: String = int_part.chars().filter(char::is_ascii_digit).collect();
    // Keep codes as text: leading zeros (00123), and long digit strings
    // (account numbers, UPCs) that Excel would round or show as 1.2E+11.
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    if !grouped && frac_part.is_none() && digits.len() > 10 {
        return None;
    }
    let decimals = frac_part.map_or(0, str::len);
    if digits.len() + decimals > 15 {
        return None;
    }
    let plain = format!(
        "{}{}{}",
        if digits.is_empty() { "0" } else { &digits },
        if frac_part.is_some() { "." } else { "" },
        frac_part.unwrap_or("")
    );
    let mut value: f64 = plain.parse().ok()?;
    if negative {
        value = -value;
    }
    let d = decimals.min(4);
    let format = if percent {
        value /= 100.0;
        if parens { formats::PERCENT_PARENS[d] } else { formats::PERCENT[d] }
    } else if currency {
        if parens { formats::ACCOUNTING_CURRENCY[d] } else { formats::CURRENCY[d] }
    } else if parens {
        formats::ACCOUNTING[d]
    } else if grouped {
        formats::GROUPED[d]
    } else if decimals > 0 {
        formats::DECIMALS[d]
    } else {
        formats::GENERAL
    };
    // Round away binary noise from the percent division.
    if percent {
        let scale = 10f64.powi(decimals as i32 + 2);
        value = (value * scale).round() / scale;
    }
    Some(Value::Number(value, format))
}

/// US dates: 10/2/2026, 10/02/26, 10-02-2026.
fn date(t: &str) -> Option<Value> {
    let sep = if t.contains('/') { '/' } else { '-' };
    let parts: Vec<&str> = t.split(sep).collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    if parts[0].len() > 2 || parts[1].len() > 2 || !(parts[2].len() == 2 || parts[2].len() == 4) {
        return None;
    }
    let m: u32 = parts[0].parse().ok()?;
    let d: u32 = parts[1].parse().ok()?;
    let mut y: i32 = parts[2].parse().ok()?;
    if parts[2].len() == 2 {
        y += if y < 70 { 2000 } else { 1900 };
    }
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) || !(1901..=9999).contains(&y) {
        return None;
    }
    let zeros = parts[0].len() == 2 && parts[0].starts_with('0') || parts[1].len() == 2 && parts[1].starts_with('0');
    let short_year = parts[2].len() == 2;
    let format = formats::DATES[usize::from(zeros) + 2 * usize::from(short_year)];
    Some(Value::Number(excel_serial(y, m, d) as f64, format))
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 31,
    }
}

/// Days since 1899-12-30 (Excel's date numbering for dates after
/// February 1900).
pub fn excel_serial(y: i32, m: u32, d: u32) -> i64 {
    days_from_civil(y, m, d) - days_from_civil(1899, 12, 30)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(if m <= 2 { y - 1 } else { y });
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(t: &str) -> (f64, &'static str) {
        match parse(t) {
            Value::Number(v, f) => (v, f),
            other => panic!("{t}: {other:?}"),
        }
    }

    #[test]
    fn numbers_keep_their_look() {
        assert_eq!(num("120"), (120.0, "General"));
        assert_eq!(num("2.45"), (2.45, "0.00"));
        assert_eq!(num("1,608.80"), (1608.8, "#,##0.00"));
        assert_eq!(num("$1,608.80"), (1608.8, "\"$\"#,##0.00"));
        assert_eq!(num("(45.00)"), (-45.0, "#,##0.00;(#,##0.00)"));
        assert_eq!(num("45.00-"), (-45.0, "0.00"));
        assert_eq!(num("-$3.50"), (-3.5, "\"$\"#,##0.00"));
        assert_eq!(num("12.5%"), (0.125, "0.0%"));
        assert_eq!(num("(2.4%)"), (-0.024, "0.0%;(0.0%)"));
        assert_eq!(num("0.65"), (0.65, "0.00"));
        assert_eq!(num(".5"), (0.5, "0.0"));
        assert_eq!(num("1.608.80"), (1608.8, "#,##0.00"));
        assert_eq!(num("1, 608.80"), (1608.8, "#,##0.00"));
    }

    #[test]
    fn codes_stay_text() {
        for t in ["00123", "0123", "FP-1001", "12345678901", "1,23", "4471-B", "1.2.3", "12x24", "", "$", "7/16"] {
            assert!(matches!(parse(t), Value::Text(_)), "{t}");
        }
    }

    #[test]
    fn dates() {
        assert_eq!(parse("10/02/2026"), Value::Number(46297.0, "mm/dd/yyyy"));
        assert_eq!(parse("10/2/2026"), Value::Number(46297.0, "m/d/yyyy"));
        assert!(matches!(parse("1/1/1900"), Value::Text(_)));
        assert_eq!(parse("2/29/24"), Value::Number(45351.0, "m/d/yy"));
        assert!(matches!(parse("2/30/2024"), Value::Text(_)));
        assert!(matches!(parse("13/01/2024"), Value::Text(_)));
    }

    #[test]
    fn misread_digits_in_number_columns() {
        assert_eq!(parse_numeric("1O5.OO"), Value::Number(105.0, "0.00"));
        assert_eq!(parse_numeric("l20"), Value::Number(120.0, "General"));
        assert_eq!(parse_numeric("Total"), Value::Text("Total".into()));
        assert_eq!(parse_numeric("2:927.15"), Value::Number(2927.15, "#,##0.00"));
        assert_eq!(parse_numeric("$702,115.00_"), Value::Number(702115.0, "\"$\"#,##0.00"));
        assert_eq!(parse_numeric("LV-2040"), Value::Text("LV-2040".into()));
    }
}
