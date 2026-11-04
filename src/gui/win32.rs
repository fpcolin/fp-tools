//! Hand-written bindings for the few Windows functions the window needs.
//!
//! Linked with `raw-dylib`, so no import libraries are needed to build and
//! the exe imports exactly these functions. The DLLs are the minimum any
//! window needs (user32 for windows and input, gdi32 for drawing) plus
//! comdlg32 for the standard Open/Save dialogs.

#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms, dead_code)]

use std::ffi::c_void;

pub type HWND = *mut c_void;
pub type HINSTANCE = *mut c_void;
pub type HDC = *mut c_void;
pub type HGDIOBJ = *mut c_void;
pub type HFONT = *mut c_void;
pub type HBRUSH = *mut c_void;
pub type HBITMAP = *mut c_void;
pub type HICON = *mut c_void;
pub type HCURSOR = *mut c_void;
pub type HMENU = *mut c_void;
pub type HDWP = *mut c_void;
pub type WPARAM = usize;
pub type LPARAM = isize;
pub type LRESULT = isize;
pub type BOOL = i32;
pub type WNDPROC = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: WPARAM,
    pub lParam: LPARAM,
    pub time: u32,
    pub pt: POINT,
    pub lPrivate: u32,
}

#[repr(C)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: u32,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: HICON,
    pub hCursor: HCURSOR,
    pub hbrBackground: HBRUSH,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
    pub hIconSm: HICON,
}

#[repr(C)]
pub struct PAINTSTRUCT {
    pub hdc: HDC,
    pub fErase: BOOL,
    pub rcPaint: RECT,
    pub fRestore: BOOL,
    pub fIncUpdate: BOOL,
    pub rgbReserved: [u8; 32],
}

#[repr(C)]
pub struct DRAWITEMSTRUCT {
    pub CtlType: u32,
    pub CtlID: u32,
    pub itemID: u32,
    pub itemAction: u32,
    pub itemState: u32,
    pub hwndItem: HWND,
    pub hDC: HDC,
    pub rcItem: RECT,
    pub itemData: usize,
}

#[repr(C)]
pub struct SCROLLINFO {
    pub cbSize: u32,
    pub fMask: u32,
    pub nMin: i32,
    pub nMax: i32,
    pub nPage: u32,
    pub nPos: i32,
    pub nTrackPos: i32,
}

#[repr(C)]
pub struct BITMAPINFOHEADER {
    pub biSize: u32,
    pub biWidth: i32,
    pub biHeight: i32,
    pub biPlanes: u16,
    pub biBitCount: u16,
    pub biCompression: u32,
    pub biSizeImage: u32,
    pub biXPelsPerMeter: i32,
    pub biYPelsPerMeter: i32,
    pub biClrUsed: u32,
    pub biClrImportant: u32,
}

#[repr(C)]
pub struct BITMAPINFO {
    pub bmiHeader: BITMAPINFOHEADER,
    pub bmiColors: [u32; 1],
}

#[repr(C)]
pub struct TEXTMETRICW {
    pub tmHeight: i32,
    pub tmAscent: i32,
    pub tmDescent: i32,
    pub tmInternalLeading: i32,
    pub tmExternalLeading: i32,
    pub tmAveCharWidth: i32,
    pub tmMaxCharWidth: i32,
    pub tmWeight: i32,
    pub tmOverhang: i32,
    pub tmDigitizedAspectX: i32,
    pub tmDigitizedAspectY: i32,
    pub tmFirstChar: u16,
    pub tmLastChar: u16,
    pub tmDefaultChar: u16,
    pub tmBreakChar: u16,
    pub tmItalic: u8,
    pub tmUnderlined: u8,
    pub tmStruckOut: u8,
    pub tmPitchAndFamily: u8,
    pub tmCharSet: u8,
}

#[repr(C)]
pub struct MINMAXINFO {
    pub ptReserved: POINT,
    pub ptMaxSize: POINT,
    pub ptMaxPosition: POINT,
    pub ptMinTrackSize: POINT,
    pub ptMaxTrackSize: POINT,
}

#[repr(C)]
pub struct MONITORINFO {
    pub cbSize: u32,
    pub rcMonitor: RECT,
    pub rcWork: RECT,
    pub dwFlags: u32,
}

#[repr(C)]
pub struct TRACKMOUSEEVENT {
    pub cbSize: u32,
    pub dwFlags: u32,
    pub hwndTrack: HWND,
    pub dwHoverTime: u32,
}

