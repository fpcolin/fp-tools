//! The window: a navy sidebar with the pages (QR codes, VendorCafe
//! converter, Settings, Help) and the update status, and the current page to
//! its right. Plain Win32 controls, drawn and laid out by this module.
//!
//! State lives in two thread-locals: [`Ui`], the control handles (fixed once
//! created), and [`Model`], everything that changes. The model is borrowed
//! only for short, self-contained steps and never while calling anything
//! that can run a message loop or send messages back to us (message boxes,
//! file dialogs, SetWindowText, SetFocus, moving controls).

// The thread-locals below are const-initialised; this lint misfires on them
// when std is built from source.
#![allow(clippy::missing_const_for_thread_local)]

mod canvas;
mod converter_page;
mod dialogs;
mod help_page;
mod layout;
mod qr_page;
mod settings_page;
mod theme;
mod updates;
mod widgets;
mod win32;

use std::cell::{Cell, RefCell};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::APP;
use crate::app::DISPLAY_NAME;
use crate::common::policy::Policy;
use crate::config::{self, Config, Page};
use canvas::{Bitmap, Canvas, RectF};
use layout::Frame;
use theme::{ButtonKind, Icon, c};
use win32::*;

/// Width of the sidebar in DIPs.
const SIDEBAR_W: f32 = 244.0;
/// Window size at first start, and the smallest it can be made (DIPs).
const START_SIZE: (i32, i32) = (1180, 800);
const MIN_SIZE: (i32, i32) = (980, 620);

const TIMER_PREVIEW: usize = 1;
const TIMER_UPDATE_CHECK: usize = 2;
/// Posted by background threads after queueing an [`Event`].
const WM_APP_EVENT: u32 = WM_APP + 1;
/// Posted once the window is on screen, for start-up questions.
const WM_APP_STARTED: u32 = WM_APP + 2;

const ID_NAV: [u16; 4] = [101, 102, 103, 104];
const ID_UPDATE: u16 = 110;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageId {
    Qr = 0,
    Converter = 1,
    Settings = 2,
    Help = 3,
}

impl PageId {
    const ALL: [PageId; 4] = [PageId::Qr, PageId::Converter, PageId::Settings, PageId::Help];
    fn index(self) -> usize {
        self as usize
    }
}

/// Control handles. Created once, then only read.
pub struct Ui {
    pub main: HWND,
    pub nav: [HWND; 4],
    pub update_btn: HWND,
    pub pages: [HWND; 4],
    pub qr: qr_page::Controls,
    pub vc: converter_page::Controls,
    pub settings: settings_page::Controls,
    pub help: help_page::Controls,
}

/// Result line shown under a page's main button.
#[derive(Clone, Debug)]
pub enum Banner {
    Success { text: String, path: std::path::PathBuf },
    Error(String),
}

/// What background threads report back.
pub enum Event {
    Update(updates::UpdateEvent),
    OldAppsRemoved(Vec<(&'static str, Result<bool, String>)>),
}

pub struct Model {
    pub config: Config,
    pub policy: Policy,
    pub page: PageId,
    /// Last layout of each page (paint list, hit areas) and its scroll offset.
    pub frames: [Frame; 4],
    pub scroll: [i32; 4],
    pub sidebar: Frame,
    pub qr: qr_page::State,
    pub vc: converter_page::State,
    pub update: updates::UpdateState,
    pub old_apps: Vec<crate::old_apps::OldApp>,
    pub removing_old_apps: bool,
    /// Scaled copies of logos, keyed by (source, size in pixels).
    pub bitmaps: Vec<((String, i32), Rc<Bitmap>)>,
    pub events: Sender<Event>,
    inbox: Receiver<Event>,
    /// Control that had the focus when the window was deactivated.
    saved_focus: usize,
}

// LAYING_OUT is set while a layout pass is running, so the WM_SIZE caused
// by a scroll bar appearing does not start a nested one.
thread_local! {
    static UI: Cell<Option<&'static Ui>> = const { Cell::new(None) };
    static MODEL: RefCell<Option<Model>> = const { RefCell::new(None) };
    static LAYING_OUT: Cell<bool> = const { Cell::new(false) };
}

pub fn ui() -> &'static Ui {
    UI.with(Cell::get).expect("window not created")
}

fn try_ui() -> Option<&'static Ui> {
    UI.with(Cell::get)
}

