//! "Help": short how-tos, where settings live, and the version.

use super::canvas::RectF;
use super::layout::{Frame, text_height, text_width};
use super::theme::{ButtonKind, Font, c};
use super::win32::*;
use super::{Model, Ui, ui, widgets};
use crate::APP;

const ID_OPEN_SETTINGS: u16 = 500;

const SECTIONS: [(&str, &str); 4] = [
    (
        "QR codes",
        "Choose Web address or Contact card and fill in the details; the preview updates as you type. \
         Save QR code writes a PNG to the folder shown under Save. An existing file is never replaced: \
         (1), (2)… is added to the name instead.\n\n\
         QR codes use the highest level of error correction, so they still scan with a logo in the middle. \
         A very short web address makes a very small code that may not scan with a logo; turn the logo off for those.",
    ),
    (
        "VendorCafe converter",
        "1.  In Rollmaster, export the VendorCafe invoices to a CSV file. Don't open or edit it first.\n\
         2.  Choose the file, choose PO number or Work order number for this customer, and click Convert.\n\
         3.  Upload the converted file to VendorCafe.\n\n\
         PO number keeps the PO No and Job No columns. Work order number renames PO No to WO No and drops Job No.",
    ),
    (
        "Keyboard",
        "Ctrl+1 QR codes · Ctrl+2 VendorCafe converter · Ctrl+3 Settings · F1 Help.\n\
         Enter saves the QR code or converts the file. Alt plus an underlined letter jumps to that box.",
    ),
    (
        "Settings and scripts",
        "Your settings are kept in your own profile, so everyone who uses this PC has their own. \
         The same features can be used from scripts: run \"FPTools help\" in a command prompt for the commands.",
    ),
];

pub struct Controls {
    pub texts: [HWND; 4],
    pub open_settings: HWND,
}

pub fn create(page: HWND) -> Controls {
    let texts = SECTIONS.map(|(_, body)| widgets::label(page, body, Font::Body, c::TEXT, c::WHITE));
    let open_settings =
        widgets::button(page, ID_OPEN_SETTINGS, "Open settings folder", ButtonKind::Secondary, c::WHITE);
    Controls { texts, open_settings }
}

pub fn sync() {}

pub fn layout(ui: &Ui, _m: &mut Model, width: f32) -> Frame {
    let h = &ui.help;
    let mut f = Frame::default();
    let x = 40.0;
    let avail = (width - 80.0).clamp(400.0, 820.0);
    f.text(RectF::new(x, 28.0, avail, 36.0), "Help", Font::Title, c::NAVY, DT_SINGLELINE);
    f.text(
        RectF::new(x, 68.0, avail, 20.0),
        &format!("Flooring Partners Tools {}", APP.version),
        Font::Body,
        c::MUTED,
        DT_SINGLELINE,
    );
    let (x0, iw) = (x + 28.0, avail - 56.0);
    let mut top = 112.0;
    for (i, (title, body)) in SECTIONS.iter().enumerate() {
        let mark = f.mark();
        let mut y = top + 24.0;
        f.text(RectF::new(x0, y, iw, 22.0), title, Font::Heading, c::NAVY, DT_SINGLELINE);
        y += 36.0;
        let th = text_height(Font::Body, body, iw);
        f.place(h.texts[i], RectF::new(x0, y, iw, th));
        y += th;
        if i == SECTIONS.len() - 1 {
            y += 18.0;
            let w = text_width(Font::Body, "Open settings folder") + 36.0;
            f.place(h.open_settings, RectF::new(x0, y, w, 36.0));
            y += 36.0;
        }
        y += 24.0;
        f.card_behind(mark, RectF::new(x, top, avail, y - top));
        top = y + 20.0;
    }
    f.height = top + 12.0;
    f
}

pub fn command(id: u16, code: u16) {
    if id == ID_OPEN_SETTINGS && code == BN_CLICKED {
        if let Some(dir) = crate::common::paths::settings_dir(APP.vendor, APP.name) {
            let _ = std::fs::create_dir_all(&dir);
            let _ = crate::common::shell::open(&dir);
        }
    }
    let _ = ui();
}
