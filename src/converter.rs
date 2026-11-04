//! The CSV transformation, with no user interface attached.
//!
//! Behaviour is identical to the Python build: every field is handled as
//! text, so amounts, leading zeros and dates are written back exactly as
//! exported; and quirks of Python's csv.DictReader/DictWriter (duplicate
//! headers, short and long rows, blank lines) are reproduced.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::common::fsutil;
use crate::csv;

/// Every column the exported file must contain. The order is only used for
/// reporting; the output keeps whatever order the source file had.
pub const REQUIRED_COLUMNS: [&str; 18] = [
    "PO No",
    "Job No",
    "Type",
    "Invoice No",
    "Invoice Desc",
    "Invoice Date",
    "Due Date",
    "Invoice Total",
    "Tax Total",
    "Shipping Total",
    "Property Name",
    "Property Code",
    "Unit",
    "Item Sequence",
    "Item Desc",
    "Quantity",
    "Unit Price",
    "Item Amount",
];

/// Refuse to load anything this large; a real export is a few megabytes.
const MAX_FILE_SIZE: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// "Convert CSV (PO)": the customer uses a PO number.
    PurchaseOrder,
    /// "Convert CSV (WO)": the customer uses a work-order number. Job No is
    /// dropped and PO No is relabelled WO No in place.
    WorkOrder,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::PurchaseOrder => "PO",
            Mode::WorkOrder => "WO",
        }
    }
}

/// A message suitable for showing directly to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionError(pub String);

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConversionError {}

fn error(message: impl Into<String>) -> ConversionError {
    ConversionError(message.into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub target: PathBuf,
    /// Invoice line rows read.
    pub rows_in: usize,
    /// Invoices written.
    pub rows_out: usize,
    pub mode: Mode,
}

impl Outcome {
    /// The confirmation text the Python build showed.
    pub fn summary(&self) -> String {
        format!(
            "{} rows consolidated into {} invoices ({}).\n\nSaved to:\n{}",
            self.rows_in,
            self.rows_out,
            self.mode.label(),
            self.target.display()
        )
    }
}

/// A row as Python's DictReader produced it: an insertion-ordered dict where
/// a missing value is `None` and a repeated header overwrites in place.
#[derive(Clone, Debug)]
struct Row(Vec<(String, Option<String>)>);

impl Row {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.as_deref())
    }

    /// `row.get(key) or ''`
    fn text(&self, key: &str) -> String {
        self.get(key).unwrap_or("").to_string()
    }

    fn set(&mut self, key: &str, value: Option<String>) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    /// `row.pop(key, '')`
    fn pop(&mut self, key: &str) -> Option<String> {
        match self.0.iter().position(|(k, _)| k == key) {
            Some(i) => self.0.remove(i).1,
            None => Some(String::new()),
        }
    }
}

/// Python's `str.strip()`: also removes the ASCII separator characters
/// 0x1C-0x1F, which Rust's `trim` keeps.
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// Decode as UTF-8 (dropping a byte-order mark), falling back to Windows-1252
/// exactly like Python's `utf-8-sig` then `cp1252` attempts.
pub fn decode_text(bytes: &[u8]) -> Result<String, ConversionError> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.strip_prefix('\u{feff}').unwrap_or(text).to_string());
    }
    // Bytes Windows-1252 leaves undefined; Python's codec rejects them.
    const C1: [u16; 32] = [
        0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0, 0x017D,
        0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0,
        0x017E, 0x0178,
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => char::from_u32(u32::from(C1[usize::from(b - 0x80)])).filter(|&c| c != '\0'),
            _ => Some(char::from(b)),
        })
        .collect::<Option<String>>()
        .ok_or_else(|| error("Could not read the file - unexpected text encoding."))
}

