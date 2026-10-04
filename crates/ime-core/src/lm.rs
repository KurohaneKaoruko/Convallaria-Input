//! 语言模型抽象：解码器只依赖此 trait，n-gram 是第一个实现。
//!
//! 为后续小模型（ONNX）解码器预留：只需提供新的 [`LanguageModel`] 实现。

use crate::dict::Dict;

/// 词 ID 与 BOS（句首）约定。
pub const BOS: u32 = u32::MAX;

/// 语言模型打分接口（log 域）。
pub trait LanguageModel {
    /// 相邻词对 (prev, cur) 的对数转移分。prev 为 [`BOS`] 表示句首。
    /// bigram 未命中时内部回退 unigram 平滑。
    fn score(&self, prev: u32, cur: u32) -> f32;
}

/// n-gram 模型：unigram（词典词频）+ 可选 bigram（词典内嵌共现表）。
pub struct NgramLm<'a> {
    dict: &'a Dict,
    /// bigram 回退权重（nats）：命中 bigram 时的插值优势。
    pub backoff_penalty: f32,
}

impl<'a> NgramLm<'a> {
    pub fn new(dict: &'a Dict) -> Self {
        Self {
            dict,
            backoff_penalty: 2.5,
        }
    }
}

impl LanguageModel for NgramLm<'_> {
    fn score(&self, prev: u32, cur: u32) -> f32 {
        let uni = self.dict.word(cur).map(|w| w.logp).unwrap_or(-25.0);
        match self.dict.bigram_logp(prev, cur) {
            Some(bg) => bg.max(uni) + 0.5, // 命中：bigram 为主，不低于 unigram
            None => uni - if prev == BOS { 0.0 } else { 0.5 },
        }
    }
}

/// 测试用 Mock 模型：显式打分表，验证 trait 可替换。
pub struct MockLm {
    pub scores: std::collections::HashMap<(u32, u32), f32>,
    pub default_score: f32,
}

impl MockLm {
    pub fn new(default_score: f32) -> Self {
        Self {
            scores: std::collections::HashMap::new(),
            default_score,
        }
    }

    pub fn set(&mut self, prev: u32, cur: u32, s: f32) {
        self.scores.insert((prev, cur), s);
    }
}

impl LanguageModel for MockLm {
    fn score(&self, prev: u32, cur: u32) -> f32 {
        self.scores
            .get(&(prev, cur))
            .copied()
            .unwrap_or(self.default_score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock模型可替换trait() {
        let mut m = MockLm::new(-9.0);
        m.set(BOS, 1, -2.0);
        assert_eq!(m.score(BOS, 1), -2.0);
        assert_eq!(m.score(1, 2), -9.0);
        // trait 对象化（解码器将以 dyn 使用）
        let dyn_m: &dyn LanguageModel = &m;
        assert_eq!(dyn_m.score(BOS, 1), -2.0);
    }
}
