//! "QR codes": a web address or a contact card, a live preview, and the
//! look and save options.

use std::path::PathBuf;
use std::rc::Rc;

use super::canvas::{Bitmap, RectF};
use super::layout::{Frame, Item, text_height, text_width};
use super::theme::{self, ButtonKind, Font, Icon, c};
use super::win32::*;
use super::{Banner, Model, PageId, Ui, dialogs, message, ui, update_settings, widgets, with};
use crate::common::paths;
use crate::config::{DEFAULT_LOGO, QrKind};
use crate::generator::{self, GenerateError};
use crate::image::Image;
use crate::vcard::{Contact, VCardError};

const ID_SEG_URL: u16 = 200;
const ID_SEG_CONTACT: u16 = 201;
const ID_URL: u16 = 202;
const ID_CONTACT_FIRST: u16 = 210;
const ID_LOGO: u16 = 220;
const ID_LOGO_CHANGE: u16 = 221;
const ID_ROUND: u16 = 222;
const ID_SAVE_CHANGE: u16 = 223;
const ID_OPEN_IMAGE: u16 = 224;
const ID_OPEN_FOLDER: u16 = 225;
const ID_SAVE: u16 = 226;
const ID_CLEAR: u16 = 227;
const ID_SHOW: u16 = 228;

/// Contact fields in grid order, with their keyboard shortcuts.
const CONTACT_LABELS: [&str; 8] =
    ["&First name", "&Last name", "Compan&y", "&Job title", "&Mobile", "&Work phone", "&Email", "Websi&te"];

/// Preview side in DIPs (it shrinks in a narrow window).
const PREVIEW_MAX: f32 = 260.0;

pub struct Controls {
    pub seg_url: HWND,
    pub seg_contact: HWND,
    pub url_label: HWND,
    pub url: HWND,
    pub contact_labels: [HWND; 8],
    pub contact: [HWND; 8],
    pub contact_hint: HWND,
    pub logo: HWND,
    pub logo_change: HWND,
    pub round: HWND,
    pub save_path: HWND,
    pub save_change: HWND,
    pub open_image: HWND,
    pub open_folder: HWND,
    pub save: HWND,
    pub clear: HWND,
    pub result_text: HWND,
    pub result_show: HWND,
}

pub enum Preview {
    /// Nothing typed yet: a hint.
    Empty(&'static str),
    /// Something to fix: shown in red.
    Problem(String),
    Ready(Rc<Image>),
}

/// A decoded logo (None = none to embed) or why it could not be used.
pub type LogoResult = Result<Option<Rc<Image>>, String>;

pub struct State {
    pub kind: QrKind,
    pub preview: Preview,
    /// The preview scaled for the screen, with its size in pixels.
    pub preview_bmp: Option<(i32, Rc<Bitmap>)>,
    /// Decoded logo for the preview, keyed by the setting it came from.
    pub logo: Option<(String, LogoResult)>,
    pub banner: Option<Banner>,
}

impl State {
    pub fn new(kind: QrKind) -> State {
        State { kind, preview: Preview::Empty(empty_hint(kind)), preview_bmp: None, logo: None, banner: None }
    }
}

fn empty_hint(kind: QrKind) -> &'static str {
    match kind {
        QrKind::Url => "Type a web address to see the QR code here.",
        QrKind::Contact => "Enter a first and last name to see the QR code here.",
    }
}

