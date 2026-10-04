//! ime-core：Convallaria Input 核心解码引擎。
//!
//! 平台无关的纯解码层：不依赖任何 UI / 系统 API，
//! 由各平台前端（Windows TSF 等）与词典工具共享。

pub mod composer;
pub mod dict;
pub mod format;
pub mod lm;
pub mod mode;
pub mod settings;
pub mod shuangpin;
pub mod syllable;
pub mod viterbi;
