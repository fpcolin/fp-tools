//! One layout pass produces a [`Frame`]: where each control goes and what to
//! paint behind them, all in DIPs (1/96 inch). Applying it converts to pixels
//! for the current DPI and scroll position.

use std::rc::Rc;

use super::canvas::{self, Bitmap, Canvas, RectF};
use super::theme::{self, Font, Icon, c};
use super::win32::*;

pub enum Item {
    Fill(RectF, f32, u32),
    /// Rounded box: fill, border colour, radius.
    Panel(RectF, u32, u32, f32),
    Text(RectF, String, Font, u32, u32),
    /// The painted box around a borderless edit control.
    Field(RectF, HWND),
    Image(RectF, Rc<Bitmap>),
    Icon(RectF, Icon, u32),
    /// Horizontal hairline.
    Rule(f32, f32, f32, u32),
    /// Progress bar, 0..1.
    Progress(RectF, f32),
}

#[derive(Default)]
pub struct Frame {
    pub items: Vec<Item>,
    pub places: Vec<(HWND, RectF)>,
    pub hidden: Vec<HWND>,
    /// Painted areas that act like a control when clicked (command id).
    pub clicks: Vec<(RectF, u16)>,
    /// Total content height in DIPs (for scrolling).
    pub height: f32,
}

impl Frame {
    pub fn place(&mut self, hwnd: HWND, r: RectF) {
        self.places.push((hwnd, r));
    }

    pub fn click(&mut self, r: RectF, id: u16) {
        self.clicks.push((r, id));
    }

    pub fn hide(&mut self, hwnd: HWND) {
        self.hidden.push(hwnd);
    }

    pub fn show_if(&mut self, hwnd: HWND, show: bool, r: RectF) {
        if show { self.place(hwnd, r) } else { self.hide(hwnd) }
    }

    pub fn card(&mut self, r: RectF) {
        self.items.push(Item::Panel(r, c::WHITE, c::BORDER, 12.0));
    }

    /// Where the next item goes; pass to [`Frame::card_behind`] once the
    /// card's height is known.
    pub fn mark(&self) -> usize {
        self.items.len()
    }

    /// A card painted behind the items added since `mark`.
    pub fn card_behind(&mut self, mark: usize, r: RectF) {
        self.items.insert(mark, Item::Panel(r, c::WHITE, c::BORDER, 12.0));
    }

    pub fn panel(&mut self, r: RectF, fill: u32, border: u32, radius: f32) {
        self.items.push(Item::Panel(r, fill, border, radius));
    }

    /// Painted text (not reachable by screen readers: use labels for
    /// anything a control needs as its name).
    pub fn text(&mut self, r: RectF, text: &str, font: Font, rgb: u32, flags: u32) {
        self.items.push(Item::Text(r, text.to_string(), font, rgb, flags));
    }

    pub fn rule(&mut self, x: f32, y: f32, w: f32) {
        self.items.push(Item::Rule(x, y, w, c::BORDER));
    }

    /// Place an edit inside a painted 40-DIP box at `r`.
    pub fn field(&mut self, edit: HWND, r: RectF) {
        self.items.push(Item::Field(r, edit));
        let line = text_height(Font::Body, "Ag", 1000.0) + 2.0;
        self.place(edit, RectF::new(r.x + 11.0, r.y + (r.h - line) / 2.0, r.w - 22.0, line));
    }
}

/// Height in DIPs of `text` wrapped at `width` DIPs.
pub fn text_height(font: Font, text: &str, width: f32) -> f32 {
    let s = theme::scale();
    let (_, h) = canvas::measure(theme::font(font), text, (width * s) as i32, DT_WORDBREAK | DT_NOPREFIX);
    (h as f32 / s).ceil()
}

/// Width in DIPs of single-line `text`.
pub fn text_width(font: Font, text: &str) -> f32 {
    let s = theme::scale();
    let (w, _) = canvas::measure(theme::font(font), text, 10_000, DT_SINGLELINE | DT_NOPREFIX);
    (w as f32 / s).ceil()
}

