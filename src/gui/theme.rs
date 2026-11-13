//! Colours, fonts and the look of each control.
//!
//! Every control the window creates is registered here with its font and
//! colours, so `WM_CTLCOLOR*`, `WM_DRAWITEM` and DPI changes can be handled
//! in one place. State is kept in a thread-local that is only ever borrowed
//! for the duration of one small function, so window messages that arrive
//! re-entrantly can never find it busy.

use std::cell::RefCell;
use std::collections::HashMap;

use super::canvas::{Canvas, RectF};
use super::win32::*;

/// Brand and neutral colours (0xRRGGBB).
pub mod c {
    pub const NAVY: u32 = 0x222A60;
    pub const NAVY_DARK: u32 = 0x1A2150;
    pub const NAVY_PRESSED: u32 = 0x151A42;
    pub const GOLD: u32 = 0xD8AB27;
    pub const GOLD_PRESSED: u32 = 0xC29820;
    pub const GOLD_SOFT: u32 = 0xFBF3DC;
    pub const GOLD_SOFT_BORDER: u32 = 0xEED9A0;
    pub const GROUND: u32 = 0xF5F6F9;
    pub const WHITE: u32 = 0xFFFFFF;
    pub const BORDER: u32 = 0xD9DCE5;
    pub const FIELD_BORDER: u32 = 0xA9AEC0;
    pub const TEXT: u32 = 0x1B1F2E;
    pub const MUTED: u32 = 0x5A6072;
    pub const SELECTED: u32 = 0xEEF0F8;
    pub const SEGMENT: u32 = 0xE4E7EF;
    pub const SURFACE_ALT: u32 = 0xF8F9FB;
    pub const SUCCESS: u32 = 0x1E7A4F;
    pub const SUCCESS_TEXT: u32 = 0x14532D;
    pub const SUCCESS_BG: u32 = 0xE9F6EF;
    pub const SUCCESS_BORDER: u32 = 0xB7DFC8;
    pub const ERROR: u32 = 0xB42318;
    pub const ERROR_TEXT: u32 = 0x5F1A12;
    pub const ERROR_BG: u32 = 0xFDEEEC;
    pub const ERROR_BORDER: u32 = 0xF1B5AE;
    pub const NAV_TEXT: u32 = 0xD3D7EE;
    pub const NAV_MUTED: u32 = 0xA3A9CF;
    pub const NAV_PRESSED: u32 = 0x2E3772;
    pub const SIDEBAR_LINE: u32 = 0x3A4274;
    pub const SIDEBAR_BUTTON_BORDER: u32 = 0x5A62A0;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Font {
    Body,
    Bold,
    Small,
    SmallBold,
    Title,
    Heading,
    Brand,
}

impl Font {
    const ALL: [Font; 7] =
        [Font::Body, Font::Bold, Font::Small, Font::SmallBold, Font::Title, Font::Heading, Font::Brand];

    /// (size in DIPs, semibold)
    fn spec(self) -> (i32, bool) {
        match self {
            Font::Body => (14, false),
            Font::Bold => (14, true),
            Font::Small => (13, false),
            Font::SmallBold => (13, true),
            Font::Title => (26, true),
            Font::Heading => (16, true),
            Font::Brand => (16, true),
        }
    }
}

/// Line drawings in a 24x24 grid, like the mock-ups' SVG icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Qr,
    File,
    /// A sheet with rows and columns (Scan to Excel).
    Table,
    Gear,
    Help,
    CheckCircle,
    Alert,
    Download,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    /// Text-only action, e.g. "Change…".
    Link,
    /// One half of a two-way switch.
    Segment,
    /// Sidebar page link.
    Nav(Icon),
    /// Outlined button on the dark sidebar.
    SidebarAction,
    /// Gold button on the dark sidebar ("Install update").
    SidebarGold,
}

#[derive(Clone, Copy, Debug)]
pub struct Ctl {
    pub font: Font,
    pub fg: u32,
    pub bg: u32,
    pub button: Option<ButtonKind>,
    /// Current page / current segment.
    pub selected: bool,
}

struct Theme {
    dpi: u32,
    fonts: HashMap<Font, HFONT>,
    brushes: HashMap<u32, HBRUSH>,
    ctls: HashMap<usize, Ctl>,
}

thread_local! {
    static THEME: RefCell<Theme> =
        RefCell::new(Theme { dpi: 96, fonts: HashMap::new(), brushes: HashMap::new(), ctls: HashMap::new() });
}

