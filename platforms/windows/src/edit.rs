//! 编辑会话：TSF 不允许直接改文档，必须经 `RequestEditSession` 申请、在回调里
//! 拿着编辑锁（ec）读写。
//!
//! **必须用异步会话**（不带 TF_ES_SYNC）：沉浸式应用的文本存区隔着进程边界，
//! 同步读写会让宿主进程直接崩溃；异步回调可能在按键处理返回之后才执行，
//! 所以组字状态通过 `Rc<Session>` 共享，回调里全量应用最新待落定内容。

use std::rc::Rc;

use windows::Win32::UI::TextServices::{
    ITfContext, ITfEditSession, ITfEditSession_Impl, TF_ES_READWRITE,
};
use windows_core::{implement, Result};

use crate::composition;
use crate::session::Session;

/// 一次异步读写会话：把最新待落定文本与组字串写进上下文。
#[implement(ITfEditSession)]
struct UpdateSession {
    context: ITfContext,
    session: Rc<Session>,
}

impl ITfEditSession_Impl for UpdateSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        // 从宿主的 C++ 调用进来：panic 不允许越过 FFI 边界。
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            composition::apply(&self.session, &self.context, ec)?;
            // 组字矩形此刻才反映最新位置：重新锚定候选窗
            crate::key_sink::reposition_candidates(&self.session);
            Ok(())
        }));
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                log_panic("编辑会话回调 panic（已拦截）");
                Err(windows_core::Error::from_hresult(windows_core::HRESULT(-1)))
            }
        }
    }
}

/// 提交一个异步读写会话。`Ok` 只代表请求被受理，写入结果记录在调试日志里。
pub fn request_update(session: &Rc<Session>, context: &ITfContext) -> Result<()> {
    let obj: ITfEditSession = UpdateSession {
        context: context.clone(),
        session: Rc::clone(session),
    }
    .into();
    let hr = unsafe {
        context.RequestEditSession(session.tid.get(), &obj, TF_ES_READWRITE)?
    };
    if hr.is_err() {
        return Err(windows_core::Error::from_hresult(hr));
    }
    Ok(())
}

fn log_panic(msg: &str) {
    #[cfg(debug_assertions)]
    {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("convallaria-debug.log"))
        {
            use std::io::Write as _;
            let _ = writeln!(f, "[panic] {msg}");
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = msg;
    }
}
