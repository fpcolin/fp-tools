//! "Scan to Excel": choose a scanned file, the tables are read in the
//! background, a preview shows what was read, and Save writes the workbook.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::canvas::RectF;
use super::layout::{Frame, Item, text_height, text_width};
use super::qr_page::{banner_layout, set_banner_controls};
use super::theme::{ButtonKind, Font, Icon, c};
use super::win32::*;
use super::{Banner, Event, Model, PageId, Ui, dialogs, ui, update_settings, widgets, with};
use crate::scan::{self, PageTable, ScanError, values::Value};

const ID_FILE: u16 = 600;
const ID_CANCEL: u16 = 601;
const ID_PREV: u16 = 602;
const ID_NEXT: u16 = 603;
const ID_ONE_SHEET: u16 = 604;
const ID_OPEN_AFTER: u16 = 605;
const ID_SAVE: u16 = 606;
const ID_SHOW: u16 = 607;

const TIPS: [&str; 4] = [
    "Scan at 300 dpi, in greyscale or colour, as a PDF, TIFF or JPEG.",
    "Pages can be a little crooked, sideways or upside down: they are straightened automatically.",
    "Printed tables work best, with or without lines. Handwriting isn't read.",
    "Check the preview, and the numbers, before relying on them.",
];

/// Rows and columns shown in the preview.
const PREVIEW_ROWS: usize = 14;

pub struct Controls {
    pub file_name: HWND,
    pub file_folder: HWND,
    pub file_button: HWND,
    pub status: HWND,
    pub cancel: HWND,
    pub prev: HWND,
    pub next: HWND,
    pub one_sheet: HWND,
    pub open_after: HWND,
    pub save: HWND,
    pub result_text: HWND,
    pub result_show: HWND,
    pub tips: [HWND; 4],
}

/// A file being read.
pub struct Running {
    pub cancel: Arc<AtomicBool>,
    pub done: usize,
    pub total: usize,
}

#[derive(Default)]
pub struct State {
    /// Numbers each reading, so a late result from an abandoned one is
    /// ignored.
    pub job: u64,
    pub running: Option<Running>,
    /// What was read, and from which file.
    pub result: Option<(PathBuf, Vec<PageTable>)>,
    /// Page shown in the preview.
    pub page: usize,
    pub banner: Option<Banner>,
}

/// Reported by the reading thread.
pub enum ScanEvent {
    Progress { job: u64, done: usize, total: usize },
    Finished { job: u64, result: Result<Vec<PageTable>, ScanError> },
}

pub fn create(page: HWND) -> Controls {
    let w = c::WHITE;
    let file_name = widgets::label_styled(page, "", Font::Bold, c::TEXT, c::SURFACE_ALT, SS_NOPREFIX | SS_ENDELLIPSIS);
    let file_folder = widgets::path_label(page, Font::Small, c::MUTED, c::SURFACE_ALT);
    let file_button = widgets::button(page, ID_FILE, "Choose file…", ButtonKind::Secondary, c::SURFACE_ALT);
    let status = widgets::label(page, "", Font::Body, c::TEXT, w);
    let cancel = widgets::button(page, ID_CANCEL, "Cancel", ButtonKind::Secondary, w);
    let prev = widgets::button(page, ID_PREV, "‹ Previous page", ButtonKind::Link, w);
    let next = widgets::button(page, ID_NEXT, "Next page ›", ButtonKind::Link, w);
    let one_sheet = widgets::checkbox(page, ID_ONE_SHEET, "Put all the pages on one sheet", w);
    let open_after = widgets::checkbox(page, ID_OPEN_AFTER, "Open it in Excel after saving", w);
    let save = widgets::button(page, ID_SAVE, "&Save as Excel…", ButtonKind::Primary, w);
    let result_text = widgets::label(page, "", Font::Body, c::SUCCESS_TEXT, c::SUCCESS_BG);
    let result_show = widgets::button(page, ID_SHOW, "Show in folder", ButtonKind::Link, c::SUCCESS_BG);
    let tips = TIPS.map(|s| widgets::label(page, s, Font::Body, c::TEXT, c::GOLD_SOFT));
    Controls {
        file_name,
        file_folder,
        file_button,
        status,
        cancel,
        prev,
        next,
        one_sheet,
        open_after,
        save,
        result_text,
        result_show,
        tips,
    }
}