/// Convert the text of an export. Returns (output CSV, rows in, rows out).
pub fn convert_text(text: &str, mode: Mode) -> Result<(String, usize, usize), ConversionError> {
    let records = csv::read_records(text).map_err(|e| error(format!("Could not read the file: {e}.")))?;
    let mut records = records.into_iter();
    let fieldnames = records.next().unwrap_or_default();
    if fieldnames.is_empty() {
        return Err(error("That file appears to be empty."));
    }

    let missing: Vec<&str> = REQUIRED_COLUMNS.iter().copied().filter(|c| !fieldnames.iter().any(|f| f == c)).collect();
    if !missing.is_empty() {
        return Err(error(format!(
            "This does not look like a VendorCafe export.\n\nMissing columns:\n  {}\n\nExport the invoices again from Rollmaster without editing the file.",
            missing.join("\n  ")
        )));
    }

    // DictReader: skip blank records; pad short rows with None; ignore extras.
    let rows: Vec<Row> = records
        .filter(|r| !r.is_empty())
        .map(|record| {
            let mut row = Row(Vec::with_capacity(fieldnames.len()));
            for (i, key) in fieldnames.iter().enumerate() {
                row.set(key, record.get(i).cloned());
            }
            row
        })
        .collect();
    if rows.is_empty() {
        return Err(error("That file has column headings but no invoice rows."));
    }

    // Keep the first row of each invoice, preserving the original order.
    let mut seen = std::collections::HashSet::new();
    let mut kept: Vec<Row> = Vec::new();
    for row in &rows {
        if seen.insert(py_strip(row.get("Invoice No").unwrap_or("")).to_string()) {
            kept.push(row.clone());
        }
    }

    for row in &mut kept {
        let total = row.text("Invoice Total");
        row.set("Item Amount", Some(total.clone()));
        row.set("Unit Price", Some(total));
        row.set("Item Sequence", Some("1".into()));
        row.set("Quantity", Some("1".into()));
        // Note: removes every "1-" in the number, not only a leading one.
        // That is what the Python build did, and existing uploads rely on it.
        let invoice = row.text("Invoice No").replace("1-", "");
        row.set("Invoice No", Some(invoice.clone()));
        row.set("Invoice Desc", Some(row.text("Invoice Desc").replace('\'', "")));
        row.set("Property Code", Some(row.text("Property Code").replace('\'', "")));
        row.set("Item Desc", Some(invoice));
    }

    let mut out_fields = fieldnames.clone();
    if mode == Mode::WorkOrder {
        // Drop the job number and relabel the PO column in place, so the
        // column order the upload expects is unchanged.
        if let Some(i) = out_fields.iter().position(|f| f == "Job No") {
            out_fields.remove(i);
        }
        if let Some(i) = out_fields.iter().position(|f| f == "PO No") {
            out_fields[i] = "WO No".to_string();
            for row in &mut kept {
                let po = row.pop("PO No");
                row.set("WO No", po);
            }
        }
    }

    let mut out = String::with_capacity(text.len() / 2);
    csv::write_record(&mut out, &out_fields);
    for row in &kept {
        let values: Vec<&str> = out_fields.iter().map(|k| row.get(k).unwrap_or("")).collect();
        csv::write_record(&mut out, &values);
    }
    Ok((out, rows.len(), kept.len()))
}

/// Where the result goes: over the export, or `<name> converted.csv` beside it.
pub fn target_path(source: &Path, overwrite: bool) -> PathBuf {
    if overwrite {
        return source.to_path_buf();
    }
    // Python's PurePath.stem/suffix rules.
    let name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, suffix) = match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name.split_at(i),
        _ => (name.as_str(), ""),
    };
    source.with_file_name(format!("{stem} converted{suffix}"))
}

/// Convert the file at `source`.
pub fn convert_file(source: &Path, mode: Mode, overwrite: bool) -> Result<Outcome, ConversionError> {
    if !source.is_file() {
        return Err(error(format!("The selected file no longer exists:\n{}", source.display())));
    }
    let bytes = fsutil::read_capped(source, MAX_FILE_SIZE)
        .map_err(|e| error(format!("Could not read the file:\n{}\n\n{e}", source.display())))?;
    let text = decode_text(&bytes)?;
    let (output, rows_in, rows_out) = convert_text(&text, mode)?;

    // Written to a temporary file and swapped in, so a failure part way
    // through cannot leave a truncated export.
    let target = target_path(source, overwrite);
    fsutil::write_atomic(&target, output.as_bytes()).map_err(|e| {
        error(format!(
            "Could not write the converted file:\n{e}\n\nIf the file is open in Excel, close it and try again."
        ))
    })?;
    Ok(Outcome { target, rows_in, rows_out, mode })
}

