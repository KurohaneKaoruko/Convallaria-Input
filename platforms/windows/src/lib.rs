//! Convallaria Input Windows 前端：TSF 文本服务（MVP 骨架，任务 5.1）。
//!
//! design R1 里程碑：能注册（regsvr32 / install.ps1）、能吃键（ITfKeyEventSink）、
//! 能上屏原文（编辑会话写入按键原文）。组字串与候选窗在 5.2-5.6 迭代。

#![cfg(windows)]

use windows_core::{implement, BOOL, HRESULT, Interface, Result};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, E_POINTER, LPARAM, S_OK, WPARAM,
};
use windows::Win32::UI::TextServices::{
    ITfKeyEventSink, ITfKeyEventSink_Impl, ITfTextInputProcessor, ITfTextInputProcessor_Impl,
    ITfThreadMgr,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;

/// 本文本服务的 CLSID。
pub const TIP_CLSID: windows_core::GUID =
    windows_core::GUID::from_u128(0x8A5C7B60_4C2A_4E1F_9D3B_5C0A11B2C001);
/// 语言 Profile GUID（zh-CN / 0x0804）。
pub const PROFILE_GUID: windows_core::GUID =
    windows_core::GUID::from_u128(0x8A5C7B60_4C2A_4E1F_9D3B_5C0A11B2C002);

/// 键处理状态：组字缓冲（MVP 仅保存原文）。
#[derive(Default)]
struct KeyState {
    buffer: String,
}

#[implement(ITfTextInputProcessor)]
struct TipService {
    state: std::sync::Arc<std::sync::Mutex<KeyState>>,
}

impl TipService {
    fn new() -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(KeyState::default())),
        }
    }
}

impl ITfTextInputProcessor_Impl for TipService_Impl {
    fn Activate(&self, ptim: windows_core::Ref<'_, ITfThreadMgr>, tid: u32) -> Result<()> {
        // 建议（advise）键盘事件池；组字与 UI 在后续任务接入
        let mgr = ptim.ok()?.clone();
        let keystroke: windows::Win32::UI::TextServices::ITfKeystrokeMgr = mgr.cast()?;
        let sink: ITfKeyEventSink = KeySink {
            state: std::sync::Arc::clone(&self.state),
        }
        .into();
        unsafe {
            keystroke.AdviseKeyEventSink(tid, &sink, true)?;
        }
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        Ok(())
    }
}

/// 独立的键盘事件池对象（与 TipService 共享组字状态）。
#[implement(ITfKeyEventSink)]
struct KeySink {
    state: std::sync::Arc<std::sync::Mutex<KeyState>>,
}

impl ITfKeyEventSink_Impl for KeySink_Impl {
    fn OnSetFocus(&self, _foreground: BOOL) -> Result<()> {
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _context: windows_core::Ref<'_, windows::Win32::UI::TextServices::ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL::from(true)) // MVP：字母与回车全吃，演示链路
    }

    fn OnTestKeyUp(
        &self,
        _context: windows_core::Ref<'_, windows::Win32::UI::TextServices::ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL::from(false))
    }

    fn OnKeyDown(
        &self,
        context: windows_core::Ref<'_, windows::Win32::UI::TextServices::ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        let vk = (wparam.0 & 0xFF) as u8;
        let mut state = self.state.lock().expect("键状态锁");
        match classify(vk) {
            Key::Char(c) => {
                state.buffer.push(c);
                Ok(BOOL::from(true))
            }
            Key::Commit => {
                // 回车：上屏原文（TODO 5.2：编辑会话 + 组字串）
                let text = std::mem::take(&mut state.buffer);
                let _ = context.ok();
                log_commit(&text);
                Ok(BOOL::from(true))
            }
            Key::Backspace => {
                state.buffer.pop();
                Ok(BOOL::from(true))
            }
            Key::Pass => Ok(BOOL::from(false)),
        }
    }

    fn OnKeyUp(
        &self,
        _context: windows_core::Ref<'_, windows::Win32::UI::TextServices::ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL::from(false))
    }

    fn OnPreservedKey(
        &self,
        _context: windows_core::Ref<'_, windows::Win32::UI::TextServices::ITfContext>,
        _rguidkey: *const windows_core::GUID,
    ) -> Result<BOOL> {
        Ok(BOOL::from(false))
    }
}
enum Key {
    Char(char),
    Commit,
    Backspace,
    Pass,
}

fn classify(vk: u8) -> Key {
    let shift = unsafe { GetKeyState(0x10) } as u16 & 0x8000 != 0;
    match vk {
        0x0D => Key::Commit,
        0x08 => Key::Backspace,
        0x41..=0x5A => {
            let base = if shift { b'A' } else { b'a' };
            Key::Char((base + (vk - 0x41)) as char)
        }
        _ => Key::Pass,
    }
}

/// 上屏占位：5.2 任务替换为 ITfContext 编辑会话写入。
fn log_commit(text: &str) {
    #[cfg(debug_assertions)]
    {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("convallaria-debug.log"))
        {
            let _ = writeln!(f, "commit: {text}");
        }
    }
}

// —— COM 自注册导出 —— //

#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_OK
}

#[unsafe(no_mangle)]
extern "system" fn DllGetClassObject(
    rclsid: *const windows_core::GUID,
    riid: *const windows_core::GUID,
    ppv: *mut *mut core::ffi::c_void,
) -> HRESULT {
    unsafe {
        if rclsid.is_null() || riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        if *rclsid != TIP_CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = ClassFactory.into();
        factory.query(riid, ppv)
    }
}

const SELFREG_E_CLASS: HRESULT = HRESULT(0x8004_0211u32 as i32);

#[unsafe(no_mangle)]
extern "system" fn DllRegisterServer() -> HRESULT {
    // 注册表写入由 install.ps1 完成（含 CLSID / CTF\TIP / 类别三项），
    // DLL 自身只需提供 DllGetClassObject 入口
    match current_dll_path() {
        Ok(_) => S_OK,
        Err(_) => SELFREG_E_CLASS,
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllUnregisterServer() -> HRESULT {
    S_OK
}

fn current_dll_path() -> Result<std::path::PathBuf> {
    let mut buf = [0u16; 512];
    let len = unsafe {
        windows::Win32::System::LibraryLoader::GetModuleFileNameW(None, &mut buf)
    };
    let s = String::from_utf16_lossy(&buf[..len as usize]);
    Ok(std::path::PathBuf::from(s))
}

// —— IClassFactory —— //

#[implement(IClassFactory)]
struct ClassFactory;

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        _outer: windows_core::Ref<'_, windows_core::IUnknown>,
        riid: *const windows_core::GUID,
        ppv: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        unsafe {
            let tip: ITfTextInputProcessor = TipService::new().into();
            hresult_to_result(tip.query(riid, ppv))
        }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}


/// HRESULT → windows_core::Result 转换。
fn hresult_to_result(hr: HRESULT) -> Result<()> {
    if hr.is_ok() { Ok(()) } else { Err(windows_core::Error::from_hresult(hr)) }
}