#[repr(C)]
pub struct OPENFILENAMEW {
    pub lStructSize: u32,
    pub hwndOwner: HWND,
    pub hInstance: HINSTANCE,
    pub lpstrFilter: *const u16,
    pub lpstrCustomFilter: *mut u16,
    pub nMaxCustFilter: u32,
    pub nFilterIndex: u32,
    pub lpstrFile: *mut u16,
    pub nMaxFile: u32,
    pub lpstrFileTitle: *mut u16,
    pub nMaxFileTitle: u32,
    pub lpstrInitialDir: *const u16,
    pub lpstrTitle: *const u16,
    pub Flags: u32,
    pub nFileOffset: u16,
    pub nFileExtension: u16,
    pub lpstrDefExt: *const u16,
    pub lCustData: LPARAM,
    pub lpfnHook: *const c_void,
    pub lpTemplateName: *const u16,
    pub pvReserved: *mut c_void,
    pub dwReserved: u32,
    pub FlagsEx: u32,
}

// Window messages.
pub const WM_CREATE: u32 = 0x0001;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_SIZE: u32 = 0x0005;
pub const WM_ACTIVATE: u32 = 0x0006;
pub const WM_SETFOCUS: u32 = 0x0007;
pub const WM_KILLFOCUS: u32 = 0x0008;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_QUERYENDSESSION: u32 = 0x0011;
pub const WM_ERASEBKGND: u32 = 0x0014;
pub const WM_ENDSESSION: u32 = 0x0016;
pub const WM_SETCURSOR: u32 = 0x0020;
pub const WM_GETMINMAXINFO: u32 = 0x0024;
pub const WM_DRAWITEM: u32 = 0x002B;
pub const WM_SETFONT: u32 = 0x0030;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_VSCROLL: u32 = 0x0115;
pub const WM_CHANGEUISTATE: u32 = 0x0127;
pub const WM_UPDATEUISTATE: u32 = 0x0128;
pub const WM_CTLCOLOREDIT: u32 = 0x0133;
pub const WM_CTLCOLORBTN: u32 = 0x0135;
pub const WM_CTLCOLORSTATIC: u32 = 0x0138;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_MOUSEWHEEL: u32 = 0x020A;
pub const WM_MOUSELEAVE: u32 = 0x02A3;
pub const WM_DPICHANGED: u32 = 0x02E0;
pub const WM_APP: u32 = 0x8000;

// Button messages and notifications.
pub const BM_GETCHECK: u32 = 0x00F0;
pub const BM_SETCHECK: u32 = 0x00F1;
pub const BN_CLICKED: u16 = 0;
pub const BST_CHECKED: usize = 1;
// Edit messages and notifications.
pub const EM_SETLIMITTEXT: u32 = 0x00C5;
pub const EM_SETSEL: u32 = 0x00B1;
pub const EM_SETCUEBANNER: u32 = 0x1501;
pub const EM_GETMODIFY: u32 = 0x00B8;
pub const EM_SETMODIFY: u32 = 0x00B9;
pub const BM_CLICK: u32 = 0x00F5;
pub const BM_SETDONTCLICK: u32 = 0x00F8;
pub const EN_SETFOCUS: u16 = 0x0100;
pub const EN_KILLFOCUS: u16 = 0x0200;
pub const EN_CHANGE: u16 = 0x0300;

// Window styles.
pub const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const WS_DISABLED: u32 = 0x0800_0000;
pub const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
pub const WS_VSCROLL: u32 = 0x0020_0000;
pub const WS_GROUP: u32 = 0x0002_0000;
pub const WS_TABSTOP: u32 = 0x0001_0000;
pub const WS_EX_CONTROLPARENT: u32 = 0x0001_0000;
pub const ES_AUTOHSCROLL: u32 = 0x0080;
pub const SS_NOPREFIX: u32 = 0x0080;
pub const SS_NOTIFY: u32 = 0x0100;
pub const SS_ENDELLIPSIS: u32 = 0x4000;
pub const SS_PATHELLIPSIS: u32 = 0x8000;
pub const SS_EDITCONTROL: u32 = 0x2000;
pub const BS_AUTOCHECKBOX: u32 = 0x0003;
pub const BS_AUTORADIOBUTTON: u32 = 0x0009;
pub const BS_OWNERDRAW: u32 = 0x000B;
pub const BS_MULTILINE: u32 = 0x2000;
pub const CS_VREDRAW: u32 = 0x0001;
pub const CS_HREDRAW: u32 = 0x0002;