pub fn create(page: HWND) -> Controls {
    let w = c::WHITE;
    let seg_url = widgets::button(page, ID_SEG_URL, "Web address", ButtonKind::Segment, c::SEGMENT);
    let seg_contact = widgets::button(page, ID_SEG_CONTACT, "Contact card", ButtonKind::Segment, c::SEGMENT);
    let url_label = widgets::mnemonic_label(page, "&Web address", Font::Bold, c::TEXT, w);
    let url = widgets::edit(page, ID_URL, 4000);
    widgets::set_cue(url, "https://www.example.com");
    let mut contact_labels = [std::ptr::null_mut(); 8];
    let mut contact = [std::ptr::null_mut(); 8];
    for i in 0..8 {
        contact_labels[i] = widgets::mnemonic_label(page, CONTACT_LABELS[i], Font::Bold, c::TEXT, w);
        contact[i] = widgets::edit(page, ID_CONTACT_FIRST + i as u16, 200);
    }
    let contact_hint = widgets::label(
        page,
        "First and last name are required. Blank fields are left off the card. Phone numbers can use digits, spaces and + - ( ).",
        Font::Small,
        c::MUTED,
        w,
    );
    let logo = widgets::checkbox(page, ID_LOGO, "Put a logo in the middle", w);
    let logo_change = widgets::button(page, ID_LOGO_CHANGE, "Change logo…", ButtonKind::Link, w);
    let round = widgets::checkbox(page, ID_ROUND, "Rounded corners", w);
    let save_path = widgets::path_label(page, Font::Body, c::TEXT, w);
    let save_change = widgets::button(page, ID_SAVE_CHANGE, "Change…", ButtonKind::Link, w);
    let open_image = widgets::checkbox(page, ID_OPEN_IMAGE, "Open the image after saving", w);
    let open_folder = widgets::checkbox(page, ID_OPEN_FOLDER, "Show it in its folder", w);
    let save = widgets::button(page, ID_SAVE, "&Save QR code", ButtonKind::Primary, w);
    let clear = widgets::button(page, ID_CLEAR, "&Clear", ButtonKind::Secondary, w);
    let result_text = widgets::label(page, "", Font::Body, c::SUCCESS_TEXT, c::SUCCESS_BG);
    let result_show = widgets::button(page, ID_SHOW, "Show in folder", ButtonKind::Link, c::SUCCESS_BG);
    Controls {
        seg_url,
        seg_contact,
        url_label,
        url,
        contact_labels,
        contact,
        contact_hint,
        logo,
        logo_change,
        round,
        save_path,
        save_change,
        open_image,
        open_folder,
        save,
        clear,
        result_text,
        result_show,
    }
}

pub fn first_control() -> HWND {
    let q = &ui().qr;
    match with(|m| m.qr.kind) {
        Some(QrKind::Contact) => q.contact[0],
        _ => q.url,
    }
}

/// Model -> controls.
pub fn sync() {
    let q = &ui().qr;
    let Some((kind, s, banner)) = with(|m| (m.qr.kind, m.config.get().qr.clone(), m.qr.banner.clone())) else { return };
    theme::set_selected(q.seg_url, kind == QrKind::Url);
    theme::set_selected(q.seg_contact, kind == QrKind::Contact);
    widgets::set_checked(q.logo, s.option_embed_logo);
    widgets::set_checked(q.round, s.option_round_edges);
    widgets::set_checked(q.open_image, s.option_open_image);
    widgets::set_checked(q.open_folder, s.option_open_folder);
    widgets::set_text(q.save_path, &save_target(&s.saveto_path, &s.saveto_name));
    widgets::enable(q.logo_change, s.option_embed_logo);
    set_banner_controls(q.result_text, q.result_show, banner.as_ref());
}

/// Colours and text of a result banner's label and button.
pub fn set_banner_controls(text: HWND, show: HWND, banner: Option<&Banner>) {
    match banner {
        Some(Banner::Success { text: t, .. }) => {
            theme::set_colors(text, c::SUCCESS_TEXT, c::SUCCESS_BG);
            theme::set_button_kind(show, ButtonKind::Link, c::SUCCESS_BG);
            widgets::set_text(text, t);
        }
        Some(Banner::Error(t)) => {
            theme::set_colors(text, c::ERROR_TEXT, c::ERROR_BG);
            widgets::set_text(text, t);
        }
        None => widgets::set_text(text, ""),
    }
}

