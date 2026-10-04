//! 每个客户端连接：管道收发 + 该连接独立的引擎状态。

use convallaria_protocol::{read_frame, write_frame, ClientMsg, ServerMsg};

use crate::engine::EngineState;

#[allow(dead_code)] // id / poll / is_alive 供后续双向指令使用
pub struct Connection {
    pub id: u64,
    stream: std::sync::Mutex<std::fs::File>,
    engine: EngineState,
}

impl Connection {
    pub fn new(id: u64, stream: std::fs::File) -> Self {
        Self {
            id,
            stream: std::sync::Mutex::new(stream),
            engine: EngineState::new(),
        }
    }

    /// 处理一条按键消息，返回应回给客户端的效果。
    pub fn on_key(
        &mut self,
        vk: u32,
        ch: Option<char>,
        ctrl: bool,
        shift: bool,
    ) -> ServerMsg {
        self.engine.on_key(vk, ch, ctrl, shift)
    }

    /// 候选数据（供候选窗渲染）。
    pub fn candidate_items(&self) -> Vec<String> {
        self.engine.candidate_items()
    }

    pub fn footer(&self) -> String {
        self.engine.footer()
    }

    /// 鼠标点选第 `index` 个候选（显示编号 - 1）。
    pub fn select_candidate(&mut self, index: usize) -> Option<ServerMsg> {
        self.engine.select_candidate(index)
    }

    /// 把效果写回客户端。
    pub fn send_effect(&self, effect: &ServerMsg) {
        let payload = match serde_json::to_vec(effect) {
            Ok(p) => p,
            Err(_) => return,
        };
        let mut guard = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        let _ = write_frame(&mut *guard, &payload);
    }

    /// 非阻塞读：取出客户端的一条消息（无消息返回 None）。
    #[allow(dead_code)]
    pub fn poll_message(&self) -> Option<ClientMsg> {
        let mut guard = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        use std::os::windows::io::AsRawHandle;
        let handle = guard.as_raw_handle();
        let mut available = 0u32;
        unsafe {
            let ok = windows::Win32::System::Pipes::PeekNamedPipe(
                windows::Win32::Foundation::HANDLE(handle as _),
                None,
                0,
                None,
                Some(&mut available),
                None,
            )
            .is_ok();
            if !ok || available < 4 {
                return None;
            }
        }
        match read_frame(&mut *guard) {
            Ok(payload) => serde_json::from_slice(&payload).ok(),
            Err(_) => None, // 对端关闭：由调用方通过写失败感知
        }
    }

    /// 连接是否存活（心跳帧能否写入）。
    #[allow(dead_code)]
    pub fn is_alive(&self) -> bool {
        let mut guard = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        write_frame(&mut *guard, &[]).is_ok()
    }
}
