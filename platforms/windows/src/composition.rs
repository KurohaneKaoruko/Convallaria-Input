//! 组字（composition）：把组字串以内联形式写在文档光标处（marked text），
//! 上屏时替换为最终文本。
//!
//! 全部写操作都在**异步**读写编辑会话里进行（见 `edit` 模块）：
//! 沉浸式应用的文本存区跨进程，同步会话会让宿主崩溃。

use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext,
    ITfContextComposition, ITfInsertAtSelection, ITfRange,
    INSERT_TEXT_AT_SELECTION_FLAGS, TF_AE_END, TF_ANCHOR_END, TF_IAS_QUERYONLY, TF_SELECTION,
    TF_SELECTIONSTYLE,
};
use windows_core::{implement, Interface, Result};

use crate::session::Session;

/// 在编辑会话回调（持写锁 `ec`）里调：先落定待上屏文本，再按待显示组字串
/// 起 / 改 / 收组句，最后把光标移到写入末尾。
pub fn apply(session: &Session, context: &ITfContext, ec: u32) -> Result<()> {
    let commit = session.pending_commit.borrow_mut().take();
    let preedit = std::mem::take(&mut *session.pending_preedit.borrow_mut());

    if let Some(text) = &commit {
        commit_text(session, context, ec, text)?;
    }
    if preedit.is_empty() {
        end_composition(session, ec)?;
    } else {
        update_preedit(session, context, ec, &preedit)?;
    }
    Ok(())
}

/// 有组句就把组句范围替换为 `text` 再结束组句；没有就在选区直接插入。
fn commit_text(session: &Session, context: &ITfContext, ec: u32, text: &str) -> Result<()> {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    match session.composition.borrow().as_ref() {
        Some(composition) => {
            let range = unsafe { composition.GetRange()? };
            unsafe { range.SetText(ec, 0, &utf16)? };
            move_selection_to_end(context, ec, &range)?;
            unsafe { composition.EndComposition(ec)? };
            *session.composition.borrow_mut() = None;
        }
        None => {
            let insert: ITfInsertAtSelection = context.cast()?;
            // 不能用 NOQUERY：它不回传 range，windows-rs 会把 NULL 当作失败。
            let range = unsafe {
                insert.InsertTextAtSelection(ec, INSERT_TEXT_AT_SELECTION_FLAGS(0), &utf16)?
            };
            // 不把光标挪到末尾的话，下一次插入仍落在原处，文字会倒着堆。
            move_selection_to_end(context, ec, &range)?;
        }
    }
    Ok(())
}

/// 有组句就更新组句文本；没有就在选区起一个空组句再写入。
fn update_preedit(session: &Session, context: &ITfContext, ec: u32, preedit: &str) -> Result<()> {
    let composition = match session.composition.borrow().as_ref() {
        Some(c) => c.clone(),
        None => start_composition(session, context, ec)?,
    };
    let utf16: Vec<u16> = preedit.encode_utf16().collect();
    let range = unsafe { composition.GetRange()? };
    unsafe { range.SetText(ec, 0, &utf16)? };
    move_selection_to_end(context, ec, &range)
}

/// 在当前选区起一个空组句（用 QUERYONLY 拿到选区范围，由组句机制自己展开）。
fn start_composition(session: &Session, context: &ITfContext, ec: u32) -> Result<ITfComposition> {
    let insert: ITfInsertAtSelection = context.cast()?;
    let range = unsafe { insert.InsertTextAtSelection(ec, TF_IAS_QUERYONLY, &[])? };
    let context_composition: ITfContextComposition = context.cast()?;
    let sink = session
        .sink
        .borrow()
        .as_ref()
        .cloned()
        .ok_or_else(|| windows_core::Error::from_hresult(windows_core::HRESULT(-1)))?;
    let composition = unsafe { context_composition.StartComposition(ec, &range, &sink)? };
    *session.composition.borrow_mut() = Some(composition.clone());
    Ok(composition)
}

/// 清空组句文本再结束，避免残留拼音字母。
fn end_composition(session: &Session, ec: u32) -> Result<()> {
    let composition = session.composition.borrow_mut().take();
    if let Some(composition) = composition {
        let range = unsafe { composition.GetRange()? };
        unsafe { range.SetText(ec, 0, &[])? };
        unsafe { composition.EndComposition(ec)? };
    }
    Ok(())
}

/// 把选区折叠到 `range` 末尾：写入之后光标必须跟在刚写入的文本后。
fn move_selection_to_end(context: &ITfContext, ec: u32, range: &ITfRange) -> Result<()> {
    let end = unsafe { range.Clone()? };
    unsafe { end.Collapse(ec, TF_ANCHOR_END)? };
    let selection = TF_SELECTION {
        range: std::mem::ManuallyDrop::new(Some(end)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_END,
            fInterimChar: false.into(),
        },
    };
    // SetSelection 不接管 range 所有权，调用后手动释放 ManuallyDrop。
    let result = unsafe { context.SetSelection(ec, std::slice::from_ref(&selection)) };
    drop(std::mem::ManuallyDrop::into_inner(selection.range));
    result
}

/// 组字终止监听（宿主撤销 / 应用切换焦点导致组字结束时回调）。
#[implement(ITfCompositionSink)]
pub struct CompSink;

impl ITfCompositionSink_Impl for CompSink_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _composition: windows_core::Ref<'_, ITfComposition>,
    ) -> Result<()> {
        // 宿主撤销组字：本地组字状态作废；引擎里的拼音串同步清空
        crate::state::with_engine(|engine| engine.clear());
        Ok(())
    }
}