/// Run `f` with the model. Returns `None` (and does nothing) if the model is
/// already in use further up the stack, which the borrowing rules above
/// should make impossible; it is a safety net rather than a code path.
pub fn with<R>(f: impl FnOnce(&mut Model) -> R) -> Option<R> {
    MODEL.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => {
            debug_assert!(false, "model re-entered");
            None
        }
    })
}

/// Change settings and save them. A read-only profile is not an error worth
/// interrupting anyone for: the change still applies for this session.
pub fn update_settings(change: impl FnOnce(&mut config::Settings)) {
    with(|m| {
        let _ = m.config.update(change);
    });
}

fn instance() -> HINSTANCE {
    // SAFETY: null asks for this exe's own module handle.
    unsafe { GetModuleHandleW(std::ptr::null()) }
}

/// Show a message box owned by the main window.
pub fn message(text: &str, kind: u32) -> i32 {
    let text = wide(text);
    let caption = wide(DISPLAY_NAME);
    let owner = try_ui().map_or(std::ptr::null_mut(), |u| u.main);
    // SAFETY: NUL-terminated strings; owner is our window or null.
    unsafe { MessageBoxW(owner, text.as_ptr(), caption.as_ptr(), kind) }
}

/// For command-line use: write to the console of whoever started us.
pub fn attach_parent_console() {
    // SAFETY: no pointers; failure (no parent console) is fine.
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

/// Open the window and run until it is closed.
pub fn run() -> ExitCode {
    match std::panic::catch_unwind(run_window) {
        Ok(code) => code,
        Err(_) => ExitCode::FAILURE,
    }
}

fn run_window() -> ExitCode {
    let config = config::load();
    let policy = Policy::load();
    std::thread::spawn(|| crate::common::updater::cleanup_stale_downloads(&APP));

    let (tx, rx) = channel();
    let start_page = match config.get().last_page {
        Page::Qr => PageId::Qr,
        Page::Converter => PageId::Converter,
    };
    let qr_kind = config.get().qr_kind;
    let update = updates::UpdateState::initial(&policy);
    MODEL.with(|m| {
        *m.borrow_mut() = Some(Model {
            config,
            policy,
            page: start_page,
            frames: Default::default(),
            scroll: [0; 4],
            sidebar: Frame::default(),
            qr: qr_page::State::new(qr_kind),
            vc: converter_page::State::default(),
            update,
            old_apps: Vec::new(),
            removing_old_apps: false,
            bitmaps: Vec::new(),
            events: tx,
            inbox: rx,
            saved_focus: 0,
        })
    });

    let Some(main) = create_main_window() else {
        message("The window could not be created.", MB_ICONERROR);
        return ExitCode::FAILURE;
    };
    // SAFETY: live window; first show uses the shortcut's setting. Focus
    // rectangles and Alt-key underlines appear once the keyboard is used,
    // as in dialog boxes.
    unsafe {
        ShowWindow(main, SW_SHOWDEFAULT);
        SendMessageW(main, WM_CHANGEUISTATE, 0x0003_0003, 0);
    }
    show_page(start_page);
    // SAFETY: live window.
    unsafe {
        PostMessageW(main, WM_APP_STARTED, 0, 0);
        if with(|m| m.update.should_check_at_start()).unwrap_or(false) {
            SetTimer(main, TIMER_UPDATE_CHECK, 1200, std::ptr::null());
        }
    }
    message_loop(main);
    ExitCode::SUCCESS
}

fn message_loop(main: HWND) {
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: standard message loop on this thread's windows.
    unsafe {
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_KEYDOWN && (shortcut(msg.wParam) || converter_page::arrow_key(msg.wParam)) {
                continue;
            }
            // Tab / Shift+Tab, arrow keys in radio groups, Alt+letter and
            // Enter, as in a dialog box.
            let before = if msg.message == WM_KEYDOWN { GetFocus() } else { std::ptr::null_mut() };
            if IsDialogMessageW(main, &mut msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            // Keep the control that the keyboard moved to in view.
            if msg.message == WM_KEYDOWN {
                let now = GetFocus();
                if now != before && !now.is_null() {
                    if let Some(page) = page_of(GetParent(now)) {
                        scroll_into_view(page, now);
                    }
                }
            }
        }
    }
}

/// Ctrl+1..4 switch pages, F1 opens Help.
fn shortcut(key: usize) -> bool {
    // SAFETY: no pointers.
    let ctrl = unsafe { GetKeyState(VK_CONTROL) } < 0;
    let page = match key {
        0x31..=0x34 if ctrl => PageId::ALL[key - 0x31],
        VK_F1 => PageId::Help,
        _ => return false,
    };
    show_page(page);
    true
}

fn register_class(name: &str, proc_: WNDPROC, icon: bool) -> bool {
    let name = wide(name);
    // SAFETY: the class struct is fully initialised; resource 1 is the icon
    // embedded by build.rs (missing in test builds, then null is fine).
    unsafe {
        let (big, small) = if icon {
            (
                LoadImageW(
                    instance(),
                    int_resource(1),
                    IMAGE_ICON,
                    GetSystemMetricsForDpi(SM_CXICON, 96),
                    GetSystemMetricsForDpi(SM_CXICON, 96),
                    LR_DEFAULTCOLOR,
                ),
                LoadImageW(
                    instance(),
                    int_resource(1),
                    IMAGE_ICON,
                    GetSystemMetricsForDpi(SM_CXSMICON, 96),
                    GetSystemMetricsForDpi(SM_CXSMICON, 96),
                    LR_DEFAULTCOLOR,
                ),
            )
        } else {
            (std::ptr::null_mut(), std::ptr::null_mut())
        };
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: proc_,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance(),
            hIcon: big,
            hCursor: LoadCursorW(std::ptr::null_mut(), int_resource(IDC_ARROW)),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: small,
        };
        RegisterClassExW(&class) != 0
    }
}

