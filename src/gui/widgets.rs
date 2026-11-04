//! Creating the standard controls and reading/writing their state.

use super::theme::{self, ButtonKind, Ctl, Font, c};
use super::win32::*;

fn instance() -> HINSTANCE {
    // SAFETY: null asks for this exe's own module handle.
    unsafe { GetModuleHandleW(std::ptr::null()) }
}

fn create(class: &str, text: &str, style: u32, parent: HWND, id: u16) -> HWND {
    let class = wide(class);
    let text = wide(text);
    // SAFETY: NUL-terminated strings; `parent` is a live window. Controls
    // start at 0x0 and are positioned by the layout pass.
    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            text.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            0,
            0,
            parent,
            id as usize as HMENU,
            instance(),
            std::ptr::null_mut(),
        )
    }
}

/// Static text. `bg` is the colour of what it sits on.
pub fn label(parent: HWND, text: &str, font: Font, fg: u32, bg: u32) -> HWND {
    label_styled(parent, text, font, fg, bg, SS_NOPREFIX)
}

/// Static text whose `&` marks a keyboard shortcut (Alt+letter) for the
/// control created right after it.
pub fn mnemonic_label(parent: HWND, text: &str, font: Font, fg: u32, bg: u32) -> HWND {
    label_styled(parent, text, font, fg, bg, 0)
}

/// Single-line text that shortens long paths with "…" in the middle.
pub fn path_label(parent: HWND, font: Font, fg: u32, bg: u32) -> HWND {
    label_styled(parent, "", font, fg, bg, SS_NOPREFIX | SS_PATHELLIPSIS)
}

pub fn label_styled(parent: HWND, text: &str, font: Font, fg: u32, bg: u32, style: u32) -> HWND {
    let hwnd = create("STATIC", text, style, parent, 0xFFFF);
    theme::register(hwnd, Ctl { font, fg, bg, button: None, selected: false });
    hwnd
}

/// A borderless single-line edit; the page paints the box around it.
pub fn edit(parent: HWND, id: u16, limit: usize) -> HWND {
    let hwnd = create("EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL, parent, id);
    theme::register(hwnd, Ctl { font: Font::Body, fg: c::TEXT, bg: c::WHITE, button: None, selected: false });
    // SAFETY: live edit control.
    unsafe { SendMessageW(hwnd, EM_SETLIMITTEXT, limit, 0) };
    hwnd
}

pub fn checkbox(parent: HWND, id: u16, text: &str, bg: u32) -> HWND {
    let hwnd = create("BUTTON", text, WS_TABSTOP | BS_AUTOCHECKBOX, parent, id);
    theme::register(hwnd, Ctl { font: Font::Body, fg: c::TEXT, bg, button: None, selected: false });
    hwnd
}

/// Radio button; `first` starts a new group (arrow keys move within it).
pub fn radio(parent: HWND, id: u16, text: &str, first: bool, bg: u32) -> HWND {
    let style = BS_AUTORADIOBUTTON | if first { WS_GROUP | WS_TABSTOP } else { 0 };
    let hwnd = create("BUTTON", text, style, parent, id);
    theme::register(hwnd, Ctl { font: Font::Bold, fg: c::TEXT, bg, button: None, selected: false });
    hwnd
}

/// Owner-drawn button in one of the app's styles.
pub fn button(parent: HWND, id: u16, text: &str, kind: ButtonKind, bg: u32) -> HWND {
    let hwnd = create("BUTTON", text, WS_TABSTOP | BS_OWNERDRAW, parent, id);
    let font = match kind {
        ButtonKind::Primary | ButtonKind::Link => Font::Bold,
        ButtonKind::SidebarAction => Font::Small,
        _ => Font::Body,
    };
    theme::register(hwnd, Ctl { font, fg: c::NAVY, bg, button: Some(kind), selected: false });
    hwnd
}

pub fn text(hwnd: HWND) -> String {
    // SAFETY: the buffer is sized from GetWindowTextLengthW plus the NUL.
    unsafe {
        let len = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let got = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        String::from_utf16_lossy(&buf[..got.min(len)])
    }
}

/// Set the text only if it differs (avoids flicker and needless EN_CHANGE).
pub fn set_text(hwnd: HWND, value: &str) {
    if text(hwnd) != value {
        let w = wide(value);
        // SAFETY: NUL-terminated string; live window.
        unsafe { SetWindowTextW(hwnd, w.as_ptr()) };
    }
}

pub fn checked(hwnd: HWND) -> bool {
    // SAFETY: live button.
    unsafe { SendMessageW(hwnd, BM_GETCHECK, 0, 0) as usize == BST_CHECKED }
}

pub fn set_checked(hwnd: HWND, on: bool) {
    // SAFETY: live button.
    unsafe { SendMessageW(hwnd, BM_SETCHECK, usize::from(on), 0) };
}

pub fn enable(hwnd: HWND, on: bool) {
    // SAFETY: live window.
    unsafe { EnableWindow(hwnd, i32::from(on)) };
}

pub fn focus(hwnd: HWND) {
    // SAFETY: live window.
    unsafe { SetFocus(hwnd) };
}

pub fn focused() -> HWND {
    // SAFETY: no arguments.
    unsafe { GetFocus() }
}

pub fn invalidate(hwnd: HWND) {
    // SAFETY: live window (or null, which repaints all windows - never passed).
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
}

/// Has the user typed in this edit since it was last filled in?
pub fn modified(hwnd: HWND) -> bool {
    // SAFETY: live edit control.
    unsafe { SendMessageW(hwnd, EM_GETMODIFY, 0, 0) != 0 }
}

pub fn clear_modified(hwnd: HWND) {
    // SAFETY: live edit control.
    unsafe { SendMessageW(hwnd, EM_SETMODIFY, 0, 0) };
}

/// Repaint a window and all the controls on it.
pub fn redraw_all(hwnd: HWND) {
    // SAFETY: live window.
    unsafe { RedrawWindow(hwnd, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN) };
}

/// Grey hint text shown in an empty edit box.
pub fn set_cue(hwnd: HWND, cue: &str) {
    let w = wide(cue);
    // SAFETY: the edit copies the string.
    unsafe { SendMessageW(hwnd, EM_SETCUEBANNER, 1, w.as_ptr() as LPARAM) };
}

/// Include or skip a control when tabbing. (Setting a radio button's check
/// state also changes this, so it is re-applied after.)
pub fn set_tabstop(hwnd: HWND, on: bool) {
    const GWL_STYLE: i32 = -16;
    // SAFETY: reading and writing a live control's style bits.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let new = if on { style | WS_TABSTOP as isize } else { style & !(WS_TABSTOP as isize) };
        if new != style {
            SetWindowLongPtrW(hwnd, GWL_STYLE, new);
        }
    }
}
