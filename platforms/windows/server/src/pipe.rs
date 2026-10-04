//! 命名管道服务端：接受 TSF DLL 的连接，连接句柄经事件通道交给主循环。

use std::sync::mpsc::Sender;

use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_WAIT,
};
use windows_core::PCWSTR;

use crate::Event;

pub const PIPE_NAME: PCWSTR = windows_core::w!(r"\\.\pipe\convallaria-input");

/// 阻塞式接受循环：每接入一个客户端，把句柄经事件通道交给主循环。
/// 本函数不返回（进程生命周期内循环）。
pub fn spawn_listener(tx: Sender<Event>) {
    std::thread::Builder::new()
        .name("pipe-listener".into())
        .spawn(move || {
            let mut next_id = 1u64;
            loop {
                let Some(file) = create_pipe_and_wait() else {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    continue;
                };
                let id = next_id;
                next_id += 1;
                let _ = tx.send(Event::Connected {
                    id,
                    stream: file,
                });
            }
        })
        .expect("启动管道监听线程");
}

/// 创建一个管道实例并阻塞等待客户端接入；接入后返回可读写的文件句柄。
fn create_pipe_and_wait() -> Option<std::fs::File> {
    unsafe {
        let open_mode = PIPE_ACCESS_DUPLEX | windows::Win32::Storage::FileSystem::FILE_FLAG_FIRST_PIPE_INSTANCE;
        let handle = CreateNamedPipeW(
            PIPE_NAME,
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1, // 单实例：同一时间一个客户端；断开后重建
            4096,
            4096,
            0,
            None,
        );
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }
        // 阻塞等待客户端连接；客户端已连接时返回 ERROR_PIPE_CONNECTED 错误，句柄仍有效
        let _ = ConnectNamedPipe(handle, None);
        use std::os::windows::io::FromRawHandle as _;
        let file = std::fs::File::from(std::os::windows::io::OwnedHandle::from_raw_handle(handle.0 as _));
        // File::from_raw_handle 需要手动接管句柄所有权
        Some(file)
    }
}