fn create_main_window() -> Option<HWND> {
    if !register_class("FPToolsMain", main_proc, true) || !register_class("FPToolsPage", page_proc, false) {
        return None;
    }
    let title = wide(DISPLAY_NAME);
    let class = wide("FPToolsMain");
    // SAFETY: registered class, NUL-terminated strings.
    let main = unsafe {
        CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            START_SIZE.0,
            START_SIZE.1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance(),
            std::ptr::null_mut(),
        )
    };
    if main.is_null() {
        return None;
    }
    // The window now knows which screen it is on: size it for that DPI.
    // SAFETY: live window.
    let dpi = unsafe { GetDpiForWindow(main) }.max(96);
    theme::set_dpi(dpi);
    let mut r = RECT { left: 0, top: 0, right: px_for(START_SIZE.0, dpi), bottom: px_for(START_SIZE.1, dpi) };
    // SAFETY: valid rect pointer.
    unsafe { AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, 0, WS_EX_CONTROLPARENT, dpi) };
    // Fit small screens (a 1366 x 768 laptop, say), centred in the work area.
    let work = work_area(main);
    let (ww, wh) = (work.right - work.left, work.bottom - work.top);
    let (w, h) = if ww > 0 && wh > 0 {
        ((r.right - r.left).min(ww), (r.bottom - r.top).min(wh))
    } else {
        (r.right - r.left, r.bottom - r.top)
    };
    // SAFETY: live window.
    unsafe {
        SetWindowPos(
            main,
            std::ptr::null_mut(),
            work.left + (ww - w) / 2,
            work.top + (wh - h) / 2,
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }

    let nav_labels = ["QR codes", "VendorCafe converter", "Settings", "Help"];
    let nav_icons = [Icon::Qr, Icon::File, Icon::Gear, Icon::Help];
    let nav = std::array::from_fn(|i| {
        widgets::button(main, ID_NAV[i], nav_labels[i], ButtonKind::Nav(nav_icons[i]), c::NAVY)
    });
    let update_btn = widgets::button(main, ID_UPDATE, "Check for updates", ButtonKind::SidebarAction, c::NAVY_DARK);
    let pages: [HWND; 4] = std::array::from_fn(|i| create_page(main, i));
    let ui = Ui {
        main,
        nav,
        update_btn,
        pages,
        qr: qr_page::create(pages[0]),
        vc: converter_page::create(pages[1]),
        settings: settings_page::create(pages[2]),
        help: help_page::create(pages[3]),
    };
    UI.with(|u| u.set(Some(Box::leak(Box::new(ui)))));
    on_size();
    // Fill every page's controls from the settings before anything can read
    // them back.
    qr_page::sync();
    converter_page::sync();
    settings_page::sync();
    Some(main)
}