pub const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
pub const SW_HIDE: i32 = 0;
pub const SW_SHOW: i32 = 5;
pub const SW_SHOWNA: i32 = 8;
pub const SW_RESTORE: i32 = 9;
pub const SW_SHOWDEFAULT: i32 = 10;
pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOMOVE: u32 = 0x0002;
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_NOACTIVATE: u32 = 0x0010;
pub const SWP_SHOWWINDOW: u32 = 0x0040;
pub const SWP_HIDEWINDOW: u32 = 0x0080;
pub const SWP_NOCOPYBITS: u32 = 0x0100;
pub const RDW_INVALIDATE: u32 = 0x0001;
pub const RDW_ALLCHILDREN: u32 = 0x0080;
pub const GWLP_USERDATA: i32 = -21;
pub const IDOK: u16 = 1;
pub const IDCANCEL: u16 = 2;
pub const IDYES: i32 = 6;
pub const MB_OK: u32 = 0x0000;
pub const MB_YESNO: u32 = 0x0004;
pub const MB_ICONERROR: u32 = 0x0010;
pub const MB_ICONQUESTION: u32 = 0x0020;
pub const MB_ICONWARNING: u32 = 0x0030;
pub const MB_ICONINFORMATION: u32 = 0x0040;
pub const IDC_ARROW: usize = 32512;
pub const IDC_HAND: usize = 32649;
pub const IMAGE_ICON: u32 = 1;
pub const LR_DEFAULTCOLOR: u32 = 0;
pub const SM_CXICON: i32 = 11;
pub const SM_CXSMICON: i32 = 49;
pub const ODS_SELECTED: u32 = 0x0001;
pub const ODS_DISABLED: u32 = 0x0004;
pub const ODS_FOCUS: u32 = 0x0010;
pub const ODS_NOFOCUSRECT: u32 = 0x0200;
pub const SB_VERT: i32 = 1;
pub const SB_LINEUP: u16 = 0;
pub const SB_LINEDOWN: u16 = 1;
pub const SB_PAGEUP: u16 = 2;
pub const SB_PAGEDOWN: u16 = 3;
pub const SB_THUMBTRACK: u16 = 5;
pub const SB_TOP: u16 = 6;
pub const SB_BOTTOM: u16 = 7;
pub const SIF_RANGE: u32 = 0x0001;
pub const SIF_PAGE: u32 = 0x0002;
pub const SIF_POS: u32 = 0x0004;
pub const SIF_TRACKPOS: u32 = 0x0010;
pub const SIF_ALL: u32 = 0x0017;
pub const SW_INVALIDATE: u32 = 0x0002;
pub const SW_SCROLLCHILDREN: u32 = 0x0001;
pub const VK_TAB: usize = 0x09;
pub const VK_CONTROL: i32 = 0x11;
pub const VK_F1: usize = 0x70;
pub const TME_LEAVE: u32 = 0x0002;
pub const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
pub const ENDSESSION_CLOSEAPP: isize = 0x0000_0001;
pub const WA_INACTIVE: u16 = 0;
pub const HTCLIENT: u16 = 1;
pub const MONITOR_DEFAULTTONEAREST: u32 = 2;

// GDI.
pub const TRANSPARENT: i32 = 1;
pub const SRCCOPY: u32 = 0x00CC_0020;
pub const DIB_RGB_COLORS: u32 = 0;
pub const BI_RGB: u32 = 0;
pub const FW_NORMAL: i32 = 400;
pub const FW_SEMIBOLD: i32 = 600;
pub const DEFAULT_CHARSET: u32 = 1;
pub const OUT_TT_PRECIS: u32 = 4;
pub const CLIP_DEFAULT_PRECIS: u32 = 0;
pub const CLEARTYPE_QUALITY: u32 = 5;
pub const DEFAULT_PITCH: u32 = 0;
pub const DT_LEFT: u32 = 0x0000;
pub const DT_CENTER: u32 = 0x0001;
pub const DT_RIGHT: u32 = 0x0002;
pub const DT_VCENTER: u32 = 0x0004;
pub const DT_WORDBREAK: u32 = 0x0010;
pub const DT_SINGLELINE: u32 = 0x0020;
pub const DT_NOCLIP: u32 = 0x0100;
pub const DT_CALCRECT: u32 = 0x0400;
pub const DT_NOPREFIX: u32 = 0x0800;
pub const DT_END_ELLIPSIS: u32 = 0x8000;
pub const DT_PATH_ELLIPSIS: u32 = 0x4000;
pub const DT_HIDEPREFIX: u32 = 0x0010_0000;