/// `<folder>\<name>.png` as shown under "Save".
pub fn save_target(folder: &str, name: &str) -> String {
    let folder = if folder.trim().is_empty() { paths::downloads() } else { PathBuf::from(folder) };
    folder.join(format!("{}.png", generator::safe_file_stem(name))).to_string_lossy().into_owned()
}

/// Lay out a result banner at `y`; returns its height.
pub fn banner_layout(f: &mut Frame, banner: Option<&Banner>, text: HWND, show: HWND, x: f32, y: f32, w: f32) -> f32 {
    let Some(banner) = banner else {
        f.hide(text);
        f.hide(show);
        return 0.0;
    };
    let (fill, border, icon, icon_color, msg, has_path) = match banner {
        Banner::Success { text, .. } => (c::SUCCESS_BG, c::SUCCESS_BORDER, Icon::CheckCircle, c::SUCCESS, text, true),
        Banner::Error(text) => (c::ERROR_BG, c::ERROR_BORDER, Icon::Alert, c::ERROR, text, false),
    };
    let show_w = if has_path { text_width(Font::Bold, "Show in folder") + 24.0 } else { 0.0 };
    let text_w = w - 16.0 - 22.0 - 12.0 - 16.0 - if has_path { show_w + 8.0 } else { 0.0 };
    let text_h = text_height(Font::Body, msg, text_w).max(20.0);
    let h = (text_h + 28.0).max(52.0);
    f.panel(RectF::new(x, y, w, h), fill, border, 10.0);
    f.items.push(Item::Icon(RectF::new(x + 16.0, y + 15.0, 22.0, 22.0), icon, icon_color));
    f.place(text, RectF::new(x + 50.0, y + (h - text_h) / 2.0, text_w, text_h));
    f.show_if(show, has_path, RectF::new(x + w - 10.0 - show_w, y + (h - 34.0) / 2.0, show_w, 34.0));
    h
}

