//! 组字（composition）编辑会话：开始组字 / 更新组字串 / 上屏。
//!
//! TSF 的写操作必须经由 ITfEditSession；这里为每种操作实现一个会话对象，
//! 在 DoEditSession 里完成实际写入。所有会话使用 TF_ES_SYNC | TF_ES_READWRITE
//! （按键事件回调内同步会话是 TSF 官方推荐用法）。
//!
//! 结果回传：编辑会话对象内无法借用线程上下文（见 state 模块的借用规则），
//! 通过 `Arc<Mutex<…>>` 把 DoEditSession 产出的范围/视图对象带回调用方。

use std::sync::{Arc, Mutex};

use windows_core::{implement, Interface, Result};
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext,
    ITfContextComposition, ITfEditSession, ITfEditSession_Impl, ITfRange,
    ITfCompositionView, TF_ES_READWRITE, TF_ES_SYNC,
};

use crate::state;
use crate::state::ThreadCtx;

#[allow(clippy::arc_with_non_send_sync)] // COM 指针仅在同一线程使用
type Outcome<T> = Arc<Mutex<Option<T>>>;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// 在插入点开始组字（若尚未开始）。
pub fn ensure_started(ctx: &mut ThreadCtx, context: &ITfContext) -> Result<()> {
    if ctx.comp_range.is_some() {
        return Ok(());
    }
    #[allow(clippy::arc_with_non_send_sync)] // COM 指针仅在同一线程使用
    let outcome: Outcome<(ITfRange, Option<ITfCompositionView>)> = Arc::new(Mutex::new(None));
    let session: ITfEditSession = StartSession {
        context: context.clone(),
        sink: ctx.composition_sink.clone().ok_or_else(not_ready)?,
        outcome: Arc::clone(&outcome),
    }
    .into();
    let hr = unsafe { context.RequestEditSession(ctx.tid, &session, TF_ES_SYNC | TF_ES_READWRITE)? };
    if hr.is_err() {
        return Err(windows_core::Error::from_hresult(hr));
    }
    if let Some((range, view)) = lock(&outcome).take() {
        ctx.comp_range = Some(range);
        ctx.comp_view = view;
    }
    Ok(())
}

/// 更新组字串文本。
pub fn update_text(ctx: &mut ThreadCtx, context: &ITfContext, text: &str) -> Result<()> {
    let Some(range) = ctx.comp_range.clone() else {
        return Ok(());
    };
    let session: ITfEditSession = WriteRangeSession {
        range,
        text: text.to_string(),
    }
    .into();
    let hr = unsafe { context.RequestEditSession(ctx.tid, &session, TF_ES_SYNC | TF_ES_READWRITE)? };
    if hr.is_err() {
        return Err(windows_core::Error::from_hresult(hr));
    }
    Ok(())
}

/// 上屏最终文本并结束组字。
pub fn commit(ctx: &mut ThreadCtx, context: &ITfContext, text: &str) -> Result<()> {
    let Some(range) = ctx.comp_range.clone() else {
        return Ok(());
    };
    let view = ctx.comp_view.clone();
    let session: ITfEditSession = CommitSession {
        range,
        view,
        text: text.to_string(),
    }
    .into();
    let hr = unsafe { context.RequestEditSession(ctx.tid, &session, TF_ES_SYNC | TF_ES_READWRITE)? };
    if hr.is_err() {
        return Err(windows_core::Error::from_hresult(hr));
    }
    ctx.clear_composition();
    Ok(())
}

/// 激活时缓存活动视图（候选窗锚定用）。
pub fn store_active_view(ctx: &mut ThreadCtx, context: &ITfContext) -> Result<()> {
    let view = unsafe { context.GetActiveView() }?;
    ctx.active_view = Some(view);
    Ok(())
}

fn not_ready() -> windows_core::Error {
    windows_core::Error::from_hresult(windows_core::HRESULT(-1))
}

// —— 会话对象 ——

/// 开始组字：在当前选区创建 composition，记录范围并取回视图对象。
#[implement(ITfEditSession)]
struct StartSession {
    context: ITfContext,
    sink: ITfCompositionSink,
    outcome: Outcome<(ITfRange, Option<ITfCompositionView>)>,
}

impl ITfEditSession_Impl for StartSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        let context = &self.context;

        // 1) 当前选区
        let mut sel = [Default::default()];
        let mut fetched = 0u32;
        unsafe { context.GetSelection(ec, 0, &mut sel, &mut fetched)? };
        if fetched == 0 {
            return Err(windows_core::Error::from_hresult(windows_core::HRESULT(-1)));
        }
        let replaced = std::mem::replace(&mut sel[0].range, std::mem::ManuallyDrop::new(None));
        let range: ITfRange = std::mem::ManuallyDrop::into_inner(replaced)
            .ok_or_else(|| windows_core::Error::from_hresult(windows_core::HRESULT(-1)))?;

        // 2) 创建 composition（sink 接收终止回调）
        let context_composition: ITfContextComposition = context.cast()?;
        unsafe {
            context_composition.StartComposition(ec, &range, &self.sink)?;
        }

        // 3) 枚举取回视图对象（最后一个即新建的）
        let enum_comps = unsafe { context_composition.EnumCompositions()? };
        let mut latest: Option<ITfCompositionView> = None;
        let mut buffer = [None];
        let mut fetched = 0u32;
        while unsafe { enum_comps.Next(&mut buffer, &mut fetched) }.is_ok() && fetched > 0 {
            latest = buffer[0].clone();
        }
        *lock(&self.outcome) = Some((range, latest));
        Ok(())
    }
}

/// 向组字范围写入文本。
#[implement(ITfEditSession)]
struct WriteRangeSession {
    range: ITfRange,
    text: String,
}

impl ITfEditSession_Impl for WriteRangeSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        let wide: Vec<u16> = self.text.encode_utf16().collect();
        unsafe { self.range.SetText(ec, 0, &wide)? };
        Ok(())
    }
}

/// 上屏最终文本并尝试显式结束组字。
#[implement(ITfEditSession)]
struct CommitSession {
    range: ITfRange,
    view: Option<ITfCompositionView>,
    text: String,
}

impl ITfEditSession_Impl for CommitSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        let wide: Vec<u16> = self.text.encode_utf16().collect();
        unsafe { self.range.SetText(ec, 0, &wide)? };
        // 视图对象 QI 到 ITfComposition 以显式结束；失败则依赖宿主终止回调
        if let Some(view) = &self.view
            && let Ok(composition) = view.cast::<ITfComposition>()
        {
            unsafe { composition.EndComposition(ec)? };
        }
        Ok(())
    }
}

/// 组字终止监听（应用主动撤销 / 切换焦点导致终止时回调）。
#[implement(ITfCompositionSink)]
pub struct CompSink;

impl ITfCompositionSink_Impl for CompSink_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _composition: windows_core::Ref<'_, ITfComposition>,
    ) -> Result<()> {
        state::with(|t| t.on_composition_terminated());
        Ok(())
    }
}