/// Convert the file chosen in the settings.
pub fn convert_selected(settings: &crate::config::VcSettings, mode: Mode) -> Result<Outcome, ConversionError> {
    if settings.filepath.is_empty() {
        return Err(error("No file selected. Click \"Change file\" first."));
    }
    convert_file(Path::new(&settings.filepath), mode, settings.option_overwrite)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "PO No,Job No,Type,Invoice No,Invoice Desc,Invoice Date,Due Date,Invoice Total,Tax Total,Shipping Total,Property Name,Property Code,Unit,Item Sequence,Item Desc,Quantity,Unit Price,Item Amount";

    #[test]
    fn consolidates_like_python() {
        let input = format!(
            "\u{feff}{HEADER}\r\nPO1,J1,I,1-1001,Desc 'a',01/02/2026,02/02/2026,1234.50,0.00,0,Prop,'P01,A,1,line one,2,10.00,20.00\r\n\
             PO1,J1,I,1-1001,Desc 'a',01/02/2026,02/02/2026,1234.50,0.00,0,Prop,'P01,A,2,line two,1,5.00,5.00\r\n\r\n\
             PO2,J2,I,0042,\"Multi, \"\"quoted\"\"\",01/02/2026,02/02/2026,007.10,0.00,0,Prop,P02,B,1,x,1,1,1\r\n"
        );
        let text = decode_text(input.as_bytes()).unwrap();
        let (out, rows_in, rows_out) = convert_text(&text, Mode::PurchaseOrder).unwrap();
        assert_eq!((rows_in, rows_out), (3, 2));
        assert_eq!(
            out,
            format!(
                "{HEADER}\r\nPO1,J1,I,1001,Desc a,01/02/2026,02/02/2026,1234.50,0.00,0,Prop,P01,A,1,1001,1,1234.50,1234.50\r\n\
                 PO2,J2,I,0042,\"Multi, \"\"quoted\"\"\",01/02/2026,02/02/2026,007.10,0.00,0,Prop,P02,B,1,0042,1,007.10,007.10\r\n"
            )
        );

        let (wo, _, _) = convert_text(&text, Mode::WorkOrder).unwrap();
        assert!(wo.starts_with("WO No,Type,Invoice No,"));
        assert!(wo.contains("\r\nPO1,I,1001,"));
    }

    #[test]
    fn reports_problems() {
        assert_eq!(convert_text("", Mode::PurchaseOrder).unwrap_err().0, "That file appears to be empty.");
        assert!(
            convert_text("PO No,Job No\r\n1,2\r\n", Mode::PurchaseOrder)
                .unwrap_err()
                .0
                .contains("Missing columns:\n  Type\n  Invoice No")
        );
        assert_eq!(
            convert_text(&format!("{HEADER}\r\n\r\n"), Mode::PurchaseOrder).unwrap_err().0,
            "That file has column headings but no invoice rows."
        );
        assert!(decode_text(b"caf\xe9 \x80").unwrap() == "café €");
        assert!(decode_text(b"bad \x81").is_err());
    }

    #[test]
    fn target_names() {
        assert_eq!(target_path(Path::new("dir/export.csv"), false), Path::new("dir/export converted.csv"));
        assert_eq!(target_path(Path::new("dir/a.b.csv"), false), Path::new("dir/a.b converted.csv"));
        assert_eq!(target_path(Path::new("dir/.csv"), false), Path::new("dir/.csv converted"));
        assert_eq!(target_path(Path::new("dir/export.csv"), true), Path::new("dir/export.csv"));
    }
}
