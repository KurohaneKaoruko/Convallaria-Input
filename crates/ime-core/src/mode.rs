//! 输入模式与双拼方案枚举（设置与引擎共用）。

use serde::{Deserialize, Serialize};

/// 输入模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputMode {
    /// 全拼。
    Quanpin,
    /// 双拼。
    Shuangpin,
    /// 五笔 86。
    Wubi,
}

impl InputMode {
    pub const ALL: [InputMode; 3] = [InputMode::Quanpin, InputMode::Shuangpin, InputMode::Wubi];

    /// 循环切换到下一模式（全拼 → 双拼 → 五笔 → 全拼）。
    pub fn next(self) -> Self {
        match self {
            InputMode::Quanpin => InputMode::Shuangpin,
            InputMode::Shuangpin => InputMode::Wubi,
            InputMode::Wubi => InputMode::Quanpin,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            InputMode::Quanpin => "全拼",
            InputMode::Shuangpin => "双拼",
            InputMode::Wubi => "五笔",
        }
    }
}

/// 双拼方案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShuangpinScheme {
    /// 小鹤双拼。
    Flypy,
    /// 自然码。
    Ziranma,
    /// 微软双拼。
    Mspy,
}

impl ShuangpinScheme {
    pub const ALL: [ShuangpinScheme; 3] = [
        ShuangpinScheme::Flypy,
        ShuangpinScheme::Ziranma,
        ShuangpinScheme::Mspy,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ShuangpinScheme::Flypy => "小鹤双拼",
            ShuangpinScheme::Ziranma => "自然码",
            ShuangpinScheme::Mspy => "微软双拼",
        }
    }
}
