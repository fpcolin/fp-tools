//! "VendorCafe converter": choose the export, say whether the customer uses
//! PO or work order numbers, convert.

use std::path::{Path, PathBuf};

use super::canvas::RectF;
use super::layout::{Frame, Item, text_height, text_width};
use super::qr_page::{banner_layout, set_banner_controls};
use super::theme::{self, ButtonKind, Font, Icon, c};
use super::win32::*;
use super::{Banner, Model, PageId, Ui, dialogs, ui, update_settings, widgets, with};
use crate::converter::{self, Mode};

const ID_FILE: u16 = 300;
const ID_PO: u16 = 301;
const ID_WO: u16 = 302;
const ID_OVERWRITE: u16 = 303;
const ID_OPEN_FOLDER: u16 = 304;
const ID_CONVERT: u16 = 305;
const ID_SHOW: u16 = 306;

const PO_HINT: &str = "Keeps PO No and Job No";
const WO_HINT: &str = "PO No becomes WO No, Job No is dropped";
const OVERWRITE_HINT: &str = "Untick to save a separate “… converted.csv” next to it instead.";
const STEPS: [&str; 4] = [
    "In Rollmaster, export the VendorCafe invoices to a CSV file.",
    "Don't open or edit the file before converting it.",
    "Choose the file here, pick PO number or Work order number for this customer, and click Convert.",
    "Upload the converted file to VendorCafe.",
];

pub struct Controls {
    pub file_name: HWND,
    pub file_folder: HWND,
    pub file_button: HWND,
    pub po: HWND,
    pub po_hint: HWND,
    pub wo: HWND,
    pub wo_hint: HWND,
    pub overwrite: HWND,
    pub overwrite_hint: HWND,
    pub open_folder: HWND,
    pub convert: HWND,
    pub result_text: HWND,
    pub result_show: HWND,
    pub steps: [HWND; 4],
}

#[derive(Default)]
pub struct State {
    /// Chosen for this session only: like the old app's two buttons, the
    /// choice is made each time rather than remembered.
    pub mode: Option<Mode>,
    pub banner: Option<Banner>,
}

pub fn create(page: HWND) -> Controls {
    let w = c::WHITE;
    let file_name = widgets::label_styled(page, "", Font::Bold, c::TEXT, c::SURFACE_ALT, SS_NOPREFIX | SS_ENDELLIPSIS);
    let file_folder = widgets::path_label(page, Font::Small, c::MUTED, c::SURFACE_ALT);
    let file_button = widgets::button(page, ID_FILE, "Choose file…", ButtonKind::Secondary, c::SURFACE_ALT);
    // The two radio buttons must be next to each other in creation order,
    // and the group must end right after them, for the arrow keys to move
    // between them.
    let po = widgets::radio(page, ID_PO, "PO number", true, w);
    let wo = widgets::radio(page, ID_WO, "Work order number", false, w);
    // Standard radio buttons "click" themselves when they get the focus.
    // Here the choice must be deliberate every time, so turn that off.
    for radio in [po, wo] {
        // SAFETY: live radio buttons.
        unsafe { SendMessageW(radio, BM_SETDONTCLICK, 1, 0) };
    }
    let po_hint = widgets::label_styled(page, PO_HINT, Font::Small, c::MUTED, w, SS_NOPREFIX | WS_GROUP);
    let wo_hint = widgets::label(page, WO_HINT, Font::Small, c::MUTED, w);
    let overwrite = widgets::checkbox(page, ID_OVERWRITE, "Replace the exported file", w);
    let overwrite_hint = widgets::label(page, OVERWRITE_HINT, Font::Small, c::MUTED, w);
    let open_folder = widgets::checkbox(page, ID_OPEN_FOLDER, "Show the file in its folder afterwards", w);
    let convert = widgets::button(page, ID_CONVERT, "&Convert", ButtonKind::Primary, w);
    let result_text = widgets::label(page, "", Font::Body, c::SUCCESS_TEXT, c::SUCCESS_BG);
    let result_show = widgets::button(page, ID_SHOW, "Show in folder", ButtonKind::Link, c::SUCCESS_BG);
    let steps = STEPS.map(|s| widgets::label(page, s, Font::Body, c::TEXT, c::GOLD_SOFT));
    Controls {
        file_name,
        file_folder,
        file_button,
        po,
        po_hint,
        wo,
        wo_hint,
        overwrite,
        overwrite_hint,
        open_folder,
        convert,
        result_text,
        result_show,
        steps,
    }
}

