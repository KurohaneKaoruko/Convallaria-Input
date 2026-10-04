//! Convallaria Input Windows 前端：TSF 文本服务。
//!
//! 模块：
//! - [`engine`]：ime-core 桥接（词典 / 设置 / 组字器）
//! - [`composition`]：编辑会话（组字开始 / 更新 / 上屏）
//! - [`key_sink`]：键盘事件池（按键状态机）
//! - [`candidate_window`]：候选窗（WS_POPUP 自绘）
//! - [`state`]：线程本地上下文

#![cfg(windows)]

mod candidate_window;
mod composition;
mod engine;
mod key_sink;
mod state;

use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_POINTER, S_OK};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::TextServices::{
    ITfKeyEventSink, ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr,
};
use windows_core::{BOOL, HRESULT, Interface, Result, implement};

/// 本文本服务的 CLSID。
pub const TIP_CLSID: windows_core::GUID =
    windows_core::GUID::from_u128(0x8A5C7B60_4C2A_4E1F_9D3B_5C0A11B2C001);
/// 语言 Profile GUID（zh-CN / 0x0804）。
pub const PROFILE_GUID: windows_core::GUID =
    windows_core::GUID::from_u128(0x8A5C7B60_4C2A_4E1F_9D3B_5C0A11B2C002);

/// 文本服务对象：负责激活 / 停用。
#[implement(ITfTextInputProcessor)]
pub struct TipService;

impl TipService {
    fn new() -> Self {
        Self
    }
}

impl ITfTextInputProcessor_Impl for TipService_Impl {
    fn Activate(&self, ptim: windows_core::Ref<'_, ITfThreadMgr>, tid: u32) -> Result<()> {
        // 线程上下文初始化
        state::init(tid);
        state::with(|t| {
            if let Ok(mgr) = ptim.ok() {
                t.thread_mgr = Some(mgr.clone());
            }
            t.composition_sink = Some(composition::CompSink.into());
        });

        // 建议键盘事件池
        if let Ok(mgr) = ptim.ok() {
            let keystroke: windows::Win32::UI::TextServices::ITfKeystrokeMgr = mgr.cast()?;
            let sink: ITfKeyEventSink = key_sink::KeySink::new().into();
            unsafe {
                keystroke.AdviseKeyEventSink(tid, &sink, true)?;
            }
        }
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        // 解除建议并清理线程状态
        state::with(|t| {
            if let Some(mgr) = t.thread_mgr.clone()
                && let Ok(keystroke) =
                    mgr.cast::<windows::Win32::UI::TextServices::ITfKeystrokeMgr>()
            {
                unsafe {
                    let _ = keystroke.UnadviseKeyEventSink(t.tid);
                }
            }
            candidate_window::destroy(t.hwnd);
        });
        state::teardown();
        Ok(())
    }
}

// —— COM 自注册导出 ——

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
    // 注册表写入由 install.ps1 完成（CLSID / CTF\TIP / 类别三项）
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
    let len = unsafe { windows::Win32::System::LibraryLoader::GetModuleFileNameW(None, &mut buf) };
    let s = String::from_utf16_lossy(&buf[..len as usize]);
    Ok(std::path::PathBuf::from(s))
}

// —— IClassFactory ——

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
            let hr = tip.query(riid, ppv);
            if hr.is_ok() {
                Ok(())
            } else {
                Err(windows_core::Error::from_hresult(hr))
            }
        }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}