/// The usable part (without the taskbar) of the screen `hwnd` is on.
fn work_area(hwnd: HWND) -> RECT {
    let mut monitor = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: 0,
    };
    // SAFETY: valid struct pointer; the monitor handle comes straight from Windows.
    unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut monitor) };
    monitor.rcWork
}

fn px_for(dip: i32, dpi: u32) -> i32 {
    (dip as i64 * dpi as i64 / 96) as i32
}

fn create_page(main: HWND, index: usize) -> HWND {
    let class = wide("FPToolsPage");
    // SAFETY: registered class; the page's index is kept in GWLP_USERDATA.
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class.as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_CLIPCHILDREN | WS_VSCROLL,
            0,
            0,
            0,
            0,
            main,
            std::ptr::null_mut(),
            instance(),
            std::ptr::null_mut(),
        );
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, index as isize + 1);
        hwnd
    }
}

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    // SAFETY: valid rect pointer.
    unsafe { GetClientRect(hwnd, &mut r) };
    (r.right - r.left, r.bottom - r.top)
}

/// The main window changed size: move the page windows and lay out.
fn on_size() {
    let Some(ui) = try_ui() else { return };
    let (w, h) = client_size(ui.main);
    let side = theme::px(SIDEBAR_W);
    for &page in &ui.pages {
        // SAFETY: live child window.
        unsafe {
            SetWindowPos(page, std::ptr::null_mut(), side, 0, (w - side).max(0), h, SWP_NOZORDER | SWP_NOACTIVATE)
        };
    }
    relayout_sidebar();
    if let Some(page) = with(|m| m.page) {
        relayout(page);
    }
}

/// Switch to `page`.
pub fn show_page(page: PageId) {
    let ui = ui();
    commit_pending_edits();
    with(|m| m.page = page);
    match page {
        PageId::Qr => update_settings(|s| s.last_page = Page::Qr),
        PageId::Converter => update_settings(|s| s.last_page = Page::Converter),
        _ => {}
    }
    for p in PageId::ALL {
        theme::set_selected(ui.nav[p.index()], p == page);
    }
    sync(page);
    for p in PageId::ALL {
        // SAFETY: live child windows.
        unsafe { ShowWindow(ui.pages[p.index()], if p == page { SW_SHOWNA } else { SW_HIDE }) };
    }
    let first = match page {
        PageId::Qr => qr_page::first_control(),
        PageId::Converter => ui.vc.file_button,
        PageId::Settings => ui.settings.change_folder,
        PageId::Help => ui.help.open_settings,
    };
    widgets::focus(first);
}

/// Copy the model into a page's controls, then lay it out.
pub fn sync(page: PageId) {
    match page {
        PageId::Qr => qr_page::sync(),
        PageId::Converter => converter_page::sync(),
        PageId::Settings => settings_page::sync(),
        PageId::Help => help_page::sync(),
    }
    relayout(page);
}

/// Save edits that are only committed when focus leaves them.
pub fn commit_pending_edits() {
    if try_ui().is_some() {
        settings_page::commit_file_name();
    }
}

