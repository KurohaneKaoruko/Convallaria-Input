//! 引擎槽位：每个输入线程持有一份词典/设置状态。
//!
//! 组字会话相关的共享状态在 [`crate::session`]（`Rc<Session>`），
//! 这里只负责引擎（词典 mmap + 组字器输入）的线程内生命周期。

use std::cell::RefCell;
use std::io::Write as _;

use crate::engine::Engine;

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
}

/// 初始化（或重置）本线程的引擎。
pub fn init_engine() {
    ENGINE.with(|slot| *slot.borrow_mut() = Engine::load());
}

/// 卸载引擎。
pub fn drop_engine() {
    ENGINE.with(|slot| *slot.borrow_mut() = None);
}

/// 访问本线程引擎（未加载时返回 None）。
///
/// 闭包内禁止再调用 `with_engine` 或任何会访问线程本地状态的函数。
pub fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
    ENGINE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        borrow.as_mut().map(f)
    })
}

/// COM 回调兜底：panic 不穿越 FFI 边界（否则宿主应用崩溃）。
pub fn catch<R: Default>(f: impl FnOnce() -> R) -> R {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => {
            #[cfg(debug_assertions)]
            {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(std::env::temp_dir().join("convallaria-debug.log"))
                {
                    let _ = writeln!(f, "[panic] COM 回调发生 panic，已拦截");
                }
            }
            R::default()
        }
    }
}
