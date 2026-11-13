//! Writing an Excel workbook (.xlsx): a zip of a few XML files.

use super::values::{Value, formats};
use crate::image::zlib;

/// One worksheet.
#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub name: String,
    pub cells: Vec<Cell>,
    /// Merged ranges: (first row, first column, last row, last column).
    pub merges: Vec<(usize, usize, usize, usize)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
    /// None for an empty cell that only carries a border.
    pub value: Option<Value>,
    pub border: bool,
}

/// "A", "B", ... "Z", "AA", ...
pub fn column_name(mut col: usize) -> String {
    let mut name = Vec::new();
    loop {
        name.push(b'A' + (col % 26) as u8);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}

fn cell_ref(row: usize, col: usize) -> String {
    format!("{}{}", column_name(col), row + 1)
}

/// Escape text for XML, dropping characters XML cannot contain.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// Sheet names: at most 31 characters, none of []:*?/\, unique.
pub fn sheet_name(wanted: &str, taken: &[String]) -> String {
    let clean: String = wanted.chars().filter(|c| !"[]:*?/\\".contains(*c)).take(31).collect();
    let clean = if clean.trim().is_empty() { "Sheet".to_string() } else { clean };
    let mut name = clean.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
        let suffix = format!(" ({n})");
        name = format!("{}{suffix}", clean.chars().take(31 - suffix.len()).collect::<String>());
        n += 1;
    }
    name
}

/// Built-in Excel number format ids.
fn builtin_format(code: &str) -> Option<u32> {
    Some(match code {
        "General" => 0,
        "0" => 1,
        "0.00" => 2,
        "#,##0" => 3,
        "#,##0.00" => 4,
        "0%" => 9,
        "0.00%" => 10,
        "m/d/yyyy" => 14,
        _ => return None,
    })
}

#[derive(Default)]
struct Styles {
    /// Custom number formats, numbered from 164.
    custom: Vec<String>,
    /// (number format id, border) per style; style 0 is the default.
    xfs: Vec<(u32, bool)>,
}

impl Styles {
    fn new() -> Self {
        Styles { custom: Vec::new(), xfs: vec![(0, false)] }
    }

    fn style(&mut self, format: &str, border: bool) -> usize {
        let id = builtin_format(format).unwrap_or_else(|| {
            let i = match self.custom.iter().position(|c| c == format) {
                Some(i) => i,
                None => {
                    self.custom.push(format.to_string());
                    self.custom.len() - 1
                }
            };
            164 + i as u32
        });
        match self.xfs.iter().position(|&x| x == (id, border)) {
            Some(i) => i,
            None => {
                self.xfs.push((id, border));
                self.xfs.len() - 1
            }
        }
    }