fn create_font(font: Font, dpi: u32) -> HFONT {
    let (size, bold) = font.spec();
    let height = -(((size as u32 * dpi + 48) / 96) as i32);
    let face = wide(if bold { "Segoe UI Semibold" } else { "Segoe UI" });
    // SAFETY: plain font creation; the face name is NUL-terminated.
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            if bold { FW_SEMIBOLD } else { FW_NORMAL },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH,
            face.as_ptr(),
        )
    }
}

/// Create the fonts for `dpi` and re-apply them to every registered control.
pub fn set_dpi(dpi: u32) {
    let (old, ctls) = THEME.with(|t| {
        let mut t = t.borrow_mut();
        t.dpi = dpi;
        let new: HashMap<Font, HFONT> = Font::ALL.iter().map(|&f| (f, create_font(f, dpi))).collect();
        let old = std::mem::replace(&mut t.fonts, new);
        let ctls: Vec<(usize, Font)> = t.ctls.iter().map(|(&h, c)| (h, c.font)).collect();
        (old, ctls)
    });
    for (hwnd, font) in ctls {
        // SAFETY: registered controls are live windows; WM_SETFONT takes a font handle.
        unsafe { SendMessageW(hwnd as HWND, WM_SETFONT, self::font(font) as usize, 1) };
    }
    for (_, f) in old {
        // SAFETY: no control uses the old fonts any more.
        unsafe { DeleteObject(f) };
    }
}

pub fn dpi() -> u32 {
    THEME.with(|t| t.borrow().dpi)
}

/// Pixels per DIP.
pub fn scale() -> f32 {
    dpi() as f32 / 96.0
}

/// DIPs to whole pixels.
pub fn px(dip: f32) -> i32 {
    (dip * scale()).round() as i32
}

pub fn font(font: Font) -> HFONT {
    THEME.with(|t| t.borrow().fonts.get(&font).copied().unwrap_or(std::ptr::null_mut()))
}

/// A cached solid brush (never freed; there are only a handful).
pub fn brush(rgb: u32) -> HBRUSH {
    THEME.with(|t| {
        *t.borrow_mut().brushes.entry(rgb).or_insert_with(|| {
            // SAFETY: plain brush creation.
            unsafe { CreateSolidBrush(colorref(rgb)) }
        })
    })
}

pub fn register(hwnd: HWND, ctl: Ctl) {
    THEME.with(|t| t.borrow_mut().ctls.insert(hwnd as usize, ctl));
    // SAFETY: `hwnd` was just created.
    unsafe { SendMessageW(hwnd, WM_SETFONT, font(ctl.font) as usize, 0) };
}

pub fn ctl(hwnd: HWND) -> Option<Ctl> {
    THEME.with(|t| t.borrow().ctls.get(&(hwnd as usize)).copied())
}

/// Change a control's colours and repaint it if they changed.
pub fn set_colors(hwnd: HWND, fg: u32, bg: u32) {
    let changed = THEME.with(|t| match t.borrow_mut().ctls.get_mut(&(hwnd as usize)) {
        Some(c) if (c.fg, c.bg) != (fg, bg) => {
            c.fg = fg;
            c.bg = bg;
            true
        }
        _ => false,
    });
    if changed {
        // SAFETY: live control.
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 1) };
    }
}

/// Change an owner-drawn button's style.
pub fn set_button_kind(hwnd: HWND, kind: ButtonKind, bg: u32) {
    let changed = THEME.with(|t| match t.borrow_mut().ctls.get_mut(&(hwnd as usize)) {
        Some(c) if (c.button, c.bg) != (Some(kind), bg) => {
            c.button = Some(kind);
            c.bg = bg;
            true
        }
        _ => false,
    });
    if changed {
        // SAFETY: live control.
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
    }
}

pub fn set_selected(hwnd: HWND, selected: bool) {
    let changed = THEME.with(|t| match t.borrow_mut().ctls.get_mut(&(hwnd as usize)) {
        Some(c) if c.selected != selected => {
            c.selected = selected;
            true
        }
        _ => false,
    });
    if changed {
        // SAFETY: live control.
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
    }
}

/// `WM_CTLCOLORSTATIC` / `WM_CTLCOLOREDIT` / `WM_CTLCOLORBTN`.
pub fn ctl_color(hdc: HDC, hwnd: HWND) -> Option<LRESULT> {
    let c = ctl(hwnd)?;
    // SAFETY: the DC is the one Windows passed with the message.
    unsafe {
        SetTextColor(hdc, colorref(c.fg));
        SetBkColor(hdc, colorref(c.bg));
    }
    Some(brush(c.bg) as LRESULT)
}

const ODS_NOACCEL: u32 = 0x0100;