/// Lay out `page` for its current size and scroll position.
pub fn relayout(page: PageId) {
    let Some(ui) = try_ui() else { return };
    if LAYING_OUT.with(|f| f.replace(true)) {
        return;
    }
    let hwnd = ui.pages[page.index()];
    let s = theme::scale();
    let mut frame = Frame::default();
    let mut scroll = 0;
    // A scroll bar appearing or disappearing changes the width: lay out
    // again for the new width (at most twice more).
    for _ in 0..3 {
        let (w, h) = client_size(hwnd);
        let width = w as f32 / s;
        let Some(built) = with(|m| match page {
            PageId::Qr => qr_page::layout(ui, m, width),
            PageId::Converter => converter_page::layout(ui, m, width),
            PageId::Settings => settings_page::layout(ui, m, width),
            PageId::Help => help_page::layout(ui, m, width),
        }) else {
            break;
        };
        frame = built;
        let content = (frame.height * s).ceil() as i32;
        let max_scroll = (content - h).max(0);
        scroll = with(|m| {
            let v = m.scroll[page.index()].clamp(0, max_scroll);
            m.scroll[page.index()] = v;
            v
        })
        .unwrap_or(0);
        let info = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: content.max(1) - 1,
            nPage: h.max(0) as u32,
            nPos: scroll,
            nTrackPos: 0,
        };
        // SAFETY: valid struct; may resize the client area (handled by the loop).
        unsafe { SetScrollInfo(hwnd, SB_VERT, &info, 1) };
        if client_size(hwnd).0 == w {
            break;
        }
    }
    layout::apply(&frame, scroll);
    with(|m| m.frames[page.index()] = frame);
    LAYING_OUT.with(|f| f.set(false));
    widgets::invalidate(hwnd);
}

pub fn relayout_sidebar() {
    let Some(ui) = try_ui() else { return };
    updates::sync_button();
    let (_, h) = client_size(ui.main);
    let height = h as f32 / theme::scale();
    let Some(frame) = with(|m| updates::sidebar_layout(ui, m, height)) else { return };
    layout::apply(&frame, 0);
    with(|m| m.sidebar = frame);
    let side = RECT { left: 0, top: 0, right: theme::px(SIDEBAR_W), bottom: h };
    // SAFETY: live window, valid rect.
    unsafe { InvalidateRect(ui.main, &side, 0) };
}

/// A logo (or any image source) scaled to `size` pixels on `background`,
/// cached so layout passes do not resize it again.
pub fn logo_bitmap(m: &mut Model, source: &str, size: i32, background: u32) -> Option<Rc<Bitmap>> {
    let key = (format!("{source}|{background:06x}"), size);
    if let Some((_, bmp)) = m.bitmaps.iter().find(|(k, _)| *k == key) {
        return Some(bmp.clone());
    }
    let image = crate::generator::logo_for(source).ok()??;
    let bmp = Rc::new(Bitmap::from_image(&image, size as u32, size as u32, background));
    if m.bitmaps.len() > 16 {
        m.bitmaps.remove(0);
    }
    m.bitmaps.push((key, bmp.clone()));
    Some(bmp)
}

fn paint_window(hwnd: HWND, page: Option<PageId>) {
    // SAFETY: BeginPaint/EndPaint pair on a live window; the canvas is
    // dropped before EndPaint.
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let (w, h) = client_size(hwnd);
        if let Some(mut cv) = Canvas::new(w, h) {
            cv.clear(if page.is_some() { c::GROUND } else { c::NAVY });
            with(|m| match page {
                Some(p) => layout::paint(&mut cv, &m.frames[p.index()].items, m.scroll[p.index()]),
                None => layout::paint(&mut cv, &m.sidebar.items, 0),
            });
            cv.present(hdc, 0, 0);
        }
        EndPaint(hwnd, &ps);
    }
}

fn scroll_page(page: PageId, cmd: u16) {
    let hwnd = ui().pages[page.index()];
    let mut info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        nMin: 0,
        nMax: 0,
        nPage: 0,
        nPos: 0,
        nTrackPos: 0,
    };
    // SAFETY: valid struct.
    unsafe { GetScrollInfo(hwnd, SB_VERT, &mut info) };
    let line = theme::px(40.0);
    let page_px = info.nPage as i32;
    let pos = match cmd {
        SB_LINEUP => info.nPos - line,
        SB_LINEDOWN => info.nPos + line,
        SB_PAGEUP => info.nPos - page_px,
        SB_PAGEDOWN => info.nPos + page_px,
        SB_THUMBTRACK => info.nTrackPos,
        SB_TOP => 0,
        SB_BOTTOM => info.nMax,
        _ => return,
    };
    set_scroll(page, pos);
}