fn status_text(m: &Model) -> String {
    match &m.scan.running {
        Some(r) if r.total == 0 => "Opening the file…".into(),
        Some(r) if r.total == 1 => "Reading the page…".into(),
        Some(r) => format!("Reading page {} of {}…", (r.done + 1).min(r.total), r.total),
        None => String::new(),
    }
}

pub fn sync() {
    let v = &ui().scan;
    let Some((s, status, running, result, page, banner)) = with(|m| {
        (
            m.config.get().scan.clone(),
            status_text(m),
            m.scan.running.is_some(),
            m.scan.result.as_ref().map(|(_, p)| p.len()),
            m.scan.page,
            m.scan.banner.clone(),
        )
    }) else {
        return;
    };
    let path = Path::new(&s.last_file);
    if s.last_file.is_empty() {
        widgets::set_text(v.file_name, "No file chosen yet");
        widgets::set_text(v.file_folder, "A scanned PDF, TIFF, JPEG or PNG.");
        widgets::set_text(v.file_button, "Choose file…");
    } else {
        widgets::set_text(v.file_name, &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        widgets::set_text(v.file_folder, &path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
        widgets::set_text(v.file_button, "Change file…");
    }
    widgets::set_text(v.status, &status);
    widgets::set_checked(v.one_sheet, s.option_one_sheet);
    widgets::set_checked(v.open_after, s.option_open_after);
    let pages = result.unwrap_or(0);
    widgets::enable(v.save, pages > 0 && !running);
    widgets::enable(v.one_sheet, pages > 1);
    widgets::enable(v.prev, page > 0);
    widgets::enable(v.next, page + 1 < pages);
    set_banner_controls(v.result_text, v.result_show, banner.as_ref());
}

/// The column letters, row numbers and cells of a page, painted like a
/// small spreadsheet. Returns the height used.
fn preview(f: &mut Frame, table: &PageTable, x: f32, y: f32, w: f32) -> f32 {
    let row_h = 26.0;
    let head_w = 34.0;
    let rows = table.rows.min(PREVIEW_ROWS);
    if table.cells.iter().all(|c| c.value.is_none()) {
        let h = 64.0;
        f.panel(RectF::new(x, y, w, h), c::SURFACE_ALT, c::BORDER, 8.0);
        f.text(
            RectF::new(x + 16.0, y, w - 32.0, h),
            "No text was found on this page.",
            Font::Body,
            c::MUTED,
            DT_SINGLELINE | DT_VCENTER,
        );
        return h;
    }
    // Column widths from the longest text in each, fitted to the space.
    let mut chars = vec![3usize; table.cols];
    for cell in &table.cells {
        if cell.cols == 1 && cell.row < rows {
            chars[cell.col] = chars[cell.col].max(cell.text().chars().count().min(40));
        }
    }
    let avail = w - head_w;
    let min_w = 56.0;
    let mut widths: Vec<f32> = chars.iter().map(|&n| (n as f32 * 7.5 + 16.0).max(min_w)).collect();
    let total: f32 = widths.iter().sum();
    if total > avail {
        // Shrink the wide columns first, then drop columns that won't fit.
        let scale = avail / total;
        widths.iter_mut().for_each(|wd| *wd = (*wd * scale).max(min_w));
    }
    let mut shown = 0;
    let mut used = 0.0;
    for wd in &widths {
        if used + wd > avail + 0.5 {
            break;
        }
        used += wd;
        shown += 1;
    }
    let hidden_cols = table.cols - shown;
    let grid_w = head_w + used;
    let h = row_h * (rows + 1) as f32;
    let mut xs = vec![x + head_w];
    for wd in &widths[..shown] {
        xs.push(xs[xs.len() - 1] + wd);
    }
    // Frame, headers.
    f.items.push(Item::Fill(RectF::new(x, y, grid_w, h), 0.0, c::WHITE));
    f.items.push(Item::Fill(RectF::new(x, y, grid_w, row_h), 0.0, c::SEGMENT));
    f.items.push(Item::Fill(RectF::new(x, y, head_w, h), 0.0, c::SEGMENT));
    for (i, pair) in xs.windows(2).enumerate() {
        f.text(
            RectF::new(pair[0], y, pair[1] - pair[0], row_h),
            &scan::xlsx::column_name(i),
            Font::Small,
            c::MUTED,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }
    for r in 0..rows {
        f.text(
            RectF::new(x, y + row_h * (r + 1) as f32, head_w, row_h),
            &(r + 1).to_string(),
            Font::Small,
            c::MUTED,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }
    // Grid lines.
    for r in 0..=rows + 1 {
        f.items.push(Item::Fill(RectF::new(x, y + row_h * r as f32, grid_w, 1.0), 0.0, c::BORDER));
    }
    for &cx in std::iter::once(&x).chain(&xs) {
        f.items.push(Item::Fill(RectF::new(cx, y, 1.0, h), 0.0, c::BORDER));
    }
    // Cells (merged ones painted over the lines they cover).
    for cell in &table.cells {
        if cell.row >= rows || cell.col >= shown {
            continue;
        }
        let last_col = (cell.col + cell.cols).min(shown);
        let last_row = (cell.row + cell.rows).min(rows);
        let r = RectF::new(
            xs[cell.col],
            y + row_h * (cell.row + 1) as f32,
            xs[last_col] - xs[cell.col],
            row_h * (last_row - cell.row) as f32,
        );
        if cell.cols > 1 || cell.rows > 1 {
            f.items.push(Item::Fill(RectF::new(r.x + 1.0, r.y + 1.0, r.w - 1.0, r.h - 1.0), 0.0, c::WHITE));
        }
        let text = cell.text();
        if text.is_empty() {
            continue;
        }
        let number = matches!(cell.value, Some(Value::Number(..)));
        let mut r = r;
        if !number && cell.rows == 1 {
            // Like Excel: text runs on over empty cells to its right (over
            // the lines too, so they are covered first).
            let mut end = last_col;
            while end < shown && !table.cells.iter().any(|o| o.row == cell.row && o.col == end && o.value.is_some()) {
                end += 1;
            }
            let wanted = text_width(Font::Small, &text) + 14.0;
            while end > last_col && xs[end - 1] - xs[cell.col] >= wanted {
                end -= 1;
            }
            if end > last_col {
                r.w = xs[end] - xs[cell.col];
                f.items.push(Item::Fill(RectF::new(r.x + 1.0, r.y + 1.0, r.w - 1.0, r.h - 1.0), 0.0, c::WHITE));
            }
        }
        let align = if number { DT_RIGHT } else { DT_LEFT };
        f.text(r.inset(7.0, 0.0), &text, Font::Small, c::TEXT, align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    }
    let mut more = Vec::new();
    if table.rows > rows {
        let n = table.rows - rows;
        more.push(format!("{n} more row{}", if n == 1 { "" } else { "s" }));
    }
    if hidden_cols > 0 {
        more.push(format!("{hidden_cols} more column{}", if hidden_cols == 1 { "" } else { "s" }));
    }
    if more.is_empty() {
        return h;
    }
    f.text(
        RectF::new(x, y + h + 8.0, w, 20.0),
        &format!("…and {} in the spreadsheet.", more.join(" and ")),
        Font::Small,
        c::MUTED,
        DT_SINGLELINE | DT_END_ELLIPSIS,
    );
    h + 28.0
}

pub fn layout(ui: &Ui, m: &mut Model, width: f32) -> Frame {
    let v = &ui.scan;
    let mut f = Frame::default();
    let x = 40.0;
    let avail = (width - 80.0).max(400.0);
    f.text(RectF::new(x, 28.0, avail, 36.0), "Scan to Excel", Font::Title, c::NAVY, DT_SINGLELINE);
    let sub = "Reads the tables in a scanned page and turns them into an Excel spreadsheet. Works with any scanner.";
    let sub_h = text_height(Font::Body, sub, avail);
    f.text(RectF::new(x, 68.0, avail, sub_h), sub, Font::Body, c::MUTED, DT_WORDBREAK);
    let top = 68.0 + sub_h + 24.0;

    let side_by_side = avail >= 880.0;
    let aside_w = if side_by_side { 270.0 } else { avail };
    let main_w = if side_by_side { avail - 24.0 - aside_w } else { avail };
    let (x0, iw) = (x + 28.0, main_w - 56.0);
    let mut y = top + 26.0;

    // 1. The file.
    f.text(RectF::new(x0, y, iw, 22.0), "1. Scanned file", Font::Heading, c::NAVY, DT_SINGLELINE);
    y += 34.0;
    let btn_text = widgets::text(v.file_button);
    let btn_w = text_width(Font::Body, &btn_text) + 36.0;
    f.panel(RectF::new(x0, y, iw, 74.0), c::SURFACE_ALT, c::BORDER, 10.0);
    f.items.push(Item::Icon(RectF::new(x0 + 16.0, y + 21.0, 32.0, 32.0), Icon::Table, c::NAVY));
    let text_w = iw - 64.0 - btn_w - 28.0;
    f.place(v.file_name, RectF::new(x0 + 62.0, y + 16.0, text_w, 22.0));
    f.place(v.file_folder, RectF::new(x0 + 62.0, y + 40.0, text_w, 20.0));
    f.place(v.file_button, RectF::new(x0 + iw - 16.0 - btn_w, y + 19.0, btn_w, 36.0));
    y += 74.0 + 30.0;

    // 2. Reading, or what was read.
    f.text(RectF::new(x0, y, iw, 22.0), "2. Check what was read", Font::Heading, c::NAVY, DT_SINGLELINE);
    y += 34.0;
    let running = m.scan.running.as_ref().map(|r| if r.total == 0 { 0.0 } else { r.done as f32 / r.total as f32 });
    let pages = m.scan.result.as_ref().map_or(0, |(_, p)| p.len());
    if let Some(frac) = running {
        let cancel_w = text_width(Font::Body, "Cancel") + 36.0;
        f.place(v.status, RectF::new(x0, y + 8.0, iw - cancel_w - 16.0, 22.0));
        f.place(v.cancel, RectF::new(x0 + iw - cancel_w, y, cancel_w, 36.0));
        y += 48.0;
        f.items.push(Item::Meter(RectF::new(x0, y, iw, 8.0), frac.max(0.03)));
        y += 8.0 + 30.0;
    } else {
        f.hide(v.status);
        f.hide(v.cancel);
    }
    let page_index = m.scan.page.min(pages.saturating_sub(1));
    match (&m.scan.result, running) {
        (Some((_, tables)), None) if !tables.is_empty() => {
            if pages > 1 {
                let label = format!("Page {} of {pages}", page_index + 1);
                f.text(RectF::new(x0, y + 8.0, 140.0, 20.0), &label, Font::Bold, c::TEXT, DT_SINGLELINE);
                let prev_w = text_width(Font::Bold, "‹ Previous page") + 24.0;
                let next_w = text_width(Font::Bold, "Next page ›") + 24.0;
                f.place(v.prev, RectF::new(x0 + iw - next_w - prev_w - 4.0, y, prev_w, 36.0));
                f.place(v.next, RectF::new(x0 + iw - next_w, y, next_w, 36.0));
                y += 44.0;
            } else {
                f.hide(v.prev);
                f.hide(v.next);
            }
            y += preview(&mut f, &tables[page_index], x0, y, iw) + 30.0;
        }
        _ => {
            f.hide(v.prev);
            f.hide(v.next);
            if running.is_none() {
                let note = "Choose a file above. Its tables are read on this PC; nothing is sent anywhere.";
                let h = text_height(Font::Body, note, iw);
                f.text(RectF::new(x0, y, iw, h), note, Font::Body, c::MUTED, DT_WORDBREAK);
                y += h + 30.0;
            }
        }
    }

    // 3. Save.
    f.text(RectF::new(x0, y, iw, 22.0), "3. Save", Font::Heading, c::NAVY, DT_SINGLELINE);
    y += 34.0;
    f.place(v.one_sheet, RectF::new(x0, y, iw, 24.0));
    y += 34.0;
    f.place(v.open_after, RectF::new(x0, y, iw, 24.0));
    y += 24.0 + 26.0;
    let save_w = text_width(Font::Bold, "Save as Excel…") + 64.0;
    f.place(v.save, RectF::new(x0, y, save_w, 44.0));
    y += 44.0;
    if m.scan.banner.is_some() {
        y += 20.0;
    }
    y += banner_layout(&mut f, m.scan.banner.as_ref(), v.result_text, v.result_show, x0, y, iw);
    y += 28.0;
    f.items.insert(0, Item::Panel(RectF::new(x, top, main_w, y - top), c::WHITE, c::BORDER, 12.0));
    let main_bottom = y;

    // Tips.
    let (ax, ay) = if side_by_side { (x + main_w + 24.0, top) } else { (x, main_bottom + 24.0) };
    let (ax0, aiw) = (ax + 24.0, aside_w - 48.0);
    let mut y = ay + 24.0;
    let mut aside = vec![Item::Text(
        RectF::new(ax0, y, aiw, 22.0),
        "For good results".into(),
        Font::Heading,
        c::NAVY,
        DT_SINGLELINE,
    )];
    y += 38.0;
    for (i, &label) in v.tips.iter().enumerate() {
        let h = text_height(Font::Body, TIPS[i], aiw - 38.0);
        aside.push(Item::Fill(RectF::new(ax0, y - 1.0, 24.0, 24.0), 12.0, c::NAVY));
        aside.push(Item::Text(
            RectF::new(ax0, y - 1.0, 24.0, 24.0),
            (i + 1).to_string(),
            Font::SmallBold,
            c::WHITE,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        ));
        f.place(label, RectF::new(ax0 + 38.0, y + 1.0, aiw - 38.0, h));
        y += h.max(22.0) + 16.0;
    }
    y += 8.0;
    f.items.push(Item::Panel(RectF::new(ax, ay, aside_w, y - ay), c::GOLD_SOFT, c::GOLD_SOFT_BORDER, 12.0));
    f.items.extend(aside);
    f.height = main_bottom.max(y) + 32.0;
    f
}

fn set_banner(banner: Option<Banner>) {
    let shown = banner.is_some();
    with(|m| m.scan.banner = banner);
    super::sync(PageId::Scan);
    if shown {
        super::scroll_into_view(PageId::Scan, ui().scan.result_text);
    }
}

pub fn command(id: u16, code: u16) {
    if code != BN_CLICKED {
        return;
    }
    let v = &ui().scan;
    match id {
        ID_FILE => choose_file(),
        ID_CANCEL => {
            with(|m| {
                if let Some(r) = &m.scan.running {
                    r.cancel.store(true, Ordering::Relaxed);
                }
            });
        }
        ID_PREV | ID_NEXT => {
            with(|m| {
                let pages = m.scan.result.as_ref().map_or(0, |(_, p)| p.len());
                m.scan.page = if id == ID_PREV {
                    m.scan.page.saturating_sub(1)
                } else {
                    (m.scan.page + 1).min(pages.saturating_sub(1))
                };
            });
            super::sync(PageId::Scan);
            // Keep the keyboard on a button that still does something.
            let (prev_on, next_on) = with(|m| {
                let pages = m.scan.result.as_ref().map_or(0, |(_, p)| p.len());
                (m.scan.page > 0, m.scan.page + 1 < pages)
            })
            .unwrap_or((false, false));
            if id == ID_PREV && !prev_on {
                widgets::focus(v.next);
            } else if id == ID_NEXT && !next_on {
                widgets::focus(v.prev);
            }
        }
        ID_ONE_SHEET | ID_OPEN_AFTER => {
            let (one, open) = (widgets::checked(v.one_sheet), widgets::checked(v.open_after));
            update_settings(|s| {
                s.scan.option_one_sheet = one;
                s.scan.option_open_after = open;
            });
            super::sync(PageId::Settings);
        }
        ID_SAVE => save(),
        ID_SHOW => {
            if let Some(Some(Banner::Success { path, .. })) = with(|m| m.scan.banner.clone()) {
                let _ = crate::common::shell::reveal(&path);
            }
        }
        _ => {}
    }
}

/// Enter: save when there is something to save, otherwise choose a file.
pub fn enter() {
    let ready = with(|m| m.scan.result.is_some() && m.scan.running.is_none()).unwrap_or(false);
    if ready { save() } else { choose_file() }
}

fn choose_file() {
    let Some(current) = with(|m| m.config.get().scan.last_file.clone()) else { return };
    let current = (!current.is_empty()).then(|| PathBuf::from(current));
    let Some(path) = dialogs::open_file(
        ui().main,
        "Choose the scanned file",
        &[("Scans (*.pdf, *.tif, *.jpg, *.png)", "*.pdf;*.tif;*.tiff;*.jpg;*.jpeg;*.png"), ("All files", "*.*")],
        current.as_deref(),
    ) else {
        return;
    };
    start(path);
}

/// Read `path` in the background (any reading already going is stopped).
pub fn start(path: PathBuf) {
    let text = path.to_string_lossy().into_owned();
    update_settings(|s| s.scan.last_file = text);
    let cancel = Arc::new(AtomicBool::new(false));
    let Some((job, events)) = with(|m| {
        if let Some(r) = &m.scan.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
        m.scan.job += 1;
        m.scan.running = Some(Running { cancel: cancel.clone(), done: 0, total: 0 });
        m.scan.result = None;
        m.scan.page = 0;
        m.scan.banner = None;
        (m.scan.job, m.events.clone())
    }) else {
        return;
    };
    let main = super::main_handle();
    let spawned = std::thread::Builder::new().name("scan".into()).stack_size(8 << 20).spawn(move || {
        let progress_events = events.clone();
        let result = scan::read_file(&path, &cancel, |done, total| {
            super::post(&progress_events, main, Event::Scan(ScanEvent::Progress { job, done, total }));
        });
        super::post(&events, main, Event::Scan(ScanEvent::Finished { job, result }));
    });
    if spawned.is_err() {
        with(|m| m.scan.running = None);
        set_banner(Some(Banner::Error("The file couldn't be read: out of memory.".into())));
        return;
    }
    super::sync(PageId::Scan);
    widgets::focus(ui().scan.cancel);
}

pub fn on_event(e: ScanEvent) {
    match e {
        ScanEvent::Progress { job, done, total } => {
            let current = with(|m| {
                if m.scan.job != job {
                    return false;
                }
                if let Some(r) = &mut m.scan.running {
                    r.done = done;
                    r.total = total;
                }
                true
            });
            if current == Some(true) {
                super::sync(PageId::Scan);
            }
        }
        ScanEvent::Finished { job, result } => {
            let Some(path) = with(|m| m.config.get().scan.last_file.clone()) else { return };
            let current = with(|m| {
                if m.scan.job != job {
                    return false;
                }
                m.scan.running = None;
                true
            });
            if current != Some(true) {
                return;
            }
            let had_focus = widgets::focused() == ui().scan.cancel;
            match result {
                Ok(pages) => {
                    with(|m| m.scan.result = Some((PathBuf::from(path), pages)));
                    super::sync(PageId::Scan);
                    if had_focus {
                        widgets::focus(ui().scan.save);
                    }
                }
                Err(ScanError::Cancelled) => {
                    super::sync(PageId::Scan);
                    if had_focus {
                        widgets::focus(ui().scan.file_button);
                    }
                }
                Err(ScanError::Failed(message)) => {
                    set_banner(Some(Banner::Error(message)));
                    if had_focus {
                        widgets::focus(ui().scan.file_button);
                    }
                }
            }
        }
    }
}

/// Ask where to save, then write the workbook.
pub fn save() {
    let Some(Some((source, bytes, open_after))) = with(|m| {
        let s = &m.config.get().scan;
        let (source, pages) = m.scan.result.as_ref()?;
        let one = s.option_one_sheet && pages.len() > 1;
        Some((source.clone(), scan::workbook(pages, one), s.option_open_after))
    }) else {
        return;
    };
    let folder = source.parent().map(Path::to_path_buf).unwrap_or_default();
    let folder = crate::common::paths::nearest_existing_dir(&folder).unwrap_or_else(crate::common::paths::downloads);
    let name = format!(
        "{}.xlsx",
        source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Scan".into())
    );
    let Some(target) = dialogs::save_file(
        ui().main,
        "Save as an Excel workbook",
        &[("Excel workbook (*.xlsx)", "*.xlsx")],
        &folder,
        &name,
        "xlsx",
    ) else {
        return;
    };
    let shown = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match crate::common::fsutil::write_atomic(&target, &bytes) {
        Ok(()) => {
            if open_after {
                let _ = crate::common::shell::open(&target);
            }
            set_banner(Some(Banner::Success { text: format!("Saved {shown}."), path: target }));
        }
        Err(e) => {
            let why = if matches!(e.raw_os_error(), Some(5 | 32 | 33)) {
                " It may be open in Excel: close it there and try again.".to_string()
            } else {
                format!(" ({e})")
            };
            set_banner(Some(Banner::Error(format!("{shown} couldn't be saved.{why}"))));
        }
    }
}
