//! 音节格上的 k-best Viterbi 整句解码。

use std::collections::HashMap;

use crate::format::NS_PINYIN;
use crate::lm::{LanguageModel, BOS};
use crate::syllable::Arc;

/// 原样字符哨兵词 ID（长句边界：码表无法解析的按键按原字符保留）。
pub const LITERAL: u32 = u32::MAX - 1;
/// 原样字符的固定罚分（log 域），确保真实词优先。
pub const LITERAL_PENALTY: f32 = -20.0;
/// 每个位置参与转移的最大词状态数（全局裁剪，控制整句解码耗时）。
pub const STATE_CAP: usize = 64;

/// 解码路径中的一个词槽。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// 词 ID；[`LITERAL`] 表示原样字符。
    pub word: u32,
    /// 字符区间 `[start, end)`（相对原始组字串）。
    pub start: usize,
    pub end: usize,
}

/// 一条整句候选。
#[derive(Debug, Clone, PartialEq)]
pub struct Sentence {
    pub slots: Vec<Slot>,
    pub text: String,
    pub score: f32,
}

/// 钉选：`[start, end)` 区间强制使用 `word`。
pub type Pin = (usize, usize, u32);

#[derive(Clone)]
struct Entry {
    /// 本词起始字符下标（也是回溯边界）。
    start: usize,
    score: f32,
    prev_word: u32,
    prev_k: usize,
}

/// 状态集合：pos → 词 → top-`beam` 条来源。
type States = Vec<HashMap<u32, Vec<Entry>>>;

fn insert(states: &mut States, pos: usize, word: u32, e: Entry, beam: usize) {
    let list = states[pos].entry(word).or_default();
    match list.iter().position(|x| x.score <= e.score) {
        Some(i) if list.len() >= beam => {
            list.insert(i, e);
            list.pop();
        }
        Some(i) => list.insert(i, e),
        None if list.len() < beam => list.push(e),
        _ => {}
    }
}

/// 把弧按起点分组并限制在 `[0, n]` 内。
fn arcs_by_start(arcs: &[Arc], n: usize) -> Vec<Vec<&Arc>> {
    let mut by_start: Vec<Vec<&Arc>> = vec![Vec::new(); n + 1];
    for arc in arcs {
        if arc.start < arc.end && arc.end <= n {
            by_start[arc.start].push(arc);
        }
    }
    by_start
}

/// k-best Viterbi 整句解码。
///
/// - `raw`：原始组字串；弧的 start/end 为其字符下标
/// - 每个位置都附加"原样字符"边（罚分 [`LITERAL_PENALTY`]），任何输入都可解码
/// - `pins` 中的区间只允许钉选词通过
/// - 返回按分数降序、按文本去重的前 `k` 条候选
pub fn decode(
    dict: &crate::dict::Dict,
    lm: &dyn LanguageModel,
    raw: &str,
    arcs: &[Arc],
    pins: &[Pin],
    k: usize,
    beam: usize,
) -> Vec<Sentence> {
    let chars: Vec<char> = raw.chars().collect();
    let n = chars.len();
    let by_start = arcs_by_start(arcs, n);
    let pin_map: HashMap<(usize, usize), u32> =
        pins.iter().map(|&(s, e, w)| ((s, e), w)).collect();

    let mut states: States = vec![HashMap::new(); n + 1];
    insert(
        &mut states,
        0,
        BOS,
        Entry {
            start: 0,
            score: 0.0,
            prev_word: BOS,
            prev_k: 0,
        },
        beam,
    );

    for pos in 0..n {
        // 快照：结束于 pos 的状态。按最优分做全局裁剪，防止状态数随输入长度爆炸
        let mut snapshot: Vec<(u32, Vec<Entry>)> = states[pos]
            .iter()
            .map(|(w, es)| (*w, es.clone()))
            .collect();
        if snapshot.is_empty() {
            continue;
        }
        snapshot.sort_by(|a, b| {
            let sa = a.1.first().map(|e| e.score).unwrap_or(f32::NEG_INFINITY);
            let sb = b.1.first().map(|e| e.score).unwrap_or(f32::NEG_INFINITY);
            sb.total_cmp(&sa)
        });
        snapshot.truncate(STATE_CAP);

        // 1) 词边
        for arc in &by_start[pos] {
            let Some(mut words) = dict
                .exact(&format!("{} {}", NS_PINYIN as char, arc.code))
            else {
                continue;
            };
            if let Some(&pinned) = pin_map.get(&(arc.start, arc.end)) {
                words.retain(|&w| w == pinned);
            }
            for word in words {
                for (prev_word, entries) in &snapshot {
                    for (vi, pe) in entries.iter().enumerate() {
                        insert(
                            &mut states,
                            arc.end,
                            word,
                            Entry {
                                start: arc.start,
                                score: pe.score
                                    + lm.score(if pos == 0 { BOS } else { *prev_word }, word),
                                prev_word: *prev_word,
                                prev_k: vi,
                            },
                            beam,
                        );
                    }
                }
            }
        }

        // 2) 原样字符边
        for (prev_word, entries) in &snapshot {
            for (vi, pe) in entries.iter().enumerate() {
                insert(
                    &mut states,
                    pos + 1,
                    LITERAL,
                    Entry {
                        start: pos,
                        score: pe.score
                            + lm.score(if pos == 0 { BOS } else { *prev_word }, LITERAL)
                            + LITERAL_PENALTY,
                        prev_word: *prev_word,
                        prev_k: vi,
                    },
                    beam,
                );
            }
        }
    }

    // 回溯 + 文本合成
    let mut out: Vec<Sentence> = Vec::new();
    for (word, entries) in &states[n] {
        for (vi, e) in entries.iter().enumerate() {
            let mut slots: Vec<Slot> = Vec::new();
            let mut score = e.score;
            let mut first = true;
            let mut cur_pos = n;
            let mut cur_word = *word;
            let mut cur_k = vi;
            while cur_word != BOS {
                let en = &states[cur_pos][&cur_word][cur_k];
                slots.push(Slot {
                    word: cur_word,
                    start: en.start,
                    end: cur_pos,
                });
                if first {
                    score = e.score;
                    first = false;
                }
                let next = (en.start, en.prev_word, en.prev_k);
                cur_pos = next.0;
                cur_word = next.1;
                cur_k = next.2;
            }
            slots.reverse();
            out.push(Sentence {
                text: render(dict, &chars, &slots),
                score,
                slots,
            });
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.dedup_by(|a, b| a.text == b.text);
    out.truncate(k);
    out
}

/// 按槽位合成文本；未被任何槽覆盖的字符按原样插入。
pub fn render(dict: &crate::dict::Dict, chars: &[char], slots: &[Slot]) -> String {
    let mut out = String::new();
    let mut cursor = 0usize;
    for s in slots {
        while cursor < s.start.min(chars.len()) {
            out.push(chars[cursor]);
            cursor += 1;
        }
        if s.word == LITERAL {
            if let Some(&c) = chars.get(s.start) {
                out.push(c);
            }
        } else if let Some(w) = dict.word(s.word) {
            out.push_str(w.text);
        }
        cursor = cursor.max(s.end);
    }
    while cursor < chars.len() {
        out.push(chars[cursor]);
        cursor += 1;
    }
    out
}