fn set_scroll(page: PageId, pos: i32) {
    let hwnd = ui().pages[page.index()];
    let mut info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        nMin: 0,
        nMax: 0,
        nPage: 0,
        nPos: 0,
        nTrackPos: 0,
    };
    // SAFETY: valid struct.
    unsafe { GetScrollInfo(hwnd, SB_VERT, &mut info) };
    let max = (info.nMax + 1 - info.nPage as i32).max(0);
    let pos = pos.clamp(0, max);
    if with(|m| std::mem::replace(&mut m.scroll[page.index()], pos)) == Some(pos) {
        return;
    }
    info.fMask = SIF_POS;
    info.nPos = pos;
    // SAFETY: valid struct.
    unsafe { SetScrollInfo(hwnd, SB_VERT, &info, 1) };
    if let Some(frame) = with(|m| std::mem::take(&mut m.frames[page.index()])) {
        layout::apply(&frame, pos);
        with(|m| m.frames[page.index()] = frame);
    }
    widgets::redraw_all(hwnd);
}

/// Bring a control that received the keyboard focus into view.
pub fn scroll_into_view(page: PageId, control: HWND) {
    let Some(r) =
        with(|m| m.frames[page.index()].places.iter().find(|(h, _)| *h == control).map(|(_, r)| *r)).flatten()
    else {
        return;
    };
    let s = theme::scale();
    let (_, h) = client_size(ui().pages[page.index()]);
    let scroll = with(|m| m.scroll[page.index()]).unwrap_or(0);
    let (top, bottom) = (((r.y - 16.0) * s) as i32, ((r.bottom() + 16.0) * s) as i32);
    if top < scroll {
        set_scroll(page, top);
    } else if bottom > scroll + h {
        set_scroll(page, bottom - h);
    }
}

fn page_of(hwnd: HWND) -> Option<PageId> {
    // SAFETY: reading our own window's user data.
    let v = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    PageId::ALL.get((v - 1) as usize).copied()
}

unsafe extern "system" fn page_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let result = std::panic::catch_unwind(|| page_message(hwnd, msg, w, l));
    // SAFETY: forwarding the original arguments.
    result.unwrap_or_else(|_| fatal()).unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, w, l) })
}

fn page_message(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    let page = page_of(hwnd)?;
    try_ui()?;
    match msg {
        WM_ERASEBKGND => Some(1),
        WM_PAINT => {
            paint_window(hwnd, Some(page));
            Some(0)
        }
        WM_SIZE => {
            if !LAYING_OUT.with(Cell::get) && with(|m| m.page) == Some(page) {
                relayout(page);
            }
            Some(0)
        }
        WM_VSCROLL => {
            scroll_page(page, loword(w));
            Some(0)
        }
        WM_MOUSEWHEEL => {
            let delta = hiword(w) as i16 as i32;
            let pos = with(|m| m.scroll[page.index()]).unwrap_or(0);
            set_scroll(page, pos - delta * theme::px(48.0) / 120);
            Some(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point_from_lparam(l);
            let s = theme::scale();
            let scroll = with(|m| m.scroll[page.index()]).unwrap_or(0);
            let (dx, dy) = (x as f32 / s, (y + scroll) as f32 / s);
            let target = with(|m| {
                let frame = &m.frames[page.index()];
                frame
                    .items
                    .iter()
                    .find_map(|i| match i {
                        layout::Item::Field(r, edit) if r.contains(dx, dy) => Some(Err(*edit)),
                        _ => None,
                    })
                    .or_else(|| frame.clicks.iter().find(|(r, _)| r.contains(dx, dy)).map(|&(_, id)| Ok(id)))
            })
            .flatten();
            match target {
                Some(Err(edit)) => widgets::focus(edit),
                Some(Ok(id)) => on_command(page, id, BN_CLICKED, std::ptr::null_mut()),
                None => {}
            }
            Some(0)
        }
        WM_COMMAND => {
            let (id, code) = (loword(w), hiword(w));
            let control = l as HWND;
            if code == EN_SETFOCUS || code == EN_KILLFOCUS {
                // Repaint the box around the edit (focus colour).
                widgets::invalidate(hwnd);
                if code == EN_SETFOCUS {
                    scroll_into_view(page, control);
                }
            }
            on_command(page, id, code, control);
            Some(0)
        }
        WM_DRAWITEM => {
            // SAFETY: Windows passes a valid DRAWITEMSTRUCT with this message.
            theme::draw_item(unsafe { &*(l as *const DRAWITEMSTRUCT) });
            Some(1)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORBTN => theme::ctl_color(w as HDC, l as HWND),
        _ => None,
    }
}

fn on_command(page: PageId, id: u16, code: u16, control: HWND) {
    match page {
        PageId::Qr => qr_page::command(id, code, control),
        PageId::Converter => converter_page::command(id, code),
        PageId::Settings => settings_page::command(id, code),
        PageId::Help => help_page::command(id, code),
    }
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let result = std::panic::catch_unwind(|| main_message(hwnd, msg, w, l));
    // SAFETY: forwarding the original arguments.
    result.unwrap_or_else(|_| fatal()).unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, w, l) })
}

