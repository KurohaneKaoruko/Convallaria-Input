//! 组字（composition）编辑会话：开始组字 / 更新组字串 / 上屏。
//!
//! TSF 的写操作必须经由 ITfEditSession；这里为每种操作实现一个会话对象，
//! 在 DoEditSession 里完成实际写入。所有会话使用 TF_ES_SYNC | TF_ES_READWRITE
//! （按键事件回调内同步会话是 TSF 官方推荐用法）。
//!
//! 实现说明：StartComposition 后通过 EnumCompositions 取回 ITfCompositionView；
//! 组字文本更新直接使用建字时的 ITfRange（范围随组字自动延伸），上屏后尝试
//! 以 QI 取得 ITfComposition::EndComposition 显式结束（QI 失败时由宿主终止回调兜底）。

use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfCompositionView, ITfContext,
    ITfContextComposition, ITfEditSession, ITfEditSession_Impl, ITfRange, TF_ES_READWRITE,
    TF_ES_SYNC,
};
use windows_core::{Interface, Result, implement};

use crate::state;

/// 在插入点开始组字（若尚未开始）。
pub fn ensure_started(context: &ITfContext, tid: u32) -> Result<()> {
    if state::comp_range().is_some() {
        return Ok(());
    }
    let session: ITfEditSession = StartSession {
        context: context.clone(),
    }
    .into();
    let hr = unsafe { context.RequestEditSession(tid, &session, TF_ES_SYNC | TF_ES_READWRITE)? };
    if hr.is_err() {
        return Err(windows_core::Error::from_hresult(hr));
    }
    Ok(())
}

/// 更新组字串文本。
pub fn update_text(context: &ITfContext, tid: u32, text: &str) -> Result<()> {
    let Some(range) = state::comp_range() else {
        return Ok(());
    };
    let session: ITfEditSession = WriteRangeSession {
        range,
        text: text.to_string(),
    }
    .into();
    let hr = unsafe { context.RequestEditSession(tid, &session, TF_ES_SYNC | TF_ES_READWRITE)? };
    if hr.is_err() {
        return Err(windows_core::Error::from_hresult(hr));
    }
    Ok(())
}

/// 上屏最终文本并结束组字。
pub fn commit(context: &ITfContext, tid: u32, text: &str) -> Result<()> {
    let Some(range) = state::comp_range() else {
        return Ok(());
    };
    let session: ITfEditSession = CommitSession {
        range,
        view: state::comp_view(),
        text: text.to_string(),
    }
    .into();
    let hr = unsafe { context.RequestEditSession(tid, &session, TF_ES_SYNC | TF_ES_READWRITE)? };
    if hr.is_err() {
        return Err(windows_core::Error::from_hresult(hr));
    }
    state::clear_composition();
    Ok(())
}

/// 激活时缓存活动视图（候选窗锚定用）。
pub fn store_active_view(context: &ITfContext) -> Result<()> {
    let view = unsafe { context.GetActiveView() }?;
    state::set_active_view(view);
    Ok(())
}

// —— 会话对象 ——

/// 开始组字：在当前选区创建 composition，记录范围并取回视图对象。
#[implement(ITfEditSession)]
struct StartSession {
    context: ITfContext,
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
        let sink = state::composition_sink()
            .ok_or_else(|| windows_core::Error::from_hresult(windows_core::HRESULT(-1)))?;
        unsafe {
            context_composition.StartComposition(ec, &range, &sink)?;
        }

        // 3) 记录范围；枚举取回视图对象（用于显式 EndComposition）
        let mut latest: Option<ITfCompositionView> = None;
        let enum_comps = unsafe { context_composition.EnumCompositions()? };
        let mut buffer = [None];
        let mut fetched = 0u32;
        while unsafe { enum_comps.Next(&mut buffer, &mut fetched) }.is_ok() && fetched > 0 {
            latest = buffer[0].clone();
        }
        state::set_composition_parts(range.clone(), latest);
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
        state::on_composition_terminated();
        Ok(())
    }
}
