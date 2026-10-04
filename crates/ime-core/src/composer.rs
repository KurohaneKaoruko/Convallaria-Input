//! 组字状态机：前端（Windows TSF 等）直接操作的顶层 API。
//!
//! 维护原始组字串、钉选与输入模式；产出词组级候选与整句候选。

use crate::dict::Dict;
use crate::format::{NS_PINYIN, NS_WUBI};
use crate::lm::LanguageModel;
use crate::mode::{InputMode, ShuangpinScheme};
use crate::shuangpin;
use crate::syllable::{word_arcs, Arc};
use crate::viterbi::{self, Sentence};

/// 词组级候选。
#[derive(Debug, Clone, PartialEq)]
pub struct WordCandidate {
    pub word: u32,
    pub text: String,
    /// 覆盖的字符区间（原始组字串）。
    pub start: usize,
    pub end: usize,
}

/// 组字器。
pub struct Composer<'a> {
    dict: &'a Dict,
    lm: &'a dyn LanguageModel,
    mode: InputMode,
    scheme: ShuangpinScheme,
    raw: String,
    pins: Vec<viterbi::Pin>,
    beam: usize,
}

impl<'a> Composer<'a> {
    pub fn new(dict: &'a Dict, lm: &'a dyn LanguageModel) -> Self {
        Self {
            dict,
            lm,
            mode: InputMode::Quanpin,
            scheme: ShuangpinScheme::Flypy,
            raw: String::new(),
            pins: Vec::new(),
            beam: 16,
        }
    }