fn main_message(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_GETMINMAXINFO => {
            // SAFETY: Windows passes a valid MINMAXINFO with this message.
            let info = unsafe { &mut *(l as *mut MINMAXINFO) };
            let dpi = theme::dpi();
            let mut r = RECT { left: 0, top: 0, right: px_for(MIN_SIZE.0, dpi), bottom: px_for(MIN_SIZE.1, dpi) };
            let work = work_area(hwnd);
            // SAFETY: valid rect pointer.
            unsafe { AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, 0, WS_EX_CONTROLPARENT, dpi) };
            // Never larger than the screen: a small or highly scaled display
            // gets a smaller window that scrolls.
            let (mut w, mut h) = (r.right - r.left, r.bottom - r.top);
            if work.right > work.left && work.bottom > work.top {
                w = w.min(work.right - work.left);
                h = h.min(work.bottom - work.top);
            }
            info.ptMinTrackSize = POINT { x: w, y: h };
            Some(0)
        }
        _ if try_ui().is_none() => None,
        WM_ERASEBKGND => Some(1),
        WM_PAINT => {
            paint_window(hwnd, None);
            Some(0)
        }
        WM_SIZE => {
            on_size();
            Some(0)
        }
        WM_DPICHANGED => {
            theme::set_dpi(loword(w) as u32);
            // SAFETY: Windows passes the suggested window rectangle.
            let r = unsafe { *(l as *const RECT) };
            // SAFETY: live window.
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
            on_size();
            Some(0)
        }
        WM_ACTIVATE => {
            if loword(w) == WA_INACTIVE {
                let focus = widgets::focused();
                with(|m| m.saved_focus = focus as usize);
            } else if let Some(saved) = with(|m| m.saved_focus).filter(|&f| f != 0) {
                // SAFETY: checks the handle is still one of our controls.
                if unsafe { IsChild(hwnd, saved as HWND) } != 0 {
                    widgets::focus(saved as HWND);
                    return Some(0);
                }
            }
            None
        }
        WM_COMMAND => {
            let (id, code) = (loword(w), hiword(w));
            if let Some(i) = ID_NAV.iter().position(|&n| n == id) {
                show_page(PageId::ALL[i]);
            } else if id == ID_UPDATE {
                updates::button_clicked();
            } else if id == IDOK && l == 0 {
                enter_pressed();
            }
            let _ = code;
            Some(0)
        }
        WM_DRAWITEM => {
            // SAFETY: Windows passes a valid DRAWITEMSTRUCT with this message.
            theme::draw_item(unsafe { &*(l as *const DRAWITEMSTRUCT) });
            Some(1)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORBTN => theme::ctl_color(w as HDC, l as HWND),
        WM_TIMER => {
            // SAFETY: live window; one-shot timers.
            unsafe { KillTimer(hwnd, w) };
            match w {
                TIMER_PREVIEW => qr_page::refresh_preview(),
                TIMER_UPDATE_CHECK => updates::start_check(false),
                _ => {}
            }
            Some(0)
        }
        WM_APP_EVENT => {
            while let Some(event) = with(|m| m.inbox.try_recv().ok()).flatten() {
                match event {
                    Event::Update(e) => updates::on_event(e),
                    Event::OldAppsRemoved(results) => updates::old_apps_removed(results),
                }
            }
            Some(0)
        }
        WM_APP_STARTED => {
            updates::check_old_apps();
            Some(0)
        }
        WM_QUERYENDSESSION => Some(1),
        WM_ENDSESSION => {
            // Windows is shutting down, or Setup (via Restart Manager) is
            // closing us to replace the files. Save and go.
            if w != 0 {
                commit_pending_edits();
                std::process::exit(0);
            }
            Some(0)
        }
        WM_CLOSE => {
            commit_pending_edits();
            // SAFETY: live window.
            unsafe { DestroyWindow(hwnd) };
            Some(0)
        }
        WM_DESTROY => {
            // SAFETY: no arguments.
            unsafe { PostQuitMessage(0) };
            Some(0)
        }
        _ => None,
    }
}

