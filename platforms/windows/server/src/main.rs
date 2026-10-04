//! Convallaria Input 后台服务。
//!
//! 单线程事件驱动：命名管道（TSF DLL 连接）+ 候选窗消息泵在同一个线程上
//! 轮询，无锁无竞争。进程独立于宿主应用——这里崩溃可重启，不影响任何应用。
//!
//! 启动：由安装器注册自启动，或手动运行 `convallaria-server.exe`。
//! 日志：`%TEMP%\convallaria-server.log`。

#![cfg(windows)]

mod candidates;
mod connection;
mod engine;
mod pipe;

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use connection::Connection;

/// 服务内部事件（管道线程 / 候选窗 → 主循环）。
pub enum Event {
    /// 新客户端连接。
    Connected { id: u64, stream: std::fs::File },
    /// 客户端断开。
    Disconnected(u64),
    /// 客户端按键。
    Key {
        conn: u64,
        vk: u32,
        ch: Option<char>,
        ctrl: bool,
        shift: bool,
    },
    /// 客户端上报组字矩形（候选窗定位）。
    Caret {
        conn: u64,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
    /// 候选窗点击了第 n 个候选。
    CandidateClicked(usize),
}

fn log(msg: &str) {
    let path = std::env::temp_dir().join("convallaria-server.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write as _;
        let _ = writeln!(f, "{msg}");
    }
}

fn main() {
    // 单实例：已有服务在跑就退出
    unsafe {
        let mutex = windows::Win32::System::Threading::CreateMutexW(
            None,
            false,
            windows_core::w!("Global\\ConvallariaInputServer"),
        )
        .unwrap_or_default();
        if windows::Win32::System::Threading::WaitForSingleObject(mutex, 0)
            == windows::Win32::Foundation::WAIT_TIMEOUT
        {
            return; // 已有实例
        }
    }

    let (tx, rx): (Sender<Event>, Receiver<Event>) = mpsc::channel();
    pipe::spawn_listener(tx.clone());
    candidates::set_click_handler(Box::new(move |index| {
        let _ = tx.send(Event::CandidateClicked(index));
    }));
    log("服务启动");

    let mut conns: HashMap<u64, Connection> = HashMap::new();
    let mut active_conn: Option<u64> = None;

    loop {
        // 候选窗消息泵 + 100ms 节拍
        candidates::pump(Duration::from_millis(100));

        while let Ok(ev) = rx.try_recv() {
            match ev {
                Event::Connected { id, stream } => {
                    log(&format!("连接 {id} 已接入"));
                    conns.insert(id, Connection::new(id, stream));
                    active_conn = Some(id);
                }
                Event::Disconnected(id) => {
                    conns.remove(&id);
                    if active_conn == Some(id) {
                        active_conn = None;
                        candidates::hide_window();
                    }
                    log(&format!("连接 {id} 已断开"));
                }
                Event::Key {
                    conn,
                    vk,
                    ch,
                    ctrl,
                    shift,
                } => {
                    active_conn = Some(conn);
                    if let Some(c) = conns.get_mut(&conn) {
                        let effect = c.on_key(vk, ch, ctrl, shift);
                        if active_conn == Some(conn) {
                            candidates::sync_candidates(c.candidate_items(), c.footer());
                        }
                        c.send_effect(&effect);
                    }
                }
                Event::Caret {
                    conn,
                    x,
                    y,
                    w,
                    h,
                } => {
                    if active_conn == Some(conn) {
                        candidates::move_window(x, y, w, h);
                    }
                }
                Event::CandidateClicked(index) => {
                    if let Some(id) = active_conn
                        && let Some(c) = conns.get_mut(&id)
                        && let Some(effect) = c.select_candidate(index)
                    {
                        candidates::hide_window();
                        c.send_effect(&effect);
                    }
                }
            }
        }
    }
}
