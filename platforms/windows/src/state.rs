//! 线程本地上下文：TSF 输入法的一切状态都挂在宿主输入线程上。

use std::cell::RefCell;

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::TextServices::{
    ITfCompositionSink, ITfCompositionView, ITfContext, ITfContextView, ITfRange, ITfThreadMgr,
};
use windows_core::BOOL;

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
}

thread_local! {
    static CTX: RefCell<Option<ThreadCtx>> = const { RefCell::new(None) };
}

/// 访问线程上下文（未激活时返回 None）。
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

// —— 组字相关便捷存取 ——

pub fn comp_range() -> Option<ITfRange> {
    with(|t| t.comp_range.clone()).flatten()
}

pub fn comp_view() -> Option<ITfCompositionView> {
    with(|t| t.comp_view.clone()).flatten()
}

pub fn set_composition_parts(range: ITfRange, view: Option<ITfCompositionView>) {
    with(|t| {
        t.comp_range = Some(range);
        t.comp_view = view;
    });
}

pub fn clear_composition() {
    with(|t| {
        t.comp_range = None;
        t.comp_view = None;
        if let Some(engine) = t.engine.as_mut() {
            engine.clear();
        }
        crate::candidate_window::hide(t.hwnd);
    });
}

pub fn composition_sink() -> Option<ITfCompositionSink> {
    with(|t| t.composition_sink.clone()).flatten()
}

pub fn set_active_view(view: ITfContextView) {
    with(|t| t.active_view = Some(view));
}

pub fn active_view() -> Option<ITfContextView> {
    with(|t| t.active_view.clone()).flatten()
}

/// 组字区间矩形（候选窗锚定）。
pub fn composition_rect() -> Option<RECT> {
    let view = active_view()?;
    let range = comp_range()?;
    let mut rect = RECT::default();
    let mut clipped = BOOL::default();
    unsafe {
        view.GetTextExt(0, &range, &mut rect, &mut clipped).ok()?;
    }
    Some(rect)
}

/// 宿主撤销组字时清理本地状态。
pub fn on_composition_terminated() {
    with(|t| {
        t.comp_range = None;
        t.comp_view = None;
        if let Some(engine) = t.engine.as_mut() {
            engine.clear();
        }
        crate::candidate_window::hide(t.hwnd);
    });
}