/// Enter: "click" the focused button, or else run the page's main action.
fn enter_pressed() {
    let focus = widgets::focused();
    if let Some(ctl) = theme::ctl(focus).filter(|c| c.button.is_some()) {
        let _ = ctl;
        // SAFETY: GetParent of a live control; the click is delivered as the
        // usual WM_COMMAND/BN_CLICKED to its parent.
        unsafe {
            let id = GetWindowLongPtrW(focus, -12) as usize; // GWLP_ID
            SendMessageW(GetParent(focus), WM_COMMAND, id & 0xFFFF, focus as LPARAM);
        }
        return;
    }
    match with(|m| m.page) {
        Some(PageId::Qr) => qr_page::save(),
        Some(PageId::Converter) => converter_page::convert(),
        _ => {}
    }
}

/// For `FPTools update`: wait for Setup while owning a hidden window, so that
/// Setup can close this process (through Restart Manager, like the window)
/// when it needs to replace the exe. Returns Setup's success.
pub fn wait_for_installer(running: crate::common::updater::Launched) -> bool {
    unsafe extern "system" fn wait_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        match msg {
            WM_QUERYENDSESSION => 1,
            WM_ENDSESSION if w != 0 => {
                println!("Setup closed this command to replace the program; the update continues.");
                std::process::exit(0)
            }
            // SAFETY: forwarding the original arguments.
            _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
        }
    }
    let (tx, rx) = channel();
    let hwnd = if register_class("FPToolsUpdateWait", wait_proc, false) {
        let class = wide("FPToolsUpdateWait");
        let title = wide(DISPLAY_NAME);
        // SAFETY: registered class; the window is never shown.
        unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance(),
                std::ptr::null_mut(),
            )
        }
    } else {
        std::ptr::null_mut()
    };
    if hwnd.is_null() {
        return running.wait();
    }
    let target = hwnd as usize;
    std::thread::spawn(move || {
        let _ = tx.send(running.wait());
        // SAFETY: posting to our own window (harmless if it is gone).
        unsafe { PostMessageW(target as HWND, WM_APP_EVENT, 0, 0) };
    });
    // SAFETY: standard message loop for this thread's hidden window.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_APP_EVENT {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        DestroyWindow(hwnd);
    }
    rx.recv().unwrap_or(false)
}

/// Post an event from a background thread and wake the window.
pub fn post(events: &Sender<Event>, main: usize, event: Event) {
    if events.send(event).is_ok() {
        // SAFETY: posting to a window handle that may already be gone is
        // harmless (the call just fails).
        unsafe { PostMessageW(main as HWND, WM_APP_EVENT, 0, 0) };
    }
}

pub fn main_handle() -> usize {
    ui().main as usize
}

fn fatal() -> Option<LRESULT> {
    message("Something went wrong and Flooring Partners Tools has to close.\n\nYour settings are saved.", MB_ICONERROR);
    std::process::exit(1);
}

/// Text box layout helper shared by pages: label above a field.
pub fn labelled_field(f: &mut Frame, label: HWND, edit: HWND, x: f32, y: f32, w: f32) -> f32 {
    f.place(label, RectF::new(x, y, w, 20.0));
    f.field(edit, RectF::new(x, y + 26.0, w, 40.0));
    y + 66.0
}