/// Move, size, show and hide the controls in one batch.
pub fn apply(frame: &Frame, scroll_px: i32) {
    let s = theme::scale();
    // SAFETY: every handle in the frame is a live control; DeferWindowPos
    // returns null on failure, in which case the remaining calls are skipped.
    unsafe {
        let mut dwp = BeginDeferWindowPos((frame.places.len() + frame.hidden.len()) as i32);
        for &(hwnd, r) in &frame.places {
            if dwp.is_null() {
                break;
            }
            let p = r.to_px(s, scroll_px as f32);
            dwp = DeferWindowPos(
                dwp,
                hwnd,
                std::ptr::null_mut(),
                p.left,
                p.top,
                (p.right - p.left).max(0),
                (p.bottom - p.top).max(0),
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOCOPYBITS,
            );
        }
        for &hwnd in &frame.hidden {
            if dwp.is_null() {
                break;
            }
            dwp = DeferWindowPos(
                dwp,
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW,
            );
        }
        if !dwp.is_null() {
            EndDeferWindowPos(dwp);
        }
    }
}

/// Paint the items onto `cv`, which covers the window's client area.
pub fn paint(cv: &mut Canvas, items: &[Item], scroll_px: i32) {
    let s = theme::scale();
    let dy = scroll_px as f32;
    let focus = super::widgets::focused();
    let to_f = |r: RectF| {
        let p = r.to_px(s, dy);
        RectF::new(p.left as f32, p.top as f32, (p.right - p.left) as f32, (p.bottom - p.top) as f32)
    };
    for item in items {
        match item {
            Item::Fill(r, radius, rgb) => cv.fill_rrect(to_f(*r), radius * s, *rgb),
            Item::Panel(r, fill, border, radius) => {
                let r = to_f(*r);
                cv.fill_rrect(r, radius * s, *border);
                cv.fill_rrect(r.inset(s.max(1.0), s.max(1.0)), (radius - 1.0).max(0.0) * s, *fill);
            }
            Item::Text(r, text, font, rgb, flags) => {
                cv.text(r.to_px(s, dy), text, theme::font(*font), *rgb, *flags | DT_NOPREFIX);
            }
            Item::Field(r, edit) => {
                let r = to_f(*r);
                // SAFETY: live edit control.
                let enabled = unsafe { IsWindowEnabled(*edit) } != 0;
                let (border, width) = if focus == *edit { (c::NAVY, 2.0 * s) } else { (c::FIELD_BORDER, s.max(1.0)) };
                cv.fill_rrect(r, 6.0 * s, border);
                cv.fill_rrect(
                    r.inset(width, width),
                    (6.0 * s - width).max(0.0),
                    if enabled { c::WHITE } else { c::SURFACE_ALT },
                );
            }
            Item::Image(r, bmp) => {
                let p = r.to_px(s, dy);
                cv.image(p.left, p.top, bmp);
            }
            Item::Icon(r, icon, rgb) => theme::draw_icon(cv, *icon, to_f(*r), *rgb),
            Item::Rule(x, y, w, rgb) => {
                let top = (y * s - dy).round() as i32;
                let thick = s.round().max(1.0) as i32;
                cv.fill_rect(
                    RECT {
                        left: (x * s).round() as i32,
                        top,
                        right: ((x + w) * s).round() as i32,
                        bottom: top + thick,
                    },
                    *rgb,
                );
            }
            Item::Progress(r, frac) => {
                let r = to_f(*r);
                cv.fill_rrect(r, r.h / 2.0, c::SIDEBAR_LINE);
                let done = RectF::new(r.x, r.y, (r.w * frac.clamp(0.0, 1.0)).max(r.h), r.h);
                cv.fill_rrect(done, r.h / 2.0, c::GOLD);
            }
        }
    }
}
