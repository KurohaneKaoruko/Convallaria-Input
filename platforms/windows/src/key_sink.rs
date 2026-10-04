//! 键盘事件池：按键状态机（组字 / 选词 / 翻页 / 中英切换 / 模式热键）。
//!
//! **职责划分**：本模块只做「按键 → 引擎状态 + 待落定内容」的翻译，
//! 写文档交给异步编辑会话（`edit`）；每个回调只做一次引擎访问
//!（`state::with_engine`），并以 `state::catch` 拦截 panic。

use std::rc::Rc;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::TextServices::{ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl};
use windows_core::{implement, BOOL, Result};

use crate::engine::Engine;
use crate::session::Session;
use crate::{candidate_window, edit, state};
use ime_core::mode::InputMode;

#[implement(ITfKeyEventSink)]
pub struct KeySink {
    session: Rc<Session>,
}

impl KeySink {
    pub fn new(session: Rc<Session>) -> Self {
        Self { session }
    }
}

impl ITfKeyEventSink_Impl for KeySink_Impl {
    fn OnSetFocus(&self, _foreground: BOOL) -> Result<()> {
        state::catch(|| {
            self.session.english.set(false);
        });
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _context: windows_core::Ref<'_, ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        let vk = (wparam.0 & 0xFF) as u32;
        let want = state::catch(|| {
            state::with_engine(|engine| wants_key(&self.session, engine, vk)).unwrap_or(false)
        });
        Ok(BOOL::from(want))
    }

    fn OnTestKeyUp(
        &self,
        _context: windows_core::Ref<'_, ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL::from(false))
    }

    fn OnKeyDown(
        &self,
        context: windows_core::Ref<'_, ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        let handled = state::catch(|| {
            let Some(context) = context.ok().ok().cloned() else {
                return false;
            };
            let vk = (wparam.0 & 0xFF) as u32;
            self.session.cache_active_view(&context);
            self.session.cache_context(&context);
            let handled = state::with_engine(|engine| {
                handle_key(&self.session, engine, &context, vk)
            })
            .unwrap_or(false);
            // 有待落定内容（组字串或上屏文本）就请求异步编辑会话
            let pending = self.session.pending_commit.borrow().is_some()
                || !self.session.pending_preedit.borrow().is_empty();
            if pending {
                let _ = edit::request_update(&self.session, &context);
            }
            handled
        });
        Ok(BOOL::from(handled))
    }

    fn OnKeyUp(
        &self,
        _context: windows_core::Ref<'_, ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        let vk = (wparam.0 & 0xFF) as u32;
        if vk == 0x10 {
            // 单击 Shift：按下期间无其他按键 → 中英切换
            state::catch(|| {
                let toggle = {
                    let was_down = self.session.shift_down.get();
                    let tainted = self.session.shift_tainted.get();
                    self.session.shift_down.set(false);
                    self.session.shift_tainted.set(false);
                    was_down && !tainted
                };
                if toggle {
                    self.session.english.set(!self.session.english.get());
                    state::with_engine(|engine| refresh(&self.session, engine));
                }
            });
        }
        Ok(BOOL::from(false))
    }

    fn OnPreservedKey(
        &self,
        _context: windows_core::Ref<'_, ITfContext>,
        _rguidkey: *const windows_core::GUID,
    ) -> Result<BOOL> {
        Ok(BOOL::from(false))
    }
}

/// 该键是否打算吃掉（OnTestKeyDown 用）。
fn wants_key(session: &Session, engine: &mut Engine, vk: u32) -> bool {
    if vk == 0xC0 && ctrl_down() {
        return true;
    }
    if session.english.get() {
        return false;
    }
    match vk {
        0x41..=0x5A => true,
        0x08 | 0x31..=0x39 | 0x20 | 0x0D | 0x1B | 0xBD | 0xBB => !engine.is_empty(),
        _ => false,
    }
}

