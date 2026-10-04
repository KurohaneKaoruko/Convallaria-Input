//! 候选窗：服务进程自绘（GDI），点击候选即选词。
//!
//! 本模块全部在窗口过程 / FFI 上下文执行，整体豁免 unsafe_op_in_unsafe_fn。
#![allow(unsafe_op_in_unsafe_fn)]
//!
//! 全部运行在服务主线程；绘制数据与本模块状态均单线程，无锁。

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    SelectObject, SetBkMode, SetTextColor, TextOutW, FONT_CHARSET, FONT_CLIP_PRECISION,
    FONT_OUTPUT_PRECISION, FONT_QUALITY, HDC, HFONT, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, MSG, PeekMessageW,
    RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    HMENU, SWP_NOACTIVATE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_LBUTTONUP, WM_PAINT, WNDCLASSW,
};
use windows_core::{w, PCWSTR};

type ClickCallback = Box<dyn Fn(usize)>;

const CLASS_NAME: PCWSTR = w!("ConvallariaServerCandidateWnd");
const BORDER: i32 = 8;
const LINE_H_BASE: i32 = 26;

#[derive(Clone, Default)]
struct UiState {
    items: Vec<String>,
    footer: String,
}

thread_local! {
    static STATE: RefCell<UiState> = RefCell::new(UiState::default());
    static CACHED_FONT: RefCell<isize> = const { RefCell::new(0) };
    static WINDOW: RefCell<isize> = const { RefCell::new(0) };
    static CLICK_TX: RefCell<Option<ClickCallback>> = const { RefCell::new(None) };
}

fn hwnd() -> isize {
    WINDOW.with(|w| *w.borrow())
}

fn hwnd_handle() -> HWND {
    HWND(hwnd() as *mut core::ffi::c_void)
}

/// 注册点击回调（主循环启动时调用一次）。
pub fn set_click_handler(f: ClickCallback) {
    CLICK_TX.with(|c| *c.borrow_mut() = Some(f));
}

/// 确保窗口已创建。
pub fn ensure_window() {
    if hwnd() != 0 {
        return;
    }
    unsafe {
        let hmodule = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hmodule.into(),
            lpszClassName: CLASS_NAME,
            hbrBackground: CreateSolidBrush(COLORREF(0x00FF_FFFF)),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let created = CreateWindowExW(
            WINDOW_EX_STYLE(
                windows::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST.0
                    | windows::Win32::UI::WindowsAndMessaging::WS_EX_TOOLWINDOW.0
                    | windows::Win32::UI::WindowsAndMessaging::WS_EX_NOACTIVATE.0,
            ),
            CLASS_NAME,
            w!("Convallaria 候选"),
            WINDOW_STYLE(windows::Win32::UI::WindowsAndMessaging::WS_POPUP.0),
            0,
            0,
            240,
            200,
            None,
            Some(HMENU::default()),
            Some(windows::Win32::Foundation::HINSTANCE(hmodule.0)),
            None,
        )
        .unwrap_or_default();
        WINDOW.with(|w| *w.borrow_mut() = created.0 as isize);
    }
}

/// 同步候选数据并显示。
pub fn sync_candidates(items: Vec<String>, footer: String) {
    ensure_window();
    STATE.with(|d| {
        let mut s = d.borrow_mut();
        s.items = items;
        s.footer = footer;
    });
    unsafe {
        let _ = InvalidateRect(Some(hwnd_handle()), None, true);
        
        let _ = SetWindowPos(
            hwnd_handle(),
            Some(windows::Win32::UI::WindowsAndMessaging::HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
    }
}

/// 重新锚定窗口位置（组字矩形左下角）。
pub fn move_window(x: i32, y: i32, w: i32, h: i32) {
    let _ = (w, h);
    if hwnd() == 0 {
        return;
    }
    unsafe {
        let _ = SetWindowPos(
            hwnd_handle(),
            Some(windows::Win32::UI::WindowsAndMessaging::HWND_TOPMOST),
            x,
            y,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
    }
}

/// 隐藏候选窗。
pub fn hide_window() {
    if hwnd() != 0 {
        unsafe {
            let _ = ShowWindow(hwnd_handle(), SW_HIDE);
        }
    }
}

/// 消息泵（主循环每拍调用）：处理候选窗消息后小睡。
pub fn pump(timeout: std::time::Duration) {
    unsafe {
        if hwnd() == 0 {
            std::thread::sleep(timeout);
            return;
        }
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let mut msg = MSG::default();
            let has = PeekMessageW(
                &mut msg,
                Some(hwnd_handle()),
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::PM_REMOVE,
            )
            .as_bool();
            if has {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            if !has {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as u16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i32;
            click_hit_test(x, y);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn click_hit_test(_x: i32, y: i32) {
    let dpi = unsafe { GetDpiForSystem() } as f32;
    let scale = dpi / 96.0;
    let line_h = (LINE_H_BASE as f32 * scale) as i32;
    let index = ((y - BORDER) / line_h).max(0) as usize;
    let hit = STATE.with(|d| {
        let s = d.borrow();
        index < s.items.len()
    });
    if hit {
        CLICK_TX.with(|c| {
            if let Some(cb) = c.borrow().as_ref() {
                cb(index);
            }
        });
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc: HDC = BeginPaint(hwnd, &mut ps);
    if hdc.is_invalid() {
        return;
    }
    let dpi = GetDpiForSystem() as f32;
    let scale = dpi / 96.0;
    let line_h = (LINE_H_BASE as f32 * scale) as i32;

    let bg = CreateSolidBrush(COLORREF(0x00FF_FFFF));
    FillRect(hdc, &ps.rcPaint, bg);
    let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(bg.0));

    let font = ensure_font(scale);
    let old_font = SelectObject(hdc, windows::Win32::Graphics::Gdi::HGDIOBJ(font.0));
    SetBkMode(hdc, TRANSPARENT);

    let snapshot = STATE.with(|d| d.borrow().clone());
    for (i, text) in snapshot.items.iter().enumerate() {
        let y = BORDER + i as i32 * line_h;
        let label: Vec<u16> = format!("{}. ", i + 1).encode_utf16().collect();
        SetTextColor(hdc, COLORREF(0x0088_8888));
        let _ = TextOutW(hdc, BORDER, y, &label);
        let content: Vec<u16> = text.encode_utf16().collect();
        SetTextColor(hdc, COLORREF(0x0020_2020));
        let _ = TextOutW(hdc, BORDER + (18.0 * scale) as i32, y, &content);
    }

    let rows = snapshot.items.len() as i32;
    let y = BORDER + rows * line_h;
    let footer: Vec<u16> = snapshot.footer.encode_utf16().collect();
    SetTextColor(hdc, COLORREF(0x0099_9999));
    let _ = TextOutW(hdc, BORDER, y + 2, &footer);

    SelectObject(hdc, old_font);
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn ensure_font(scale: f32) -> HFONT {
    let cached = CACHED_FONT.with(|f| *f.borrow());
    if cached != 0 {
        return HFONT(cached as *mut core::ffi::c_void);
    }
    let height = (-14.0 * scale) as i32;
    let font = CreateFontW(
        height,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        FONT_CHARSET(0),
        FONT_OUTPUT_PRECISION(0),
        FONT_CLIP_PRECISION(0),
        FONT_QUALITY(5),
        0,
        w!("Microsoft YaHei UI"),
    );
    CACHED_FONT.with(|f| *f.borrow_mut() = font.0 as isize);
    font
}