pub fn layout(ui: &Ui, m: &mut Model, width: f32) -> Frame {
    let q = &ui.qr;
    let s = m.config.get().qr.clone();
    let mut f = Frame::default();
    let x = 40.0;
    let avail = (width - 80.0).max(400.0);
    f.text(RectF::new(x, 28.0, avail, 36.0), "QR codes", Font::Title, c::NAVY, DT_SINGLELINE);
    f.text(
        RectF::new(x, 68.0, avail, 20.0),
        "Make a QR code for a web address or a contact card.",
        Font::Body,
        c::MUTED,
        DT_SINGLELINE | DT_END_ELLIPSIS,
    );

    // Web address | Contact card
    let seg_w = text_width(Font::Bold, "Contact card").max(text_width(Font::Bold, "Web address")) + 36.0;
    let y = 104.0;
    f.items.push(Item::Fill(RectF::new(x, y, seg_w * 2.0 + 12.0, 44.0), 10.0, c::SEGMENT));
    f.place(q.seg_url, RectF::new(x + 4.0, y + 4.0, seg_w, 36.0));
    f.place(q.seg_contact, RectF::new(x + 8.0 + seg_w, y + 4.0, seg_w, 36.0));

    let top = y + 44.0 + 20.0;
    let preview_w = (avail * 0.36).clamp(250.0, 330.0);
    let form_w = avail - 24.0 - preview_w;
    let (x0, iw) = (x + 28.0, form_w - 56.0);
    let mut y = top + 26.0;

    // What to encode.
    match m.qr.kind {
        QrKind::Url => {
            y = super::labelled_field(&mut f, q.url_label, q.url, x0, y, iw) + 26.0;
            for i in 0..8 {
                f.hide(q.contact_labels[i]);
                f.hide(q.contact[i]);
            }
            f.hide(q.contact_hint);
        }
        QrKind::Contact => {
            f.hide(q.url_label);
            f.hide(q.url);
            let cols = if iw >= 400.0 { 2 } else { 1 };
            let col_w = (iw - (cols - 1) as f32 * 20.0) / cols as f32;
            for i in 0..8 {
                let (col, row) = (i % cols, i / cols);
                let cx = x0 + col as f32 * (col_w + 20.0);
                super::labelled_field(&mut f, q.contact_labels[i], q.contact[i], cx, y + row as f32 * 82.0, col_w);
            }
            y += (8 / cols) as f32 * 82.0 - 8.0;
            let hint = "First and last name are required. Blank fields are left off the card. Phone numbers can use digits, spaces and + - ( ).";
            let h = text_height(Font::Small, hint, iw);
            f.place(q.contact_hint, RectF::new(x0, y, iw, h));
            y += h + 26.0;
        }
    }

    // Look.
    f.rule(x0, y, iw);
    y += 22.0;
    f.text(RectF::new(x0, y, iw, 20.0), "Look", Font::Bold, c::TEXT, DT_SINGLELINE);
    y += 32.0;
    let cb_w = text_width(Font::Body, "Put a logo in the middle") + 32.0;
    f.place(q.logo, RectF::new(x0, y, cb_w, 24.0));
    let thumb_x = x0 + cb_w + 14.0;
    let thumb = RectF::new(thumb_x, y - 6.0, 36.0, 36.0);
    f.panel(thumb, c::WHITE, c::BORDER, 8.0);
    let source = if s.option_embed_logo { s.embed_image.clone() } else { String::new() };
    if let Some(bmp) = (!source.is_empty()).then(|| super::logo_bitmap(m, &source, theme::px(28.0), c::WHITE)).flatten()
    {
        f.items.push(Item::Image(RectF::new(thumb_x + 4.0, y - 2.0, 28.0, 28.0), bmp));
    } else {
        f.items.push(Item::Icon(RectF::new(thumb_x + 8.0, y + 2.0, 20.0, 20.0), Icon::Qr, c::FIELD_BORDER));
    }
    let change_w = text_width(Font::Bold, "Change logo…") + 24.0;
    f.place(q.logo_change, RectF::new(thumb_x + 44.0, y - 5.0, change_w, 34.0));
    y += 36.0;
    f.place(q.round, RectF::new(x0, y, iw, 24.0));
    y += 24.0 + 24.0;

    // Save.
    f.rule(x0, y, iw);
    y += 22.0;
    f.text(RectF::new(x0, y, iw, 20.0), "Save", Font::Bold, c::TEXT, DT_SINGLELINE);
    y += 30.0;
    let change_w = text_width(Font::Bold, "Change…") + 24.0;
    f.place(q.save_path, RectF::new(x0, y + 7.0, iw - change_w - 12.0, 20.0));
    f.place(q.save_change, RectF::new(x0 + iw - change_w, y, change_w, 34.0));
    y += 44.0;
    f.place(q.open_image, RectF::new(x0, y, iw, 24.0));
    y += 34.0;
    f.place(q.open_folder, RectF::new(x0, y, iw, 24.0));
    y += 24.0 + 26.0;

    // Buttons and result.
    f.rule(x0, y, iw);
    y += 24.0;
    let save_w = text_width(Font::Bold, "Save QR code") + 48.0;
    let clear_w = text_width(Font::Body, "Clear") + 40.0;
    f.place(q.save, RectF::new(x0, y, save_w, 44.0));
    f.place(q.clear, RectF::new(x0 + save_w + 12.0, y, clear_w, 44.0));
    y += 44.0;
    if m.qr.banner.is_some() {
        y += 18.0;
    }
    y += banner_layout(&mut f, m.qr.banner.as_ref(), q.result_text, q.result_show, x0, y, iw);
    y += 28.0;
    let form_bottom = y;
    f.items.insert(0, Item::Panel(RectF::new(x, top, form_w, form_bottom - top), c::WHITE, c::BORDER, 12.0));

    // Preview.
    let px0 = x + form_w + 24.0;
    let pw = preview_w - 40.0;
    let side = pw.min(PREVIEW_MAX);
    let mut py = top + 22.0;
    let mut preview_items = Vec::new();
    preview_items.push(Item::Text(
        RectF::new(px0 + 20.0, py, pw, 20.0),
        "Preview".into(),
        Font::Bold,
        c::TEXT,
        DT_SINGLELINE,
    ));
    py += 34.0;
    let img_rect = RectF::new(px0 + 20.0 + (pw - side) / 2.0, py, side, side);
    let caption = match &m.qr.preview {
        Preview::Ready(img) => {
            let want = theme::px(side);
            let bmp = match &m.qr.preview_bmp {
                Some((size, bmp)) if *size == want => bmp.clone(),
                _ => {
                    let bmp = Rc::new(Bitmap::from_image(img, want as u32, want as u32, c::WHITE));
                    m.qr.preview_bmp = Some((want, bmp.clone()));
                    bmp
                }
            };
            preview_items.push(Item::Image(img_rect, bmp));
            (format!("Updates as you type.\nSaves as a {} × {} px PNG.", img.width, img.height), c::MUTED)
        }
        Preview::Empty(hint) => {
            preview_items.push(Item::Panel(img_rect, c::SURFACE_ALT, c::BORDER, 10.0));
            preview_items.push(Item::Icon(
                RectF::new(img_rect.x + side / 2.0 - 20.0, py + side / 2.0 - 34.0, 40.0, 40.0),
                Icon::Qr,
                c::FIELD_BORDER,
            ));
            preview_items.push(Item::Text(
                RectF::new(img_rect.x + 20.0, py + side / 2.0 + 14.0, side - 40.0, 60.0),
                hint.to_string(),
                Font::Small,
                c::MUTED,
                DT_CENTER | DT_WORDBREAK,
            ));
            (String::new(), c::MUTED)
        }
        Preview::Problem(problem) => {
            preview_items.push(Item::Panel(img_rect, c::ERROR_BG, c::ERROR_BORDER, 10.0));
            preview_items.push(Item::Icon(
                RectF::new(img_rect.x + side / 2.0 - 16.0, py + 24.0, 32.0, 32.0),
                Icon::Alert,
                c::ERROR,
            ));
            preview_items.push(Item::Text(
                RectF::new(img_rect.x + 16.0, py + 68.0, side - 32.0, side - 80.0),
                problem.clone(),
                Font::Small,
                c::ERROR_TEXT,
                DT_CENTER | DT_WORDBREAK | DT_END_ELLIPSIS,
            ));
            (String::new(), c::MUTED)
        }
    };
    py += side + 14.0;
    if !caption.0.is_empty() {
        let h = text_height(Font::Small, &caption.0, pw);
        preview_items.push(Item::Text(
            RectF::new(px0 + 20.0, py, pw, h),
            caption.0,
            Font::Small,
            caption.1,
            DT_CENTER | DT_WORDBREAK,
        ));
        py += h;
    }
    py += 22.0;
    f.items.push(Item::Panel(RectF::new(px0, top, preview_w, py - top), c::WHITE, c::BORDER, 12.0));
    f.items.extend(preview_items);
    f.height = form_bottom.max(py) + 32.0;
    f
}

