//! 线程本地上下文：TSF 输入法的一切状态都挂在宿主输入线程上。
//!
//! **借用规则**：每个 COM 回调入口只允许一次 [`with`]；其余代码一律通过
//! `&mut ThreadCtx` 参数传递（`RefCell` 重入借用会 panic，panic 穿越 COM
//! 边界会带崩宿主应用——这正是历史上「一输入就崩软件」的根因）。

use std::cell::RefCell;

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::TextServices::{
    ITfCompositionSink, ITfCompositionView, ITfContext, ITfContextView, ITfRange, ITfThreadMgr,
};
use windows_core::BOOL;

use crate::candidate_window;
use crate::engine::Engine;

/// 每输入线程的全部状态。
pub struct ThreadCtx {
    pub tid: u32,
    pub thread_mgr: Option<ITfThreadMgr>,
    /// 最近一次按键带来的焦点上下文。
    pub context: Option<ITfContext>,
    /// 活动视图（候选窗锚定用）。
    pub active_view: Option<ITfContextView>,
    /// 组字范围（随组字自动延伸）。
    pub comp_range: Option<ITfRange>,
    /// 组字视图对象（显式 EndComposition 用）。
    pub comp_view: Option<ITfCompositionView>,
    pub composition_sink: Option<ITfCompositionSink>,
    pub engine: Option<Engine>,
    /// 候选窗句柄。
    pub hwnd: isize,
    /// 英文直通模式。
    pub english: bool,
    /// Shift 按下期间是否按过其他键（单击 Shift 判定）。
    pub shift_tainted: bool,
    /// Shift 是否处于按下状态。
    pub shift_down: bool,
}

impl ThreadCtx {
    pub fn new(tid: u32) -> Self {
        Self {
            tid,
            thread_mgr: None,
            context: None,
            active_view: None,
            comp_range: None,
            comp_view: None,
            composition_sink: None,
            engine: Engine::load(),
            hwnd: 0,
            english: false,
            shift_tainted: false,
            shift_down: false,
        }
    }

    /// 清空组字状态并隐藏候选窗。
    pub fn clear_composition(&mut self) {
        self.comp_range = None;
        self.comp_view = None;
        if let Some(engine) = self.engine.as_mut() {
            engine.clear();
        }
        candidate_window::hide(self.hwnd);
    }
    /// 宿主撤销组字时清理本地状态。
    pub fn on_composition_terminated(&mut self) {
        self.clear_composition();
    }

    /// 组字区间矩形（候选窗锚定）。
    pub fn composition_rect(&self) -> Option<RECT> {
        let view = self.active_view.as_ref()?;
        let range = self.comp_range.as_ref()?;
        let mut rect = RECT::default();
        let mut clipped = BOOL::default();
        unsafe {
            view.GetTextExt(0, range, &mut rect, &mut clipped).ok()?;
        }
        Some(rect)
    }
}

thread_local! {
    static CTX: RefCell<Option<ThreadCtx>> = const { RefCell::new(None) };
}

/// 访问线程上下文（未激活时返回 None）。
///
/// 闭包内**禁止**再调用 `with` 或任何会内部调用 `with` 的函数。
pub fn with<R>(f: impl FnOnce(&mut ThreadCtx) -> R) -> Option<R> {
    CTX.with(|cell| {
        let mut borrow = cell.borrow_mut();
        borrow.as_mut().map(f)
    })
}

/// 初始化 / 重置线程上下文。
pub fn init(tid: u32) {
    CTX.with(|cell| *cell.borrow_mut() = Some(ThreadCtx::new(tid)));
}

pub fn teardown() {
    CTX.with(|cell| *cell.borrow_mut() = None);
}

/// COM 回调兜底：panic 不穿越 FFI 边界（否则宿主应用崩溃）。
pub fn catch<R: Default>(f: impl FnOnce() -> R) -> R {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => {
            #[cfg(debug_assertions)]
            {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(std::env::temp_dir().join("convallaria-debug.log"))
                {
                    let _ = writeln!(f, "[panic] COM 回调发生 panic，已拦截");
                }
            }
            R::default()
        }
    }
}