/// `WM_DRAWITEM` for every owner-drawn button.
pub fn draw_item(dis: &DRAWITEMSTRUCT) {
    let Some(ctl) = ctl(dis.hwndItem) else { return };
    let Some(kind) = ctl.button else { return };
    let r = dis.rcItem;
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let Some(mut cv) = Canvas::new(w, h) else { return };
    let s = scale();
    let full = RectF::new(0.0, 0.0, w as f32, h as f32);
    let pressed = dis.itemState & ODS_SELECTED != 0;
    let disabled = dis.itemState & ODS_DISABLED != 0;
    let focused = dis.itemState & ODS_FOCUS != 0 && dis.itemState & ODS_NOFOCUSRECT == 0;
    let text = super::widgets::text(dis.hwndItem);
    let mut font_kind = ctl.font;
    let mut align = DT_CENTER;
    let mut text_rect = RECT { left: 0, top: 0, right: w, bottom: h };

    cv.clear(ctl.bg);
    let fg = match kind {
        ButtonKind::Primary => {
            let fill = if disabled {
                0x9AA0C0
            } else if pressed {
                c::NAVY_PRESSED
            } else {
                c::NAVY
            };
            cv.fill_rrect(full, 8.0 * s, fill);
            c::WHITE
        }
        ButtonKind::Secondary => {
            cv.fill_rrect(full, 8.0 * s, if pressed { c::SELECTED } else { c::WHITE });
            cv.stroke_rrect(full, 8.0 * s, s.max(1.0), c::FIELD_BORDER);
            if disabled { c::FIELD_BORDER } else { c::NAVY }
        }
        ButtonKind::Link => {
            if pressed {
                cv.fill_rrect(full, 6.0 * s, c::SELECTED);
            }
            if disabled { c::FIELD_BORDER } else { c::NAVY }
        }
        ButtonKind::Segment => {
            if ctl.selected {
                cv.fill_rrect(full, 7.0 * s, c::NAVY);
                font_kind = Font::Bold;
                c::WHITE
            } else {
                if pressed {
                    cv.fill_rrect(full, 7.0 * s, 0xD5D9E6);
                }
                c::NAVY
            }
        }
        ButtonKind::Nav(icon) => {
            let fg = if ctl.selected {
                cv.fill_rrect(full, 8.0 * s, c::WHITE);
                font_kind = Font::Bold;
                c::NAVY
            } else {
                if pressed {
                    cv.fill_rrect(full, 8.0 * s, c::NAV_PRESSED);
                }
                c::NAV_TEXT
            };
            let size = 20.0 * s;
            draw_icon(&mut cv, icon, RectF::new(12.0 * s, (h as f32 - size) / 2.0, size, size), fg);
            align = DT_LEFT;
            text_rect.left = (44.0 * s) as i32;
            text_rect.right = w - (8.0 * s) as i32;
            fg
        }
        ButtonKind::SidebarAction => {
            if pressed {
                cv.fill_rrect(full, 8.0 * s, c::NAV_PRESSED);
            }
            cv.stroke_rrect(full, 8.0 * s, s.max(1.0), c::SIDEBAR_BUTTON_BORDER);
            c::WHITE
        }
        ButtonKind::SidebarGold => {
            cv.fill_rrect(full, 8.0 * s, if pressed { c::GOLD_PRESSED } else { c::GOLD });
            font_kind = Font::Bold;
            c::NAVY
        }
    };
    if focused {
        let ring = if matches!(kind, ButtonKind::Primary) { c::GOLD } else { c::NAVY };
        let ring = if matches!(kind, ButtonKind::Nav(_) | ButtonKind::SidebarAction) && !ctl.selected {
            c::GOLD
        } else {
            ring
        };
        cv.stroke_rrect(full, 8.0 * s, 2.0 * s, ring);
    }
    // A label that does not fit in semibold falls back to regular weight
    // before it is shortened with "…".
    if font_kind == Font::Bold {
        let room = text_rect.right - text_rect.left - (16.0 * s) as i32;
        if super::canvas::measure(font(Font::Bold), &text, 10_000, DT_SINGLELINE).0 > room {
            font_kind = Font::Body;
        }
    }
    let prefix = if dis.itemState & ODS_NOACCEL != 0 { DT_HIDEPREFIX } else { 0 };
    cv.text(text_rect, &text, font(font_kind), fg, align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | prefix);
    cv.present(dis.hDC, r.left, r.top);
}

