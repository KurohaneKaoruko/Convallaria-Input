//! Convallaria Input Windows 前端：TSF 文本服务。
//!
//! 模块：
//! - [`engine`]：ime-core 桥接（词典 / 设置 / 组字器）
//! - [`composition`] / [`edit`]：组字与异步编辑会话
//! - [`key_sink`]：键盘事件池（按键状态机）
//! - [`candidate_window`]：候选窗（WS_POPUP 自绘）
//! - [`session`]：按键与编辑回调共享的组字会话状态
//! - [`registry`]：TSF 注册 / 注销
//!
//! **健壮性**：所有 COM 回调经 `state::catch` 拦截 panic——输入法崩溃
//! 不允许带走宿主应用。

#![cfg(windows)]

mod candidate_window;
mod composition;
mod edit;
mod engine;
mod key_sink;
mod registry;
mod session;
mod state;

use std::rc::Rc;

use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_POINTER, S_FALSE, S_OK};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::TextServices::{
    ITfKeyEventSink, ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr,
};
use windows_core::{implement, BOOL, HRESULT, Interface, Result};

/// 本文本服务的 CLSID。
pub const TIP_CLSID: windows_core::GUID =
    windows_core::GUID::from_u128(0x8A5C7B60_4C2A_4E1F_9D3B_5C0A11B2C001);
/// CLSID 的注册表字符串形式（与 [`TIP_CLSID`] 保持同步）。
pub const TIP_CLSID_STR: &str = "{8A5C7B60-4C2A-4E1F-9D3B-5C0A11B2C001}";
/// 语言 Profile GUID（zh-CN / 0x0804）。
pub const PROFILE_GUID: windows_core::GUID =
    windows_core::GUID::from_u128(0x8A5C7B60_4C2A_4E1F_9D3B_5C0A11B2C002);
/// 中文（简体）LANGID。
pub const LANGID_ZH_CN: u16 = 0x0804;

/// 文本服务对象：负责激活 / 停用。
#[implement(ITfTextInputProcessor)]
pub struct TipService {
    session: Rc<session::Session>,
}

impl TipService {
    fn new() -> (Rc<session::Session>, Self) {
        let session = Rc::new(session::Session::new());
        let slf = Self {
            session: Rc::clone(&session),
        };
        (session, slf)
    }
}

impl ITfTextInputProcessor_Impl for TipService_Impl {
    fn Activate(&self, ptim: windows_core::Ref<'_, ITfThreadMgr>, tid: u32) -> Result<()> {
        state::catch(|| {
            self.session.tid.set(tid);
            state::init_engine();
            *self.session.sink.borrow_mut() = Some(composition::CompSink.into());

            // 建议键盘事件池
            if let Ok(mgr) = ptim.ok()
                && let Ok(keystroke) =
                    mgr.cast::<windows::Win32::UI::TextServices::ITfKeystrokeMgr>()
            {
                let sink: ITfKeyEventSink =
                    key_sink::KeySink::new(Rc::clone(&self.session)).into();
                unsafe {
                    let _ = keystroke.AdviseKeyEventSink(tid, &sink, true);
                }
            }
        });
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        state::catch(|| {
            // 敲了一半的组字串原样落定，再收会话
            let raw = self.session.pending_preedit.borrow().clone();
            if !raw.is_empty()
                && let Some(context) = self.session.context.borrow().as_ref().cloned()
            {
                *self.session.pending_commit.borrow_mut() = Some(raw);
                *self.session.pending_preedit.borrow_mut() = String::new();
                let _ = edit::request_update(&self.session, &context);
            }
            self.session.sink.borrow_mut().take();
            candidate_window::destroy(self.session.hwnd.get());
            state::drop_engine();
        });
        Ok(())
    }
}

// —— COM 自注册导出 ——

#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    // 输入法 DLL 常驻宿主进程属正常形态；返回 S_FALSE 防止使用中被卸载
    S_FALSE
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
    if registry::register() {
        S_OK
    } else {
        SELFREG_E_CLASS
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllUnregisterServer() -> HRESULT {
    registry::unregister();
    S_OK
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
            let (session, slf) = TipService::new();
            let tip: ITfTextInputProcessor = slf.into();
            // TipService 内部已持有会话引用，这里释放多余的一份
            drop(session);
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
