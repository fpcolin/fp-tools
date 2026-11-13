//! Scan to Excel from end to end: small scans of the same table as PNG, as
//! a black-and-white Group 4 TIFF (upside down) and as a JPEG inside a PDF.
//! Each is read with the built-in text recognition models.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use fp_tools::scan::{self, PageTable, values::Value};

fn data(name: &str) -> PathBuf {
    PathBuf::from(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn read(name: &str) -> PageTable {
    let cancel = AtomicBool::new(false);
    let mut seen = Vec::new();
    let mut pages = scan::read_file(&data(name), &cancel, |done, total| seen.push((done, total))).unwrap();
    assert_eq!(seen, vec![(0, 1), (1, 1)], "{name}");
    assert_eq!(pages.len(), 1);
    pages.remove(0)
}

fn cell(page: &PageTable, row: usize, col: usize) -> Option<&Value> {
    page.cells.iter().find(|c| c.row == row && c.col == col).and_then(|c| c.value.as_ref())
}

fn text(t: &str) -> Value {
    Value::Text(t.into())
}

fn check(name: &str) {
    let page = read(name);
    assert_eq!((page.rows, page.cols), (5, 3), "{name}: {page:#?}");
    assert_eq!(cell(&page, 0, 0), Some(&text("Sample order 1042")), "{name}");
    let header: Vec<Option<&Value>> = (0..3).map(|c| cell(&page, 1, c)).collect();
    assert_eq!(header, vec![Some(&text("Item")), Some(&text("Description")), Some(&text("Amount"))], "{name}");
    assert_eq!(cell(&page, 2, 0), Some(&text("FP-1001")), "{name}");
    assert_eq!(cell(&page, 2, 1), Some(&text("Carpet - Berber")), "{name}");
    assert_eq!(cell(&page, 2, 2), Some(&Value::Number(1240.5, "#,##0.00")), "{name}");
    assert_eq!(cell(&page, 3, 2), Some(&Value::Number(96.0, "0.00")), "{name}");
    // Leading zeros stay text; parentheses mean negative.
    assert_eq!(cell(&page, 4, 0), Some(&text("00315")), "{name}");
    assert_eq!(cell(&page, 4, 2), Some(&Value::Number(-12.75, "#,##0.00;(#,##0.00)")), "{name}");
    // The table's cells have borders; the title above it doesn't.
    assert!(page.cells.iter().filter(|c| c.row >= 1).all(|c| c.boxed), "{name}");
    assert!(page.cells.iter().filter(|c| c.row == 0).all(|c| !c.boxed), "{name}");
}

#[test]
fn reads_a_png() {
    check("scan-table.png");
}

#[test]
fn reads_an_upside_down_fax_tiff() {
    check("scan-table-g4.tif");
}

#[test]
fn reads_a_pdf() {
    check("scan-table.pdf");
}

#[test]
fn writes_a_workbook() {
    let page = read("scan-table.png");
    let bytes = scan::workbook(&[page.clone(), page], false);
    assert_eq!(&bytes[..4], b"PK\x03\x04");
    // Stored names are visible in the zip directory.
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"xl/worksheets/sheet2.xml"));
    assert!(has(b"xl/styles.xml"));
}

#[test]
fn refuses_other_files() {
    let cancel = AtomicBool::new(false);
    let err = scan::read_file(&data("palette.png").with_file_name("nope.pdf"), &cancel, |_, _| {}).unwrap_err();
    assert_eq!(err.to_string(), "The file isn't there any more.");
    let dir = std::env::temp_dir().join(format!("fp-scan-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("list.pdf");
    std::fs::write(&csv, b"a,b,c\n1,2,3\n").unwrap();
    let err = scan::read_file(&csv, &cancel, |_, _| {}).unwrap_err();
    assert!(err.to_string().contains("isn't a scan"), "{err}");
    std::fs::remove_dir_all(dir).unwrap();
}