// Open/Save dialogs.
pub const OFN_OVERWRITEPROMPT: u32 = 0x0000_0002;
pub const OFN_HIDEREADONLY: u32 = 0x0000_0004;
pub const OFN_NOCHANGEDIR: u32 = 0x0000_0008;
pub const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
pub const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
pub const OFN_NOREADONLYRETURN: u32 = 0x0000_8000;
pub const OFN_EXPLORER: u32 = 0x0008_0000;
pub const OFN_DONTADDTORECENT: u32 = 0x0200_0000;

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn RegisterClassExW(class: *const WNDCLASSEXW) -> u16;
    pub fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: HWND,
        menu: HMENU,
        instance: HINSTANCE,
        param: *mut c_void,
    ) -> HWND;
    pub fn DefWindowProcW(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn DestroyWindow(hwnd: HWND) -> BOOL;
    pub fn ShowWindow(hwnd: HWND, cmd: i32) -> BOOL;
    pub fn IsWindowVisible(hwnd: HWND) -> BOOL;
    pub fn GetMessageW(msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> BOOL;
    pub fn TranslateMessage(msg: *const MSG) -> BOOL;
    pub fn DispatchMessageW(msg: *const MSG) -> LRESULT;
    pub fn IsDialogMessageW(dlg: HWND, msg: *mut MSG) -> BOOL;
    pub fn PostMessageW(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> BOOL;
    pub fn SendMessageW(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn PostQuitMessage(code: i32);
    pub fn SetWindowPos(hwnd: HWND, after: HWND, x: i32, y: i32, w: i32, h: i32, flags: u32) -> BOOL;
    pub fn BeginDeferWindowPos(count: i32) -> HDWP;
    pub fn DeferWindowPos(info: HDWP, hwnd: HWND, after: HWND, x: i32, y: i32, w: i32, h: i32, flags: u32) -> HDWP;
    pub fn EndDeferWindowPos(info: HDWP) -> BOOL;
    pub fn GetClientRect(hwnd: HWND, rect: *mut RECT) -> BOOL;
    pub fn GetWindowRect(hwnd: HWND, rect: *mut RECT) -> BOOL;
    pub fn InvalidateRect(hwnd: HWND, rect: *const RECT, erase: BOOL) -> BOOL;
    pub fn RedrawWindow(hwnd: HWND, rect: *const RECT, rgn: *mut c_void, flags: u32) -> BOOL;
    pub fn BeginPaint(hwnd: HWND, ps: *mut PAINTSTRUCT) -> HDC;
    pub fn EndPaint(hwnd: HWND, ps: *const PAINTSTRUCT) -> BOOL;
    pub fn GetDC(hwnd: HWND) -> HDC;
    pub fn ReleaseDC(hwnd: HWND, hdc: HDC) -> i32;
    pub fn SetWindowTextW(hwnd: HWND, text: *const u16) -> BOOL;
    pub fn GetWindowTextW(hwnd: HWND, text: *mut u16, max: i32) -> i32;
    pub fn GetWindowTextLengthW(hwnd: HWND) -> i32;
    pub fn EnableWindow(hwnd: HWND, enable: BOOL) -> BOOL;
    pub fn IsWindowEnabled(hwnd: HWND) -> BOOL;
    pub fn SetFocus(hwnd: HWND) -> HWND;
    pub fn GetFocus() -> HWND;
    pub fn GetParent(hwnd: HWND) -> HWND;
    pub fn IsChild(parent: HWND, hwnd: HWND) -> BOOL;
    pub fn SetForegroundWindow(hwnd: HWND) -> BOOL;
    pub fn MessageBoxW(hwnd: HWND, text: *const u16, caption: *const u16, kind: u32) -> i32;
    pub fn LoadCursorW(instance: HINSTANCE, name: *const u16) -> HCURSOR;
    pub fn SetCursor(cursor: HCURSOR) -> HCURSOR;
    pub fn LoadImageW(instance: HINSTANCE, name: *const u16, kind: u32, cx: i32, cy: i32, flags: u32) -> *mut c_void;
    pub fn SetTimer(hwnd: HWND, id: usize, ms: u32, func: *const c_void) -> usize;
    pub fn KillTimer(hwnd: HWND, id: usize) -> BOOL;
    pub fn GetDpiForWindow(hwnd: HWND) -> u32;
    pub fn GetSystemMetricsForDpi(index: i32, dpi: u32) -> i32;
    pub fn AdjustWindowRectExForDpi(rect: *mut RECT, style: u32, menu: BOOL, ex_style: u32, dpi: u32) -> BOOL;
    pub fn SetScrollInfo(hwnd: HWND, bar: i32, info: *const SCROLLINFO, redraw: BOOL) -> i32;
    pub fn GetScrollInfo(hwnd: HWND, bar: i32, info: *mut SCROLLINFO) -> BOOL;
    pub fn ScrollWindowEx(
        hwnd: HWND,
        dx: i32,
        dy: i32,
        scroll: *const RECT,
        clip: *const RECT,
        update_rgn: *mut c_void,
        update: *mut RECT,
        flags: u32,
    ) -> i32;
    pub fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    pub fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
    pub fn DrawTextW(hdc: HDC, text: *const u16, len: i32, rect: *mut RECT, format: u32) -> i32;
    pub fn GetKeyState(key: i32) -> i16;
    pub fn ScreenToClient(hwnd: HWND, point: *mut POINT) -> BOOL;
    pub fn GetCursorPos(point: *mut POINT) -> BOOL;
    pub fn TrackMouseEvent(event: *mut TRACKMOUSEEVENT) -> BOOL;
    pub fn SystemParametersInfoW(action: u32, param: u32, out: *mut c_void, winini: u32) -> BOOL;
    pub fn MonitorFromWindow(hwnd: HWND, flags: u32) -> *mut c_void;
    pub fn GetMonitorInfoW(monitor: *mut c_void, info: *mut MONITORINFO) -> BOOL;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike_out: u32,
        charset: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face: *const u16,
    ) -> HFONT;
    pub fn DeleteObject(obj: HGDIOBJ) -> BOOL;
    pub fn SelectObject(hdc: HDC, obj: HGDIOBJ) -> HGDIOBJ;
    pub fn CreateCompatibleDC(hdc: HDC) -> HDC;
    pub fn DeleteDC(hdc: HDC) -> BOOL;
    pub fn CreateDIBSection(
        hdc: HDC,
        info: *const BITMAPINFO,
        usage: u32,
        bits: *mut *mut c_void,
        section: *mut c_void,
        offset: u32,
    ) -> HBITMAP;
    pub fn BitBlt(dst: HDC, x: i32, y: i32, w: i32, h: i32, src: HDC, sx: i32, sy: i32, rop: u32) -> BOOL;
    pub fn SetBkMode(hdc: HDC, mode: i32) -> i32;
    pub fn SetTextColor(hdc: HDC, color: u32) -> u32;
    pub fn SetBkColor(hdc: HDC, color: u32) -> u32;
    pub fn CreateSolidBrush(color: u32) -> HBRUSH;
    pub fn GdiFlush() -> BOOL;
    pub fn GetTextMetricsW(hdc: HDC, tm: *mut TEXTMETRICW) -> BOOL;
}

#[link(name = "comdlg32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn GetOpenFileNameW(ofn: *mut OPENFILENAMEW) -> BOOL;
    pub fn GetSaveFileNameW(ofn: *mut OPENFILENAMEW) -> BOOL;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
    pub fn AttachConsole(pid: u32) -> BOOL;
}

/// NUL-terminated UTF-16 for passing strings to Windows.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// `MAKEINTRESOURCE`.
pub fn int_resource(id: usize) -> *const u16 {
    id as *const u16
}

pub fn loword(v: usize) -> u16 {
    (v & 0xFFFF) as u16
}

pub fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xFFFF) as u16
}

/// Signed x and y of a mouse-message LPARAM.
pub fn point_from_lparam(l: LPARAM) -> (i32, i32) {
    ((l & 0xFFFF) as i16 as i32, ((l >> 16) & 0xFFFF) as i16 as i32)
}

/// Windows COLORREF (0x00BBGGRR) from 0xRRGGBB.
pub const fn colorref(rgb: u32) -> u32 {
    ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)
}
