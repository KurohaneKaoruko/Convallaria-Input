//! 候选窗：宿主线程上的 WS_POPUP 自绘窗口（GDI 渲染，DPI 感知）。
//!
//! - 位置锚定组字矩形下方，避免遮挡组字串
//! - WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW：不抢焦点、不出现在任务栏
//! - 绘制数据直接读取线程上下文快照（同线程消息循环由宿主应用驱动）

// 本模块所有函数都在窗口过程 / FFI 上下文中执行，unsafe 块显式标注到调用点级
// 会淹没可读性；此处整体豁免 unsafe_op_in_unsafe_fn。
#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    SelectObject, SetBkMode, SetTextColor, TextOutW, FONT_CHARSET, FONT_CLIP_PRECISION,
    FONT_OUTPUT_PRECISION, FONT_QUALITY, HDC, HFONT, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos, ShowWindow,
    CS_HREDRAW, CS_VREDRAW, HMENU, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW, SW_HIDE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_PAINT, WNDCLASSW,
};
use windows_core::{w, PCWSTR};

use crate::state;
const CLASS_NAME: PCWSTR = w!("ConvallariaCandidateWnd");
const BORDER: i32 = 8;

/// 当前绘制快照。
#[derive(Clone, Default)]
struct Snapshot {
    items: Vec<(usize, String)>,
    footer: String,
}

thread_local! {
    static PAINT_DATA: std::cell::RefCell<Snapshot> = std::cell::RefCell::new(Snapshot::default());
    static CACHED_FONT: std::cell::RefCell<isize> = const { std::cell::RefCell::new(0) };
}

fn hwnd_from_isize(h: isize) -> HWND {
    HWND(h as *mut core::ffi::c_void)
}

/// 确保候选窗已创建，返回句柄。
pub fn ensure_window() -> isize {
    state::with(|t| {
        if t.hwnd != 0 {
            return t.hwnd;
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
            let hwnd = CreateWindowExW(
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
                Some(HINSTANCE(hmodule.0)),
                None,
            )
            .unwrap_or_default();
            t.hwnd = hwnd.0 as isize;
            t.hwnd
        }
    })
    .unwrap_or(0)
}

/// 显示候选：锚定组字矩形下方。
pub fn show(items: Vec<(usize, String)>, footer: String, anchor: Option<RECT>) {
    let hwnd = ensure_window();
    if hwnd == 0 {
        return;
    }
    PAINT_DATA.with(|d| *d.borrow_mut() = Snapshot { items, footer });
    let rows = PAINT_DATA.with(|d| d.borrow().items.len()).max(1) as i32;
    let dpi = unsafe { GetDpiForSystem() } as f32;
    let scale = dpi / 96.0;
    let line_h = (26.0 * scale) as i32;
    let width = (232.0 * scale) as i32;
    let height = line_h * (rows + 1) + BORDER * 2;
    unsafe {
        let (x, y) = match anchor {
            Some(rect) => (rect.left, rect.bottom + 2),
            None => (0, 0),
        };
        let final_flags = if anchor.is_some() {
            SWP_NOACTIVATE | SWP_SHOWWINDOW
        } else {
            SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE
        };
        let _ = SetWindowPos(
            hwnd_from_isize(hwnd),
            Some(HWND_TOPMOST),
            x,
            y,
            width,
            height,
            final_flags,
        );
    }
    unsafe {
        let _ = InvalidateRect(Some(hwnd_from_isize(hwnd)), None, true);
    }
}

/// 隐藏候选窗。
pub fn hide(hwnd: isize) {
    if hwnd != 0 {
        unsafe {
            let _ = ShowWindow(hwnd_from_isize(hwnd), SW_HIDE);
        }
    }
}

/// 销毁候选窗（线程退出时）。
pub fn destroy(hwnd: isize) {
    if hwnd != 0 {
        unsafe {
            let _ = DestroyWindow(hwnd_from_isize(hwnd));
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_PAINT {
        paint(hwnd);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc: HDC = BeginPaint(hwnd, &mut ps);
    if hdc.is_invalid() {
        return;
    }

    let dpi = GetDpiForSystem() as f32;
    let scale = dpi / 96.0;
    let line_h = (26.0 * scale) as i32;

    // 白底填充
    let bg = CreateSolidBrush(COLORREF(0x00FF_FFFF));
    FillRect(hdc, &ps.rcPaint, bg);
    let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(bg.0));

    // CJK 字体
    let font = ensure_font(scale);
    let old_font = SelectObject(hdc, windows::Win32::Graphics::Gdi::HGDIOBJ(font.0));
    SetBkMode(hdc, TRANSPARENT);

    let snapshot = PAINT_DATA.with(|d| d.borrow().clone());

    for (i, (idx, text)) in snapshot.items.iter().enumerate() {
        let y = BORDER + i as i32 * line_h;
        let label: Vec<u16> = format!("{idx}. ").encode_utf16().collect();
        SetTextColor(hdc, COLORREF(0x0088_8888));
        let _ = TextOutW(hdc, BORDER, y, &label);
        let content: Vec<u16> = text.encode_utf16().collect();
        SetTextColor(hdc, COLORREF(0x0020_2020));
        let _ = TextOutW(hdc, BORDER + (18.0 * scale) as i32, y, &content);
    }

    // 页脚（模式 / 页码 / 中英状态）
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