fn contact() -> Contact {
    let t = |i: usize| widgets::text(ui().qr.contact[i]);
    Contact {
        first_name: t(0),
        last_name: t(1),
        company: t(2),
        title: t(3),
        mobile: t(4),
        work_phone: t(5),
        email: t(6),
        website: t(7),
    }
}

fn payload(kind: QrKind) -> Result<String, GenerateError> {
    match kind {
        QrKind::Url => generator::url_payload(&widgets::text(ui().qr.url)),
        QrKind::Contact => generator::vcard_payload(&contact()),
    }
}

/// The decoded logo for the current setting (cached).
fn logo(m: &mut Model) -> LogoResult {
    let s = &m.config.get().qr;
    if !s.option_embed_logo {
        return Ok(None);
    }
    let key = s.embed_image.clone();
    if let Some((k, result)) = &m.qr.logo {
        if *k == key {
            return result.clone();
        }
    }
    let result = generator::logo_for(&key).map(|o| o.map(|img| Rc::new(img.into_owned()))).map_err(|e| e.to_string());
    m.qr.logo = Some((key, result.clone()));
    result
}

/// Re-render the preview from what is typed now.
pub fn refresh_preview() {
    let Some(kind) = with(|m| m.qr.kind) else { return };
    let typed = payload(kind);
    let preview = with(|m| {
        let round = m.config.get().qr.option_round_edges;
        match typed {
            Err(GenerateError::NothingToEncode) => Preview::Empty(empty_hint(kind)),
            Err(GenerateError::VCard(VCardError::MissingName)) => {
                if contact_is_blank() {
                    Preview::Empty(empty_hint(kind))
                } else {
                    Preview::Problem("Enter a first and last name.".into())
                }
            }
            Err(e) => Preview::Problem(e.to_string()),
            Ok(data) => match logo(m) {
                Err(reason) => Preview::Problem(format!("The logo could not be used: {reason}")),
                Ok(logo) => match generator::render_with(&data, round, logo.as_deref()) {
                    Ok(img) => Preview::Ready(Rc::new(img)),
                    Err(e) => Preview::Problem(e.to_string()),
                },
            },
        }
    });
    let Some(preview) = preview else { return };
    with(|m| {
        m.qr.preview = preview;
        m.qr.preview_bmp = None;
    });
    super::relayout(PageId::Qr);
}