pub fn sync() {
    let v = &ui().vc;
    let Some((s, state)) = with(|m| (m.config.get().vc.clone(), (m.vc.mode, m.vc.banner.clone()))) else { return };
    let (mode, banner) = state;
    let path = Path::new(&s.filepath);
    if s.filepath.is_empty() {
        widgets::set_text(v.file_name, "No file chosen yet");
        widgets::set_text(v.file_folder, "Choose the CSV file you exported from Rollmaster.");
        widgets::set_text(v.file_button, "Choose file…");
    } else {
        widgets::set_text(v.file_name, &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        widgets::set_text(v.file_folder, &path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
        widgets::set_text(v.file_button, "Change file…");
    }
    widgets::set_checked(v.po, mode == Some(Mode::PurchaseOrder));
    widgets::set_checked(v.wo, mode == Some(Mode::WorkOrder));
    // Tab reaches the checked choice, or the first one while neither is.
    widgets::set_tabstop(v.po, mode != Some(Mode::WorkOrder));
    widgets::set_tabstop(v.wo, mode == Some(Mode::WorkOrder));
    for (radio, hint, on) in
        [(v.po, v.po_hint, mode == Some(Mode::PurchaseOrder)), (v.wo, v.wo_hint, mode == Some(Mode::WorkOrder))]
    {
        let bg = if on { c::SELECTED } else { c::WHITE };
        theme::set_colors(radio, c::TEXT, bg);
        theme::set_colors(hint, c::MUTED, bg);
    }
    widgets::set_checked(v.overwrite, s.option_overwrite);
    widgets::set_checked(v.open_folder, s.option_open_folder);
    set_banner_controls(v.result_text, v.result_show, banner.as_ref());
}

pub fn layout(ui: &Ui, m: &mut Model, width: f32) -> Frame {
    let v = &ui.vc;
    let mut f = Frame::default();
    let x = 40.0;
    let avail = (width - 80.0).max(400.0);
    f.text(RectF::new(x, 28.0, avail, 36.0), "VendorCafe converter", Font::Title, c::NAVY, DT_SINGLELINE);
    let sub = "Turns a Rollmaster invoice export into one line per invoice, ready to upload to VendorCafe.";
    let sub_h = text_height(Font::Body, sub, avail);
    f.text(RectF::new(x, 68.0, avail, sub_h), sub, Font::Body, c::MUTED, DT_WORDBREAK);
    let top = 68.0 + sub_h + 24.0;

    let side_by_side = avail >= 820.0;
    let aside_w = if side_by_side { 290.0 } else { avail };
    let main_w = if side_by_side { avail - 24.0 - aside_w } else { avail };
    let (x0, iw) = (x + 28.0, main_w - 56.0);
    let mut y = top + 26.0;

    // 1. The file.
    f.text(RectF::new(x0, y, iw, 22.0), "1. Exported file", Font::Heading, c::NAVY, DT_SINGLELINE);
    y += 34.0;
    let btn_text = widgets::text(v.file_button);
    let btn_w = text_width(Font::Body, &btn_text) + 36.0;
    let file_box = RectF::new(x0, y, iw, 74.0);
    f.panel(file_box, c::SURFACE_ALT, c::BORDER, 10.0);
    f.items.push(Item::Icon(RectF::new(x0 + 16.0, y + 21.0, 32.0, 32.0), Icon::File, c::NAVY));
    let text_w = iw - 64.0 - btn_w - 28.0;
    f.place(v.file_name, RectF::new(x0 + 62.0, y + 16.0, text_w, 22.0));
    f.place(v.file_folder, RectF::new(x0 + 62.0, y + 40.0, text_w, 20.0));
    f.place(v.file_button, RectF::new(x0 + iw - 16.0 - btn_w, y + 19.0, btn_w, 36.0));
    y += 74.0 + 30.0;

    // 2. PO or WO.
    f.text(RectF::new(x0, y, iw, 22.0), "2. This customer uses a…", Font::Heading, c::NAVY, DT_SINGLELINE);
    y += 34.0;
    let two = iw >= 460.0;
    let card_w = if two { (iw - 16.0) / 2.0 } else { iw };
    let hint_w = card_w - 54.0;
    let hint_h = text_height(Font::Small, PO_HINT, hint_w).max(text_height(Font::Small, WO_HINT, hint_w));
    let card_h = 41.0 + hint_h + 14.0;
    let cards = [(v.po, v.po_hint, Mode::PurchaseOrder, ID_PO), (v.wo, v.wo_hint, Mode::WorkOrder, ID_WO)];
    for (i, (radio, hint, mode, id)) in cards.into_iter().enumerate() {
        let (cx, cy) = if two { (x0 + i as f32 * (card_w + 16.0), y) } else { (x0, y + i as f32 * (card_h + 14.0)) };
        let on = m.vc.mode == Some(mode);
        let card = RectF::new(cx, cy, card_w, card_h);
        f.panel(card, if on { c::SELECTED } else { c::WHITE }, if on { c::NAVY } else { c::BORDER }, 10.0);
        if on {
            f.items.push(Item::Panel(card.inset(1.0, 1.0), c::SELECTED, c::NAVY, 9.0));
        }
        f.place(radio, RectF::new(cx + 16.0, cy + 14.0, card_w - 32.0, 24.0));
        f.place(hint, RectF::new(cx + 38.0, cy + 41.0, hint_w, hint_h));
        f.click(card, id);
    }
    y += if two { card_h } else { card_h * 2.0 + 14.0 } + 30.0;

    // 3. Options.
    f.text(RectF::new(x0, y, iw, 22.0), "3. Options", Font::Heading, c::NAVY, DT_SINGLELINE);
    y += 34.0;
    f.place(v.overwrite, RectF::new(x0, y, iw, 24.0));
    y += 26.0;
    let hint_h = text_height(Font::Small, OVERWRITE_HINT, iw - 24.0);
    f.place(v.overwrite_hint, RectF::new(x0 + 24.0, y, iw - 24.0, hint_h));
    y += hint_h + 14.0;
    f.place(v.open_folder, RectF::new(x0, y, iw, 24.0));
    y += 24.0 + 30.0;

    let convert_w = text_width(Font::Bold, "Convert") + 64.0;
    f.place(v.convert, RectF::new(x0, y, convert_w, 44.0));
    y += 44.0;
    if m.vc.banner.is_some() {
        y += 20.0;
    }
    y += banner_layout(&mut f, m.vc.banner.as_ref(), v.result_text, v.result_show, x0, y, iw);
    y += 28.0;
    f.items.insert(0, Item::Panel(RectF::new(x, top, main_w, y - top), c::WHITE, c::BORDER, 12.0));
    let main_bottom = y;

    // Before you start.
    let (ax, ay) = if side_by_side { (x + main_w + 24.0, top) } else { (x, main_bottom + 24.0) };
    let (ax0, aiw) = (ax + 24.0, aside_w - 48.0);
    let mut y = ay + 24.0;
    let mut aside = vec![Item::Text(
        RectF::new(ax0, y, aiw, 22.0),
        "Before you start".into(),
        Font::Heading,
        c::NAVY,
        DT_SINGLELINE,
    )];
    y += 38.0;
    for (i, &label) in v.steps.iter().enumerate() {
        let h = text_height(Font::Body, STEPS[i], aiw - 38.0);
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

/// Arrow keys switch between the two radio buttons (done here rather than
/// by the dialog manager, which does not handle them reliably inside nested
/// child windows).
pub fn arrow_key(key: usize) -> bool {
    const VK_LEFT: usize = 0x25;
    const VK_DOWN: usize = 0x28;
    let v = &ui().vc;
    let focus = widgets::focused();
    if !(VK_LEFT..=VK_DOWN).contains(&key) || (focus != v.po && focus != v.wo) {
        return false;
    }
    let other = if focus == v.po { v.wo } else { v.po };
    widgets::focus(other);
    // SAFETY: live radio button; BM_CLICK sends the usual BN_CLICKED.
    unsafe { SendMessageW(other, BM_CLICK, 0, 0) };
    true
}

fn set_banner(banner: Option<Banner>) {
    let shown = banner.is_some();
    with(|m| m.vc.banner = banner);
    super::sync(PageId::Converter);
    if shown {
        super::scroll_into_view(PageId::Converter, ui().vc.result_text);
    }
}

pub fn command(id: u16, code: u16) {
    if code != BN_CLICKED {
        return;
    }
    let v = &ui().vc;
    match id {
        ID_FILE => choose_file(),
        ID_PO | ID_WO => {
            let mode = if id == ID_PO { Mode::PurchaseOrder } else { Mode::WorkOrder };
            with(|m| {
                m.vc.mode = Some(mode);
                if matches!(m.vc.banner, Some(Banner::Error(_))) {
                    m.vc.banner = None;
                }
            });
            super::sync(PageId::Converter);
            widgets::focus(if id == ID_PO { v.po } else { v.wo });
        }
        ID_OVERWRITE => {
            let on = widgets::checked(v.overwrite);
            update_settings(|s| s.vc.option_overwrite = on);
            super::sync(PageId::Settings);
        }
        ID_OPEN_FOLDER => {
            let on = widgets::checked(v.open_folder);
            update_settings(|s| s.vc.option_open_folder = on);
            super::sync(PageId::Settings);
        }
        ID_CONVERT => convert(),
        ID_SHOW => {
            if let Some(Some(Banner::Success { path, .. })) = with(|m| m.vc.banner.clone()) {
                let _ = crate::common::shell::reveal(&path);
            }
        }
        _ => {}
    }
}

fn choose_file() {
    let Some(current) = with(|m| m.config.get().vc.filepath.clone()) else { return };
    let current = (!current.is_empty()).then(|| PathBuf::from(current));
    let Some(path) = dialogs::open_file(
        ui().main,
        "Choose the exported CSV file",
        &[("CSV files (*.csv)", "*.csv"), ("All files", "*.*")],
        current.as_deref(),
    ) else {
        return;
    };
    let path = path.to_string_lossy().into_owned();
    update_settings(|s| s.vc.filepath = path);
    with(|m| m.vc.banner = None);
    super::sync(PageId::Converter);
    widgets::focus(ui().vc.po);
}

pub fn convert() {
    let Some((settings, mode)) = with(|m| (m.config.get().vc.clone(), m.vc.mode)) else { return };
    if settings.filepath.is_empty() {
        set_banner(Some(Banner::Error("Choose the exported file first.".into())));
        widgets::focus(ui().vc.file_button);
        return;
    }
    let Some(mode) = mode else {
        set_banner(Some(Banner::Error("Choose whether this customer uses a PO number or a work order number.".into())));
        widgets::focus(ui().vc.po);
        return;
    };
    match converter::convert_selected(&settings, mode) {
        Ok(outcome) => {
            let kind = match outcome.mode {
                Mode::PurchaseOrder => "PO number",
                Mode::WorkOrder => "work order number",
            };
            let name = outcome.target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let text = format!(
                "Done: {} rows became {} invoices ({kind}). Saved as {name}.",
                outcome.rows_in, outcome.rows_out
            );
            if settings.option_open_folder {
                let _ = crate::common::shell::reveal(&outcome.target);
            }
            set_banner(Some(Banner::Success { text, path: outcome.target }));
        }
        Err(e) => set_banner(Some(Banner::Error(e.to_string()))),
    }
}
