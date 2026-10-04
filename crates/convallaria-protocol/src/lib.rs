//! 管道协议：帧 = `u32` 小端长度 + JSON 载荷。
//!
//! 两端共用：TSF DLL 是客户端，服务进程是服务端。

use serde::{Deserialize, Serialize};

/// 客户端（TSF DLL）→ 服务端。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    /// 连接握手（附客户端版本）。
    Hello {
        version: String,
    },
    /// 一次按键。
    Key {
        /// Windows 虚拟键码。
        vk: u32,
        /// 可打印字符（字母已转小写；非字符键为 None）。
        ch: Option<char>,
        ctrl: bool,
        shift: bool,
    },
    /// 组字/插入点屏幕矩形（候选窗定位）。
    Caret {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
}

/// 服务端 → 客户端。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMsg {
    /// 握手应答。
    Welcome {
        version: String,
    },
    /// 按键的处理结果。
    Effect {
        /// true = 按键已被输入法消费（宿主不再收到）；false = 透传给应用。
        consumed: bool,
        /// 文档中要显示的组字串（空串 = 收起组字）。
        preedit: String,
        /// 需要落定的最终文本（选词/上屏）。
        commit: Option<String>,
    },
    Pong,
}

/// 写一帧：长度前缀 + 载荷。
pub fn write_frame<W: std::io::Write>(w: &mut W, payload: &[u8]) -> std::io::Result<()> {
    let len = payload.len() as u32;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

/// 读一帧：阻塞直到完整帧到达（调用方自行决定是否先探测可读）。
pub fn read_frame<R: std::io::Read>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 64 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "帧长度超限",
        ));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok(payload)
}
