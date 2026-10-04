//! 组字会话共享状态。
//!
//! 编辑会话是**异步**的：回调可能在按键处理返回之后才执行，所以组字相关的
//! 全部状态放在 `Rc<Session>` 里，由按键处理与编辑回调共享；编辑回调总是
//! 读取「最新」的待落定内容全量应用（快速连打时自然合并）。

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::TextServices::{ITfComposition, ITfCompositionSink, ITfContext, ITfContextView};
use windows_core::BOOL;

/// 每输入线程的组字会话状态。
pub struct Session {
    /// 客户端 id（TSF Activate 给出的 tid）。
    pub tid: Cell<u32>,
    /// 候选窗句柄。
    pub hwnd: Cell<isize>,
    /// 英文直通模式。
    pub english: Cell<bool>,
    /// 候选翻页页码。
    pub page: Cell<usize>,
    /// Shift 单击判定。
    pub shift_down: Cell<bool>,
    pub shift_tainted: Cell<bool>,
    /// 活动视图（候选窗锚定用）。
    pub view: RefCell<Option<ITfContextView>>,
    /// 活动组字对象。
    pub composition: RefCell<Option<ITfComposition>>,
    /// 组字终止监听（TSF 持有，结束组字时回调）。
    pub sink: RefCell<Option<ITfCompositionSink>>,
    /// 组字所属上下文（停用时把剩余组字原样落定用）。
    pub context: RefCell<Option<ITfContext>>,
    /// 待上屏文本（选词/空格/回车产生；异步会话里落定）。
    pub pending_commit: RefCell<Option<String>>,
    /// 待显示的组字串（当前按键原文；空串表示收起组字）。
    pub pending_preedit: RefCell<String>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            tid: Cell::new(0),
            hwnd: Cell::new(0),
            english: Cell::new(false),
            page: Cell::new(0),
            shift_down: Cell::new(false),
            shift_tainted: Cell::new(false),
            view: RefCell::new(None),
            composition: RefCell::new(None),
            sink: RefCell::new(None),
            context: RefCell::new(None),
            pending_commit: RefCell::new(None),
            pending_preedit: RefCell::new(String::new()),
        }
    }

    /// 缓存活动视图。
    pub fn set_active_view(&self, view: ITfContextView) {
        *self.view.borrow_mut() = Some(view);
    }

    /// 缓存按键事件带来的上下文。
    pub fn cache_context(&self, context: &ITfContext) {
        *self.context.borrow_mut() = Some(context.clone());
    }

    /// 缓存按键事件带来的活动视图。
    pub fn cache_active_view(&self, context: &ITfContext) {
        if let Ok(view) = unsafe { context.GetActiveView() } {
            self.set_active_view(view);
        }
    }

    /// 组字区间矩形（候选窗锚定）。
    pub fn composition_rect(&self) -> Option<RECT> {
        let view = self.view.borrow().as_ref()?.clone();
        let composition = self.composition.borrow().as_ref()?.clone();
        let range = unsafe { composition.GetRange() }.ok()?;
        let mut rect = RECT::default();
        let mut clipped = BOOL::default();
        unsafe {
            view.GetTextExt(0, &range, &mut rect, &mut clipped).ok()?;
        }
        Some(rect)
    }
}