    pub fn set_mode(&mut self, mode: InputMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> InputMode {
        self.mode
    }

    pub fn set_scheme(&mut self, scheme: ShuangpinScheme) {
        self.scheme = scheme;
    }

    pub fn scheme(&self) -> ShuangpinScheme {
        self.scheme
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// 键入一个字符（小写字母、`'`；双拼方案下还包括 `;`）。
    pub fn push(&mut self, c: char) {
        self.raw.push(c.to_ascii_lowercase());
        self.trim_pins();
    }

    /// 退格：删除末字符；其上的钉选一并清除。
    pub fn backspace(&mut self) {
        self.raw.pop();
        self.trim_pins();
    }

    pub fn clear(&mut self) {
        self.raw.clear();
        self.pins.clear();
    }

    /// 钉选：`[start, end)` 区间强制使用 `word`（逐段修正）。
    pub fn pin(&mut self, start: usize, end: usize, word: u32) {
        self.pins.retain(|&(s, e, _)| !(s == start && e == end));
        self.pins.push((start, end, word));
    }

    pub fn clear_pins(&mut self) {
        self.pins.clear();
    }

    /// 清除失效钉选（起止越过组字串末尾的）。
    fn trim_pins(&mut self) {
        let n = self.raw.chars().count();
        self.pins.retain(|&(_, e, _)| e <= n);
    }

    /// 当前模式下的查询码前缀（FST 命名空间前缀 + 模式归一化编码）。
    fn query_prefix(&self) -> Option<String> {
        let raw = self.raw.to_lowercase();
        match self.mode {
            InputMode::Quanpin | InputMode::Shuangpin => {
                let letters: String = raw.chars().filter(|&c| c != '\'').collect();
                if letters.is_empty() {
                    return None;
                }
                let code = match self.mode {
                    InputMode::Shuangpin => self.shuangpin_code(&raw),
                    _ => letters,
                };
                Some(format!("{} {}", NS_PINYIN as char, code))
            }
            InputMode::Wubi => {
                let letters: String = raw.chars().filter(|c| c.is_ascii_lowercase()).collect();
                if letters.is_empty() {
                    None
                } else {
                    Some(format!("{} {}", NS_WUBI as char, letters))
                }
            }
        }
    }

    /// 双拼码：完整两键块 → 首选音节；尾块（不足两键）忽略。
    fn shuangpin_code(&self, raw: &str) -> String {
        let normalized: String = raw.chars().filter(|&c| c != '\'').collect();
        normalized
            .chars()
            .collect::<Vec<_>>()
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| {
                let code: String = c.iter().collect();
                shuangpin::decode(self.scheme, &code)
                    .first()
                    .cloned()
                    .unwrap_or(code)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// 词组级候选：词频降序（并列按词 ID 升序，确定性）。
    ///
    /// 全拼模式下做音节感知查询：完整弧精确匹配 + 尾音节前缀匹配
    /// （`nihao` 命中 `ni hao`，`niha` 经前缀命中 `ni hao`）。
    pub fn word_candidates(&self, cap: usize) -> Vec<WordCandidate> {
        let n = self.raw.chars().count();
        if n == 0 {
            return Vec::new();
        }
        // (键, 是否精确码, 是否覆盖整个输入)
        let mut queries: Vec<(String, bool, bool)> = Vec::new();
        match self.mode {
            InputMode::Quanpin => {
                let chars: Vec<char> = self.raw.chars().collect();
                let arcs = word_arcs(&self.raw, 6);
                for a in &arcs {
                    let covers_all = a.start == 0 && a.end == n;
                    let key = format!("{} {}", NS_PINYIN as char, a.code);
                    if a.end == n {
                        queries.push((key, true, covers_all));
                    } else {
                        let tail: String = chars[a.end..].iter().collect();
                        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_lowercase()) {
                            queries.push((format!("{key} {tail}"), false, false));
                        }
                    }
                }
                if arcs.is_empty() {
                    let tail: String = chars.iter().collect();
                    if tail.chars().all(|c| c.is_ascii_lowercase()) {
                        queries.push((format!("{} {tail}", NS_PINYIN as char), false, false));
                    }
                }
            }
            _ => {
                if let Some(prefix) = self.query_prefix() {
                    queries.push((prefix, false, false));
                }
            }
        }

        let mut seen: Vec<(u32, bool)> = Vec::new(); // (词 ID, 是否覆盖整个输入)
        let push = |id: u32, full: bool, seen: &mut Vec<(u32, bool)>| {
            if let Some(e) = seen.iter_mut().find(|(w, _)| w == &id) {
                e.1 |= full; // 任一整段命中即视为整段覆盖
            } else {
                seen.push((id, full));
            }
        };
        for (key, exact, full) in &queries {
            if *exact {
                if let Some(ids) = self.dict.exact(key) {
                    for id in ids {
                        push(id, *full, &mut seen);
                    }
                }
            } else {
                let _ = self
                    .dict
                    .prefix(key, |_, ids| {
                        for &id in ids {
                            push(id, false, &mut seen);
                        }
                        seen.len() < cap * 4
                    })
                    .is_ok();
            }
            if seen.len() >= cap * 4 {
                break;
            }
        }
        let mut paired: Vec<(WordCandidate, bool)> = seen
            .into_iter()
            .filter_map(|(id, full)| {
                let w = self.dict.word(id)?;
                Some((WordCandidate {
                    word: id,
                    text: w.text.to_string(),
                    start: 0,
                    end: n,
                }, full))
            })
            .collect();
        // 完整码覆盖优先，其后词频降序；并列按词 ID 升序（确定性）
        paired.sort_by(|(a, a_full), (b, b_full)| {
            b_full
                .cmp(a_full)
                .then_with(|| {
                    let fa = self.dict.word(a.word).map(|w| w.raw_freq).unwrap_or(0);
                    let fb = self.dict.word(b.word).map(|w| w.raw_freq).unwrap_or(0);
                    fb.cmp(&fa)
                })
                .then_with(|| a.word.cmp(&b.word))
        });
        let mut out: Vec<WordCandidate> = paired.into_iter().map(|(c, _)| c).collect();
        out.truncate(cap);
        out
    }

    /// 当前模式下的整句解码弧。
    fn arcs(&self) -> Vec<Arc> {
        match self.mode {
            InputMode::Quanpin => word_arcs(&self.raw, 6),
            InputMode::Shuangpin => self.shuangpin_arcs(),
            InputMode::Wubi => Vec::new(), // 五笔不做整句
        }
    }

    /// 双拼弧：两键块对齐，逐块按方案还原音节（多义全枚举）。
    fn shuangpin_arcs(&self) -> Vec<Arc> {
        let chars: Vec<char> = self.raw.chars().collect();
        let n = chars.len();
        let mut arcs: Vec<Arc> = Vec::new();
        // 块边界 = 跳过 `'` 后每两键
        let idxs: Vec<usize> = (0..n)
            .filter(|&i| chars[i] != '\'')
            .collect();
        for start in 0..n {
            if chars[start] == '\'' {
                continue;
            }
            // start 在块序列中的序位
            let pos = idxs.iter().position(|&i| i == start).unwrap();
            let mut codes: Vec<(usize, Vec<String>)> = Vec::new(); // (end, syllables)
            let mut k = pos;
            while k + 2 <= idxs.len() {
                let a = idxs[k];
                let b = idxs[k + 1];
                if b != a + 1 {
                    break; // 中间有分隔符：块断裂
                }
                let two: String = chars[a].to_string();
                let two = format!("{}{}", two, chars[b]);
                let syls = shuangpin::decode(self.scheme, &two);
                if syls.is_empty() {
                    break;
                }
                k += 2;
                let end = b + 1;
                codes.push((end, syls.to_vec()));
            }
            if codes.is_empty() {
                continue;
            }
            // 组合展开（最多 6 音节）
            let mut acc: Vec<(usize, Vec<String>)> = vec![(start, Vec::new())];
            for &(end, ref syls) in &codes {
                let mut next_acc = Vec::new();
                for (_e, syl) in &acc {
                    if syl.len() >= 6 {
                        continue;
                    }
                    for s in syls {
                        let mut v = syl.clone();
                        v.push(s.clone());
                        next_acc.push((end, v));
                    }
                }
                acc = next_acc;
                for (end, syl) in &acc {
                    let code = syl.join(" ");
                    if !arcs
                        .iter()
                        .any(|a| a.start == start && a.end == *end && a.code == code)
                    {
                        arcs.push(Arc {
                            start,
                            end: *end,
                            code,
                        });
                    }
                }
            }
        }
        arcs
    }

    /// 整句候选（全拼 / 双拼）。
    pub fn sentence_candidates(&self, k: usize) -> Vec<Sentence> {
        viterbi::decode(self.dict, self.lm, &self.raw, &self.arcs(), &self.pins, k, self.beam)
    }

    /// 词文本（供前端显示与钉选匹配）。
    pub fn word_text(&self, word: u32) -> Option<String> {
        self.dict.word(word).map(|w| w.text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lm::{MockLm, NgramLm};

    /// 迷你词典：词 + 单字，码内按词频降序。
    fn mini_dict() -> Dict {
        use crate::format::DictBuilder;
        // (文本, 拼音码, 词频)
        let entries: Vec<(&str, &str, u32)> = vec![
            ("你好", "ni hao", 500),
            ("尼好", "ni hao", 100),
            ("今天", "jin tian", 900),
            ("天气", "tian qi", 800),
            ("真好", "zhen hao", 700),
            ("田七", "tian qi", 50),
            ("你", "ni", 800),
            ("尼", "ni", 100),
            ("好", "hao", 700),
            ("的", "de", 990),
            ("今", "jin", 300),
            ("天", "tian", 400),
            ("气", "qi", 200),
            ("真", "zhen", 300),
        ];
        let mut b = DictBuilder::default();
        for (text, _code, freq) in &entries {
            let logp = if text.chars().count() > 1 {
                (*freq as f32 / 1000.0).ln()
            } else {
                (*freq as f32 / 1000.0).ln() - 5.0
            };
            b.push_word(*text, *freq, logp);
        }
        let mut by_code: std::collections::BTreeMap<String, Vec<u32>> = Default::default();
        for (i, (_, code, _)) in entries.iter().enumerate() {
            by_code
                .entry(format!("{} {code}", NS_PINYIN as char))
                .or_default()
                .push(i as u32);
        }
        for (code, ids) in by_code {
            b.push_code(code, ids);
        }
        Dict::from_bytes(b.finish().unwrap()).unwrap()
    }

    #[test]
    fn 词组候选按词频降序且确定() {
        let d = mini_dict();
        let lm = NgramLm::new(&d);
        let mut c = Composer::new(&d, &lm);
        for ch in "ni".chars() {
            c.push(ch);
        }
        let candidates = c.word_candidates(10);
        let texts: Vec<&str> = candidates.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts.first(), Some(&"你"), "高频字应居首: {texts:?}");
        let pos = |t: &str| texts.iter().position(|x| *x == t).unwrap();
        assert!(pos("你") < pos("尼"), "同码候选应按词频降序: {texts:?}");
        // 重复查询结果确定
        let again = c.word_candidates(10);
        assert_eq!(again.first().map(|w| w.text.as_str()), Some("你"));
    }

    #[test]
    fn 词组候选_完整码() {
        let d = mini_dict();
        let lm = NgramLm::new(&d);
        let mut c = Composer::new(&d, &lm);
        for ch in "nihao".chars() {
            c.push(ch);
        }
        let candidates = c.word_candidates(10);
        let texts: Vec<&str> = candidates.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts.first(), Some(&"你好"), "{texts:?}");
    }

    #[test]
    fn 整句解码_金句() {
        let d = mini_dict();
        let lm = NgramLm::new(&d);
        let mut c = Composer::new(&d, &lm);
        for ch in "jintiandetianqizhenhao".chars() {
            c.push(ch);
        }
        let sentences = c.sentence_candidates(3);
        let texts: Vec<&str> = sentences.iter().map(|s| s.text.as_str()).collect();
        assert!(
            texts.first() == Some(&"今天的天气真好"),
            "金句解码失败: {texts:?}"
        );
        assert!(sentences.len() >= 2, "应存在次优整句候选: {texts:?}");
    }

    #[test]
    fn 整句解码_长句边界原样保留() {
        let d = mini_dict();
        let lm = NgramLm::new(&d);
        let mut c = Composer::new(&d, &lm);
        for ch in "jintian9hao".chars() {
            c.push(ch);
        }
        let s = c.sentence_candidates(3);
        assert_eq!(s.first().map(|x| x.text.as_str()), Some("今天9好"));
    }

    #[test]
    fn 逐段修正_替换后保留确认段并重解码() {
        let d = mini_dict();
        let lm = NgramLm::new(&d);
        let mut c = Composer::new(&d, &lm);
        for ch in "jintiandetianqizhenhao".chars() {
            c.push(ch);
        }
        let best = c.sentence_candidates(1).remove(0);
        let tianqi_slot = best
            .slots
            .iter()
            .find(|s| d.word(s.word).map(|w| w.text == "天气").unwrap_or(false))
            .cloned()
            .expect("缺 天气 槽");
        let tianqi_id = (0..d.word_count())
            .find(|&i| d.word(i).map(|w| w.text == "田七").unwrap_or(false))
            .expect("缺 田七");
        c.pin(tianqi_slot.start, tianqi_slot.end, tianqi_id);
        let after = c.sentence_candidates(1).remove(0);
        assert_eq!(after.text, "今天的田七真好", "{:?}", after.slots);
        assert!(
            after
                .slots
                .iter()
                .any(|s| d.word(s.word).map(|w| w.text == "今天").unwrap_or(false)),
            "已确认段「今天」应保留"
        );
    }

    #[test]
    fn 五笔候选_单字全码_简码_词组() {
        use crate::format::{DictBuilder, NS_WUBI};
        let mut b = DictBuilder::default();
        // 单字（含一级简码「的」= r）与词组
        let de = b.push_word("的", 999, -0.5);
        let hen = b.push_word("很", 800, -1.0);
        let ni = b.push_word("你", 900, -1.0);
        let hao = b.push_word("好", 700, -1.2);
        let zhongguo = b.push_word("中国", 990, -1.0);
        let nihao = b.push_word("你好", 500, -1.5);
        b.push_code(format!("{} rqyy", NS_WUBI as char), vec![de]); // 的 全码
        b.push_code(format!("{} ng", NS_WUBI as char), vec![hen]); // 很 全码(二键)
        b.push_code(format!("{} wqiy", NS_WUBI as char), vec![ni]);
        b.push_code(format!("{} vbg", NS_WUBI as char), vec![hao]);
        b.push_code(format!("{} khlk", NS_WUBI as char), vec![zhongguo]);
        b.push_code(format!("{} wqvb", NS_WUBI as char), vec![nihao]); // 你好 词组码
        let d = Dict::from_bytes(b.finish().unwrap()).unwrap();
        let lm = MockLm::new(-5.0);
        let mut c = Composer::new(&d, &lm);
        c.set_mode(InputMode::Wubi);

        // 单字全码
        for ch in "khlk".chars() {
            c.push(ch);
        }
        assert_eq!(c.word_candidates(5).first().map(|w| w.text.as_str()), Some("中国"));
        c.clear();
        // 一级简码：r → 的 应出现在前列
        c.push('r');
        assert_eq!(c.word_candidates(5).first().map(|w| w.text.as_str()), Some("的"));
        c.clear();
        // 词组编码
        for ch in "wqvb".chars() {
            c.push(ch);
        }
        assert_eq!(c.word_candidates(5).first().map(|w| w.text.as_str()), Some("你好"));
    }

    #[test]
    fn 模式循环切换() {
        assert_eq!(InputMode::Quanpin.next(), InputMode::Shuangpin);
        assert_eq!(InputMode::Shuangpin.next(), InputMode::Wubi);
        assert_eq!(InputMode::Wubi.next(), InputMode::Quanpin);
    }
}