/// Draw `icon` scaled into `r` (pixels).
pub fn draw_icon(cv: &mut Canvas, icon: Icon, r: RectF, rgb: u32) {
    let k = r.w / 24.0;
    let p = |u: f32, v: f32| (r.x + u * k, r.y + v * k);
    let sw = 1.8 * k;
    let line = |cv: &mut Canvas, a: (f32, f32), b: (f32, f32)| {
        let (a, b) = (p(a.0, a.1), p(b.0, b.1));
        cv.line(a.0, a.1, b.0, b.1, sw, rgb);
    };
    let square = |cv: &mut Canvas, u: f32, v: f32, size: f32, filled: bool| {
        let (x, y) = p(u, v);
        let rr = RectF::new(x, y, size * k, size * k);
        if filled {
            cv.fill_rrect(rr, 0.6 * k, rgb);
        } else {
            cv.stroke_rrect(rr, 1.5 * k, sw, rgb);
        }
    };
    match icon {
        Icon::Qr => {
            square(cv, 3.0, 3.0, 7.5, false);
            square(cv, 13.5, 3.0, 7.5, false);
            square(cv, 3.0, 13.5, 7.5, false);
            square(cv, 13.5, 13.5, 3.2, true);
            square(cv, 17.8, 17.8, 3.2, true);
            square(cv, 17.8, 13.5, 3.2, true);
            square(cv, 13.5, 17.8, 3.2, true);
        }
        Icon::File => {
            let (x, y) = p(5.0, 2.5);
            cv.stroke_rrect(RectF::new(x, y, 14.0 * k, 19.0 * k), 2.0 * k, sw, rgb);
            line(cv, (8.5, 8.5), (12.5, 8.5));
            line(cv, (8.5, 12.5), (15.5, 12.5));
            line(cv, (8.5, 16.5), (15.5, 16.5));
        }
        Icon::Table => {
            let (x, y) = p(3.0, 4.0);
            cv.stroke_rrect(RectF::new(x, y, 18.0 * k, 16.0 * k), 2.0 * k, sw, rgb);
            line(cv, (3.5, 9.5), (20.5, 9.5));
            line(cv, (3.5, 14.8), (20.5, 14.8));
            line(cv, (9.5, 4.5), (9.5, 19.5));
        }
        Icon::Gear => {
            let (cx, cy) = p(12.0, 12.0);
            cv.stroke_circle(cx, cy, 3.0 * k, sw, rgb);
            cv.stroke_circle(cx, cy, 6.8 * k, sw, rgb);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let (s, c) = a.sin_cos();
                cv.line(cx + c * 7.6 * k, cy + s * 7.6 * k, cx + c * 10.0 * k, cy + s * 10.0 * k, 2.4 * k, rgb);
            }
        }
        Icon::Help => {
            let (cx, cy) = p(12.0, 12.0);
            cv.stroke_circle(cx, cy, 9.0 * k, sw, rgb);
            let pts: Vec<(f32, f32)> = [
                (9.4, 9.6),
                (9.9, 8.1),
                (12.0, 7.2),
                (14.1, 8.1),
                (14.6, 9.7),
                (13.6, 11.2),
                (12.0, 12.2),
                (12.0, 13.8),
            ]
            .iter()
            .map(|&(u, v)| p(u, v))
            .collect();
            cv.polyline(&pts, sw, rgb);
            let (dx, dy) = p(12.0, 17.0);
            cv.fill_circle(dx, dy, 1.2 * k, rgb);
        }
        Icon::CheckCircle => {
            let (cx, cy) = p(12.0, 12.0);
            cv.stroke_circle(cx, cy, 9.0 * k, sw, rgb);
            let pts: Vec<(f32, f32)> = [(8.0, 12.5), (10.6, 15.0), (16.0, 9.5)].iter().map(|&(u, v)| p(u, v)).collect();
            cv.polyline(&pts, sw * 1.1, rgb);
        }
        Icon::Alert => {
            let (cx, cy) = p(12.0, 12.0);
            cv.stroke_circle(cx, cy, 9.0 * k, sw, rgb);
            line(cv, (12.0, 7.5), (12.0, 13.0));
            let (dx, dy) = p(12.0, 16.5);
            cv.fill_circle(dx, dy, 1.2 * k, rgb);
        }
        Icon::Download => {
            line(cv, (12.0, 3.5), (12.0, 15.0));
            let pts: Vec<(f32, f32)> =
                [(7.0, 10.0), (12.0, 15.0), (17.0, 10.0)].iter().map(|&(u, v)| p(u, v)).collect();
            cv.polyline(&pts, sw, rgb);
            line(cv, (5.0, 20.0), (19.0, 20.0));
        }
    }
}
