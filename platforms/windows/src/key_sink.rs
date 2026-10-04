//! 键盘事件池：按键状态机（组字 / 选词 / 翻页 / 中英切换 / 模式热键）。
//!
//! **借用规则**：每个回调只做一次 `state::with`；`handle_key` / `refresh`
//! 通过 `&mut ThreadCtx` 操作状态，绝不再进入 `state::with`。
//! 需要同时访问引擎与组字状态时，先在作用域内取出所有权值、结束借用后再调用
//! 组字会话（composition::* 均需 `&mut ThreadCtx`）。
//! 所有回调以 `state::catch` 包裹——panic 不允许穿越 COM 边界带走宿主应用。

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::TextServices::{ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl};
use windows_core::{implement, BOOL, Result};

use crate::{candidate_window, composition, state};
use ime_core::mode::InputMode;

#[implement(ITfKeyEventSink)]
pub struct KeySink;

impl KeySink {
    pub fn new() -> Self {
        Self
    }
}

impl Default for KeySink {
    fn default() -> Self {
        Self::new()
    }
}

impl ITfKeyEventSink_Impl for KeySink_Impl {
    fn OnSetFocus(&self, _foreground: BOOL) -> Result<()> {
        state::catch(|| {
            state::with(|t| {
                t.shift_down = false;
                t.shift_tainted = false;
            });
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
        let want = state::catch(|| state::with(|t| wants_key(t, vk)).unwrap_or(false));
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
            state::with(|t| {
                t.context = Some(context.clone());
                let _ = composition::store_active_view(t, &context);
                handle_key(t, &context, vk)
            })
            .unwrap_or(false)
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
                let toggle = state::with(|t| {
                    let was_down = t.shift_down;
                    let tainted = t.shift_tainted;
                    t.shift_down = false;
                    t.shift_tainted = false;
                    was_down && !tainted
                })
                .unwrap_or(false);
                if toggle {
                    state::with(|t| {
                        t.english = !t.english;
                        refresh(t);
                    });
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
fn wants_key(t: &mut state::ThreadCtx, vk: u32) -> bool {
    // Ctrl+` 任何时候都吃
    if vk == 0xC0 && ctrl_down() {
        return true;
    }
    if t.english {
        return false;
    }
    let Some(engine) = t.engine.as_ref() else {
        return false;
    };
    match vk {
        // 字母
        0x41..=0x5A => true,
        // 退格 / 数字 / 空格 / 回车 / Esc / 翻页：仅在组字中
        0x08 | 0x31..=0x39 | 0x20 | 0x0D | 0x1B | 0xBD | 0xBB => !engine.is_empty(),
        _ => false,
    }
}

fn handle_key(t: &mut state::ThreadCtx, context: &ITfContext, vk: u32) -> bool {
    // Shift 按下状态跟踪（单击判定）
    if vk == 0x10 {
        t.shift_down = true;
        t.shift_tainted = false;
        return false; // 不吃 Shift 本身
    }
    if t.shift_down {
        t.shift_tainted = true;
    }

    // Ctrl+` 循环切换输入模式（规格：全拼 → 双拼 → 五笔，模式可见）
    if vk == 0xC0 && ctrl_down() {
        let (mode, page, total) = match t.engine.as_mut() {
            Some(engine) => {
                let mode = engine.cycle_mode();
                let (_, page, total) = engine.page_items();
                (mode, page, total)
            }
            None => (InputMode::Quanpin, 0, 1),
        };
        let footer = footer(mode, page, total, t.english);
        let anchor = t.composition_rect();
        candidate_window::show(&mut t.hwnd, Vec::new(), footer, anchor);
        return true;
    }

    // 英文直通
    if t.english {
        return false;
    }
    if t.engine.is_none() {
        return false;
    }

    match vk {
        // 字母 → 组字
        0x41..=0x5A => {
            let ch = (b'a' + (vk - 0x41) as u8) as char;
            let raw = {
                let engine = t.engine.as_mut().expect("engine 已判空");
                engine.push(ch);
                engine.raw.clone()
            };
            let _ = composition::ensure_started(t, context);
            let _ = composition::update_text(t, context, &raw);
            refresh(t);
            true
        }
        // 数字 1-9 → 选词
        0x31..=0x39 => {
            let pick = (vk - 0x31) as usize;
            let picked = {
                let engine = t.engine.as_mut().expect("engine 已判空");
                engine
                    .page_items()
                    .0
                    .get(pick)
                    .map(|(text, _)| text.clone())
            };
            match picked {
                Some(text) => {
                    let _ = composition::commit(t, context, &text);
                    true
                }
                None => false,
            }
        }
        // 空格 → 首选上屏
        0x20 => {
            let first = {
                let engine = t.engine.as_mut().expect("engine 已判空");
                engine
                    .page_items()
                    .0
                    .first()
                    .map(|(text, _)| text.clone())
                    .unwrap_or_else(|| engine.raw.clone())
            };
            let _ = composition::commit(t, context, &first);
            true
        }
        // 回车 → 上屏原文
        0x0D => {
            let raw = {
                let engine = t.engine.as_mut().expect("engine 已判空");
                engine.raw.clone()
            };
            let _ = composition::commit(t, context, &raw);
            true
        }
        // Esc → 清空
        0x1B => {
            if let Some(engine) = t.engine.as_mut() {
                engine.clear();
            }
            let _ = composition::commit(t, context, "");
            candidate_window::hide(t.hwnd);
            true
        }
        // 退格
        0x08 => {
            let now_empty = {
                let engine = t.engine.as_mut().expect("engine 已判空");
                engine.backspace();
                let empty = engine.is_empty();
                let raw = engine.raw.clone();
                (empty, raw)
            };
            if now_empty.0 {
                let _ = composition::commit(t, context, "");
                candidate_window::hide(t.hwnd);
            } else {
                let _ = composition::update_text(t, context, &now_empty.1);
                refresh(t);
            }
            true
        }
        // 翻页：`-` 上一页、`=` 下一页
        0xBD | 0xBB => {
            if let Some(engine) = t.engine.as_mut() {
                if vk == 0xBB {
                    engine.page += 1;
                } else {
                    engine.page = engine.page.saturating_sub(1);
                }
            }
            refresh(t);
            true
        }
        _ => false,
    }
}

/// 组字串与候选窗刷新。
fn refresh(t: &mut state::ThreadCtx) {
    let Some(engine) = t.engine.as_ref() else {
        return;
    };
    if engine.is_empty() {
        candidate_window::hide(t.hwnd);
        return;
    }
    let (items, page, total) = engine.page_items();
    let mode = engine.settings.mode;
    let display: Vec<(usize, String)> = items
        .iter()
        .enumerate()
        .map(|(i, (text, _))| (i + 1, text.clone()))
        .collect();
    let footer = footer(mode, page, total, t.english);
    let anchor = t.composition_rect();
    candidate_window::show(&mut t.hwnd, display, footer, anchor);
    // 组字串文本已在 OnKeyDown 中更新
}

fn footer(mode: InputMode, page: usize, total: usize, english: bool) -> String {
    let lang = if english { "英" } else { "中" };
    format!("[{}] 第 {}/{} 页  {}", mode.label(), page + 1, total, lang)
}

fn ctrl_down() -> bool {
    (unsafe { GetKeyState(0x11) } as u16) & 0x8000 != 0
}