fn contact_is_blank() -> bool {
    ui().qr.contact.iter().all(|&h| widgets::text(h).trim().is_empty())
}

fn schedule_preview() {
    // SAFETY: live window; re-arming the timer restarts the delay.
    unsafe { SetTimer(ui().main, super::TIMER_PREVIEW, 150, std::ptr::null()) };
}

fn set_banner(banner: Option<Banner>) {
    let shown = banner.is_some();
    with(|m| m.qr.banner = banner);
    super::sync(PageId::Qr);
    if shown {
        super::scroll_into_view(PageId::Qr, ui().qr.result_text);
    }
}

pub fn command(id: u16, code: u16, _control: HWND) {
    let q = &ui().qr;
    match (id, code) {
        (ID_SEG_URL | ID_SEG_CONTACT, BN_CLICKED) => {
            let kind = if id == ID_SEG_URL { QrKind::Url } else { QrKind::Contact };
            with(|m| {
                m.qr.kind = kind;
                m.qr.banner = None;
            });
            update_settings(|s| s.qr_kind = kind);
            super::sync(PageId::Qr);
            refresh_preview();
            widgets::focus(first_control());
        }
        (ID_URL, EN_CHANGE) | (ID_CONTACT_FIRST..=217, EN_CHANGE) => {
            if with(|m| m.qr.banner.take().is_some()) == Some(true) {
                super::sync(PageId::Qr);
            }
            schedule_preview();
        }
        (ID_LOGO, BN_CLICKED) => {
            let on = widgets::checked(q.logo);
            update_settings(|s| s.qr.option_embed_logo = on);
            settings_changed();
        }
        (ID_ROUND, BN_CLICKED) => {
            let on = widgets::checked(q.round);
            update_settings(|s| s.qr.option_round_edges = on);
            settings_changed();
        }
        (ID_OPEN_IMAGE, BN_CLICKED) => {
            let on = widgets::checked(q.open_image);
            update_settings(|s| s.qr.option_open_image = on);
            settings_changed();
        }
        (ID_OPEN_FOLDER, BN_CLICKED) => {
            let on = widgets::checked(q.open_folder);
            update_settings(|s| s.qr.option_open_folder = on);
            settings_changed();
        }
        (ID_LOGO_CHANGE, BN_CLICKED) => choose_logo(),
        (ID_SAVE_CHANGE, BN_CLICKED) => {
            choose_save_location();
        }
        (ID_SAVE, BN_CLICKED) => save(),
        (ID_CLEAR, BN_CLICKED) => clear(),
        (ID_SHOW, BN_CLICKED) => {
            if let Some(Some(Banner::Success { path, .. })) = with(|m| m.qr.banner.clone()) {
                let _ = crate::common::shell::reveal(&path);
            }
        }
        _ => {}
    }
}