    fn xml(&self) -> String {
        let mut s = String::from(XML_HEAD);
        s.push_str(r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#);
        if !self.custom.is_empty() {
            s.push_str(&format!(r#"<numFmts count="{}">"#, self.custom.len()));
            for (i, code) in self.custom.iter().enumerate() {
                s.push_str(&format!(r#"<numFmt numFmtId="{}" formatCode="{}"/>"#, 164 + i, escape(code)));
            }
            s.push_str("</numFmts>");
        }
        s.push_str(concat!(
            r#"<fonts count="1"><font><sz val="11"/><color theme="1"/><name val="Calibri"/><family val="2"/><scheme val="minor"/></font></fonts>"#,
            r#"<fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>"#,
            r#"<borders count="2"><border><left/><right/><top/><bottom/><diagonal/></border>"#,
            r#"<border><left style="thin"><color indexed="64"/></left><right style="thin"><color indexed="64"/></right>"#,
            r#"<top style="thin"><color indexed="64"/></top><bottom style="thin"><color indexed="64"/></bottom><diagonal/></border></borders>"#,
            r#"<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>"#,
        ));
        s.push_str(&format!(r#"<cellXfs count="{}">"#, self.xfs.len()));
        for &(fmt, border) in &self.xfs {
            s.push_str(&format!(
                r#"<xf numFmtId="{fmt}" fontId="0" fillId="0" borderId="{}" xfId="0"{}{}/>"#,
                u8::from(border),
                if fmt != 0 { r#" applyNumberFormat="1""# } else { "" },
                if border { r#" applyBorder="1""# } else { "" },
            ));
        }
        s.push_str("</cellXfs>");
        s.push_str(r#"<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>"#);
        s.push_str("</styleSheet>");
        s
    }
}

const XML_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";
const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Column width in Excel's units (characters) for text of `chars` length.
fn width_for(chars: usize) -> f32 {
    (chars as f32 * 1.1 + 2.0).clamp(8.43, 60.0)
}

fn sheet_xml(sheet: &Sheet, styles: &mut Styles) -> String {
    let mut cells: Vec<&Cell> = sheet.cells.iter().collect();
    cells.sort_by_key(|c| (c.row, c.col));
    let max_row = cells.iter().map(|c| c.row).max().unwrap_or(0);
    let max_col = cells.iter().map(|c| c.col).max().unwrap_or(0);

    // Widths from the longest text in each column (merged cells excluded).
    let mut widths = vec![0usize; max_col + 1];
    for c in &cells {
        let merged = sheet.merges.iter().any(|&(r0, c0, r1, c1)| c.row == r0 && c.col == c0 && c1 > c0 && r1 >= r0);
        if merged {
            continue;
        }
        let len = match &c.value {
            Some(Value::Text(t)) => t.chars().count(),
            Some(Value::Number(v, f)) => {
                if formats::DATES.contains(f) {
                    10
                } else {
                    format!("{v}").len() + 2
                }
            }
            None => 0,
        };
        widths[c.col] = widths[c.col].max(len);
    }

    let mut s = String::from(XML_HEAD);
    s.push_str(&format!(r#"<worksheet xmlns="{MAIN_NS}" xmlns:r="{REL_NS}">"#));
    let dimension = if cells.is_empty() { "A1".to_string() } else { format!("A1:{}", cell_ref(max_row, max_col)) };
    s.push_str(&format!(r#"<dimension ref="{dimension}"/>"#));
    s.push_str(r#"<sheetViews><sheetView workbookViewId="0"/></sheetViews>"#);
    s.push_str(r#"<sheetFormatPr defaultRowHeight="15"/>"#);
    if !cells.is_empty() {
        s.push_str("<cols>");
        for (i, &w) in widths.iter().enumerate() {
            s.push_str(&format!(r#"<col min="{0}" max="{0}" width="{1:.2}" customWidth="1"/>"#, i + 1, width_for(w)));
        }
        s.push_str("</cols>");
    }
    s.push_str("<sheetData>");
    let mut current_row = usize::MAX;
    for c in &cells {
        if c.row != current_row {
            if current_row != usize::MAX {
                s.push_str("</row>");
            }
            current_row = c.row;
            s.push_str(&format!(r#"<row r="{}">"#, c.row + 1));
        }
        let r = cell_ref(c.row, c.col);
        match &c.value {
            Some(Value::Text(t)) => {
                let style = styles.style(formats::GENERAL, c.border);
                let space = if t.starts_with(' ') || t.ends_with(' ') { r#" xml:space="preserve""# } else { "" };
                s.push_str(&format!(
                    r#"<c r="{r}" s="{style}" t="inlineStr"><is><t{space}>{}</t></is></c>"#,
                    escape(t)
                ));
            }
            Some(Value::Number(v, format)) => {
                let style = styles.style(format, c.border);
                s.push_str(&format!(r#"<c r="{r}" s="{style}"><v>{v}</v></c>"#));
            }
            None => {
                let style = styles.style(formats::GENERAL, c.border);
                s.push_str(&format!(r#"<c r="{r}" s="{style}"/>"#));
            }
        }
    }
    if current_row != usize::MAX {
        s.push_str("</row>");
    }
    s.push_str("</sheetData>");
    if !sheet.merges.is_empty() {
        s.push_str(&format!(r#"<mergeCells count="{}">"#, sheet.merges.len()));
        for &(r0, c0, r1, c1) in &sheet.merges {
            s.push_str(&format!(r#"<mergeCell ref="{}:{}"/>"#, cell_ref(r0, c0), cell_ref(r1, c1)));
        }
        s.push_str("</mergeCells>");
    }
    s.push_str(r#"<pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/>"#);
    s.push_str("</worksheet>");
    s
}

/// The workbook file's bytes.
pub fn workbook(sheets: &[Sheet]) -> Vec<u8> {
    let mut styles = Styles::new();
    let sheet_files: Vec<String> = sheets.iter().map(|sh| sheet_xml(sh, &mut styles)).collect();

    let mut types = String::from(XML_HEAD);
    types.push_str(concat!(
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
        r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
        r#"<Default Extension="xml" ContentType="application/xml"/>"#,
        r#"<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#,
        r#"<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#,
        r#"<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#,
    ));
    for i in 1..=sheets.len() {
        types.push_str(&format!(
            r#"<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#
        ));
    }
    types.push_str("</Types>");

    let root_rels = format!(
        concat!(
            "{}",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
            r#"<Relationship Id="rId1" Type="{}/officeDocument" Target="xl/workbook.xml"/>"#,
            r#"<Relationship Id="rId2" Type="{}/extended-properties" Target="docProps/app.xml"/>"#,
            "</Relationships>"
        ),
        XML_HEAD, REL_NS, REL_NS
    );
    let app = format!(
        concat!(
            "{}",
            r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">"#,
            "<Application>{}</Application></Properties>"
        ),
        XML_HEAD,
        escape(crate::app::DISPLAY_NAME)
    );

    let mut book = String::from(XML_HEAD);
    book.push_str(&format!(
        r#"<workbook xmlns="{MAIN_NS}" xmlns:r="{REL_NS}"><bookViews><workbookView/></bookViews><sheets>"#
    ));
    for (i, sh) in sheets.iter().enumerate() {
        book.push_str(&format!(r#"<sheet name="{}" sheetId="{}" r:id="rId{}"/>"#, escape(&sh.name), i + 1, i + 1));
    }
    book.push_str("</sheets></workbook>");

    let mut book_rels = String::from(XML_HEAD);
    book_rels.push_str(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    for i in 1..=sheets.len() {
        book_rels.push_str(&format!(
            r#"<Relationship Id="rId{i}" Type="{REL_NS}/worksheet" Target="worksheets/sheet{i}.xml"/>"#
        ));
    }
    book_rels.push_str(&format!(
        r#"<Relationship Id="rId{}" Type="{REL_NS}/styles" Target="styles.xml"/>"#,
        sheets.len() + 1
    ));
    book_rels.push_str("</Relationships>");

    let mut zip = Zip::default();
    zip.add("[Content_Types].xml", types.as_bytes());
    zip.add("_rels/.rels", root_rels.as_bytes());
    zip.add("docProps/app.xml", app.as_bytes());
    zip.add("xl/workbook.xml", book.as_bytes());
    zip.add("xl/_rels/workbook.xml.rels", book_rels.as_bytes());
    zip.add("xl/styles.xml", styles.xml().as_bytes());
    for (i, xml) in sheet_files.iter().enumerate() {
        zip.add(&format!("xl/worksheets/sheet{}.xml", i + 1), xml.as_bytes());
    }
    zip.finish()
}

/// A minimal zip writer (deflate, no zip64; workbooks are small).
#[derive(Default)]
struct Zip {
    out: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

impl Zip {
    fn add(&mut self, name: &str, data: &[u8]) {
        let zlib_stream = zlib::deflate_zlib(data);
        // Raw deflate: without the 2-byte zlib header and Adler-32 trailer.
        let deflated = &zlib_stream[2..zlib_stream.len() - 4];
        let crc = zlib::crc32(data);
        let (time, date) = dos_time();
        let offset = self.out.len() as u32;
        let mut header = Vec::new();
        header.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        header.extend_from_slice(&20u16.to_le_bytes()); // version needed
        header.extend_from_slice(&0u16.to_le_bytes()); // flags
        header.extend_from_slice(&8u16.to_le_bytes()); // deflate
        header.extend_from_slice(&time.to_le_bytes());
        header.extend_from_slice(&date.to_le_bytes());
        header.extend_from_slice(&crc.to_le_bytes());
        header.extend_from_slice(&(deflated.len() as u32).to_le_bytes());
        header.extend_from_slice(&(data.len() as u32).to_le_bytes());
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // extra
        self.out.extend_from_slice(&header);
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(deflated);

        let c = &mut self.central;
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes()); // made by
        c.extend_from_slice(&header[4..30]); // shared fields
        c.extend_from_slice(&0u16.to_le_bytes()); // comment
        c.extend_from_slice(&0u16.to_le_bytes()); // disk
        c.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        c.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        c.extend_from_slice(&offset.to_le_bytes());
        c.extend_from_slice(name.as_bytes());
        self.count += 1;
    }

    fn finish(mut self) -> Vec<u8> {
        let start = self.out.len() as u32;
        let size = self.central.len() as u32;
        self.out.extend_from_slice(&self.central);
        self.out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        self.out.extend_from_slice(&[0, 0, 0, 0]);
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&size.to_le_bytes());
        self.out.extend_from_slice(&start.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        self.out
    }
}

/// The current time (UTC) in MS-DOS format, for the zip entries.
fn dos_time() -> (u16, u16) {
    let secs =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970 (inverse of days_from_civil).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let y = y.clamp(1980, 2107);
    let time = (((rem / 3600) << 11) | ((rem % 3600 / 60) << 5) | ((rem % 60) / 2)) as u16;
    let date = (((y - 1980) << 9) | (m << 5) | d) as u16;
    (time, date)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_names() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(27), "AB");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
    }

    #[test]
    fn sheet_names_are_valid_and_unique() {
        assert_eq!(sheet_name("Page 1", &[]), "Page 1");
        assert_eq!(sheet_name("a/b:c", &[]), "abc");
        assert_eq!(sheet_name("Page 1", &["page 1".into()]), "Page 1 (2)");
        assert_eq!(sheet_name(&"x".repeat(40), &[]).len(), 31);
    }

    #[test]
    fn writes_a_zip_with_the_parts() {
        let sheet = Sheet {
            name: "Page 1".into(),
            cells: vec![
                Cell { row: 0, col: 0, value: Some(Value::Text("Item & <b>".into())), border: true },
                Cell { row: 0, col: 1, value: Some(Value::Number(1608.8, "#,##0.00")), border: true },
                Cell { row: 1, col: 0, value: Some(Value::Number(0.5, "\"$\"#,##0.00")), border: false },
            ],
            merges: vec![(1, 0, 1, 1)],
        };
        let bytes = workbook(&[sheet]);
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        // End of central directory with 7 entries.
        let eocd = &bytes[bytes.len() - 22..];
        assert_eq!(&eocd[..4], &0x0605_4b50u32.to_le_bytes());
        assert_eq!(u16::from_le_bytes([eocd[10], eocd[11]]), 7);
    }
}