fn handle_key(
    session: &Session,
    engine: &mut Engine,
    _context: &ITfContext,
    vk: u32,
) -> bool {
    if vk == 0x10 {
        session.shift_down.set(true);
        session.shift_tainted.set(false);
        return false; // 不吃 Shift 本身
    }
    if session.shift_down.get() {
        session.shift_tainted.set(true);
    }

    // Ctrl+` 循环切换输入模式（模式名在候选窗页脚显示）
    if vk == 0xC0 && ctrl_down() {
        let mode = engine.cycle_mode();
        let (_, page, total) = engine.page_items();
        let footer = footer(mode, page, total, session.english.get());
        let anchor = session.composition_rect();
        candidate_window::show(&session.hwnd, Vec::new(), footer, anchor);
        return true;
    }

    if session.english.get() {
        return false;
    }

    match vk {
        // 字母 → 组字
        0x41..=0x5A => {
            let ch = (b'a' + (vk - 0x41) as u8) as char;
            engine.push(ch);
            *session.pending_preedit.borrow_mut() = engine.raw.clone();
            refresh(session, engine);
            true
        }
        // 数字 1-9 → 选词
        0x31..=0x39 => {
            let pick = (vk - 0x31) as usize;
            match engine.page_items().0.get(pick).map(|(text, _)| text.clone()) {
                Some(text) => {
                    *session.pending_commit.borrow_mut() = Some(text);
                    *session.pending_preedit.borrow_mut() = String::new();
                    true
                }
                None => false,
            }
        }
        // 空格 → 首选上屏
        0x20 => {
            let first = engine
                .page_items()
                .0
                .first()
                .map(|(text, _)| text.clone())
                .unwrap_or_else(|| engine.raw.clone());
            *session.pending_commit.borrow_mut() = Some(first);
            *session.pending_preedit.borrow_mut() = String::new();
            true
        }
        // 回车 → 上屏原文
        0x0D => {
            *session.pending_commit.borrow_mut() = Some(engine.raw.clone());
            *session.pending_preedit.borrow_mut() = String::new();
            true
        }
        // Esc → 清空
        0x1B => {
            engine.clear();
            *session.pending_preedit.borrow_mut() = String::new();
            candidate_window::hide(session.hwnd.get());
            true
        }
        // 退格
        0x08 => {
            engine.backspace();
            if engine.is_empty() {
                *session.pending_preedit.borrow_mut() = String::new();
                candidate_window::hide(session.hwnd.get());
            } else {
                *session.pending_preedit.borrow_mut() = engine.raw.clone();
                refresh(session, engine);
            }
            true
        }
        // 翻页：`-` 上一页、`=` 下一页
        0xBD | 0xBB => {
            if vk == 0xBB {
                engine.page += 1;
            } else {
                engine.page = engine.page.saturating_sub(1);
            }
            refresh(session, engine);
            true
        }
        _ => false,
    }
}

/// 候选窗刷新（按键路径与编辑会话回调共用）。
pub fn refresh(session: &Session, engine: &mut Engine) {
    if engine.is_empty() {
        candidate_window::hide(session.hwnd.get());
        return;
    }
    let (items, page, total) = engine.page_items();
    let mode = engine.settings.mode;
    let display: Vec<(usize, String)> = items
        .iter()
        .enumerate()
        .map(|(i, (text, _))| (i + 1, text.clone()))
        .collect();
    let footer = footer(mode, page, total, session.english.get());
    let anchor = session.composition_rect();
    candidate_window::show(&session.hwnd, display, footer, anchor);
}

/// 编辑会话写入完成后重新定位候选窗（此时组字矩形才是最新位置）。
pub fn reposition_candidates(session: &Session) {
    state::with_engine(|engine| refresh(session, engine));
}

fn footer(mode: InputMode, page: usize, total: usize, english: bool) -> String {
    let lang = if english { "英" } else { "中" };
    format!("[{}] 第 {}/{} 页  {}", mode.label(), page + 1, total, lang)
}

fn ctrl_down() -> bool {
    (unsafe { GetKeyState(0x11) } as u16) & 0x8000 != 0
}