/// QR settings changed (here or on the Settings page): refresh both pages
/// and the preview.
pub fn settings_changed() {
    super::sync(PageId::Qr);
    super::sync(PageId::Settings);
    refresh_preview();
}

pub fn save() {
    super::commit_pending_edits();
    let Some(kind) = with(|m| m.qr.kind) else { return };
    let data = match payload(kind) {
        Ok(data) => data,
        Err(e) => {
            set_banner(Some(Banner::Error(e.to_string())));
            widgets::focus(first_control());
            return;
        }
    };
    let Some(settings) = with(|m| m.config.get().qr.clone()) else { return };
    match generator::save(&data, &settings) {
        Ok(path) => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            set_banner(Some(Banner::Success { text: format!("Saved as {name}"), path: path.clone() }));
            generator::reveal(&path, &settings);
        }
        Err(e) if e.is_permission_denied() => {
            set_banner(None);
            if message(&format!("{e}\n\nChoose a different folder?"), MB_YESNO | MB_ICONWARNING) == IDYES
                && choose_save_location()
            {
                save();
            }
        }
        Err(e) => set_banner(Some(Banner::Error(e.to_string()))),
    }
}

fn clear() {
    let q = &ui().qr;
    match with(|m| m.qr.kind) {
        Some(QrKind::Url) => widgets::set_text(q.url, ""),
        _ => q.contact.iter().for_each(|&h| widgets::set_text(h, "")),
    }
    set_banner(None);
    refresh_preview();
    widgets::focus(first_control());
}

/// Ask for the folder and file name for new QR codes. Returns true if one
/// was chosen.
pub fn choose_save_location() -> bool {
    let Some(s) = with(|m| m.config.get().qr.clone()) else { return false };
    let folder = if s.saveto_path.trim().is_empty() { paths::downloads() } else { PathBuf::from(&s.saveto_path) };
    let folder = paths::nearest_existing_dir(&folder).unwrap_or_else(paths::downloads);
    let name = format!("{}.png", generator::safe_file_stem(&s.saveto_name));
    let Some(path) = dialogs::save_file(
        ui().main,
        "Choose where to save QR codes",
        &[("PNG image (*.png)", "*.png")],
        &folder,
        &name,
        "png",
    ) else {
        return false;
    };
    let (Some(folder), Some(stem)) = (path.parent(), path.file_stem()) else { return false };
    let (folder, stem) = (folder.to_string_lossy().into_owned(), stem.to_string_lossy().into_owned());
    update_settings(|s| {
        s.qr.saveto_path = folder;
        s.qr.saveto_name = stem;
    });
    settings_changed();
    true
}

/// Ask for an image to put in the middle of QR codes.
pub fn choose_logo() {
    let Some(current) = with(|m| m.config.get().qr.embed_image.clone()) else { return };
    let current = (current != DEFAULT_LOGO).then(|| PathBuf::from(current));
    let Some(path) = dialogs::open_file(
        ui().main,
        "Choose an image for the middle of QR codes",
        &[("Images (*.png, *.jpg)", "*.png;*.jpg;*.jpeg"), ("All files", "*.*")],
        current.as_deref(),
    ) else {
        return;
    };
    // Check it now rather than at the next QR code.
    if let Err(e) = generator::load_image_file(&path) {
        message(&e.to_string(), MB_ICONWARNING);
        return;
    }
    let path = path.to_string_lossy().into_owned();
    update_settings(|s| {
        s.qr.embed_image = path;
        s.qr.option_embed_logo = true;
    });
    settings_changed();
}

/// Back to the built-in logo.
pub fn use_default_logo() {
    update_settings(|s| s.qr.embed_image = DEFAULT_LOGO.to_string());
    settings_changed();
}
