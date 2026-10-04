//! 双拼方案：全拼音节 ↔ 双拼两键码 的双向映射。
//!
//! 映射规则照搬 Rime 官方三套 schema 的 speller/algebra（rime-double-pinyin 仓库，
//! LGPL-3.0；映射表本身是事实数据）。实现为正向变换函数（音节 → 两键码），
//! 运行时对全部音节求逆得到（两键码 → 音节列表）。

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::mode::ShuangpinScheme;

/// 自然码/微软中 ch 的键位（小鹤为 i）。
const ZH: [char; 3] = ['v', 'v', 'v'];
const CH: [char; 3] = ['i', 'i', 'i'];
const SH: [char; 3] = ['u', 'u', 'u'];

fn idx(s: ShuangpinScheme) -> usize {
    match s {
        ShuangpinScheme::Flypy => 0,
        ShuangpinScheme::Ziranma => 1,
        ShuangpinScheme::Mspy => 2,
    }
}

/// 韵母 → 键位表（按最长匹配优先排列）。
const FINAL_KEYS: [[(&str, char); 27]; 3] = [
    // 小鹤
    [
        ("iong", 's'),
        ("uang", 'l'),
        ("iang", 'l'),
        ("uai", 'k'),
        ("ing", 'k'),
        ("uan", 'r'),
        ("ue", 't'),
        ("ve", 't'),
        ("un", 'y'),
        ("uo", 'o'),
        ("ie", 'p'),
        ("ong", 's'),
        ("ang", 'h'),
        ("ian", 'm'),
        ("an", 'j'),
        ("ou", 'z'),
        ("ia", 'x'),
        ("ua", 'x'),
        ("iao", 'n'),
        ("ao", 'c'),
        ("ui", 'v'),
        ("in", 'b'),
        ("iu", 'q'),
        ("ei", 'w'),
        ("ai", 'd'),
        ("en", 'f'),
        ("eng", 'g'),
    ],
    // 自然码
    [
        ("iong", 's'),
        ("uang", 'd'),
        ("iang", 'd'),
        ("uai", 'y'),
        ("ing", 'y'),
        ("uan", 'r'),
        ("van", 'r'),
        ("ue", 't'),
        ("ve", 't'),
        ("un", 'p'),
        ("vn", 'p'),
        ("uo", 'o'),
        ("ong", 's'),
        ("ang", 'h'),
        ("ian", 'm'),
        ("an", 'j'),
        ("iao", 'c'),
        ("ao", 'k'),
        ("ai", 'l'),
        ("ei", 'z'),
        ("ie", 'x'),
        ("ui", 'v'),
        ("ou", 'b'),
        ("in", 'n'),
        ("iu", 'q'),
        ("ia", 'w'),
        ("ua", 'w'),
    ],
    // 微软
    [
        ("iong", 's'),
        ("uang", 'd'),
        ("iang", 'd'),
        ("uai", 'y'),
        ("ing", ';'),
        ("uan", 'r'),
        ("van", 'r'),
        ("ue", 't'),
        ("ve", 't'),
        ("un", 'p'),
        ("vn", 'p'),
        ("uo", 'o'),
        ("ong", 's'),
        ("ang", 'h'),
        ("ian", 'm'),
        ("an", 'j'),
        ("iao", 'c'),
        ("ao", 'k'),
        ("ai", 'l'),
        ("ei", 'z'),
        ("ie", 'x'),
        ("ui", 'v'),
        ("ou", 'b'),
        ("in", 'n'),
        ("iu", 'q'),
        ("ia", 'w'),
        ("ua", 'w'),
    ],
];

/// 韵母键位（精确匹配，最长优先）。
fn final_key(scheme: ShuangpinScheme, fin: &str) -> Option<char> {
    FINAL_KEYS[idx(scheme)]
        .iter()
        .find(|(pat, _)| *pat == fin)
        .map(|&(_, key)| key)
}

/// 声母（含 y/w，配合韵母判定）。
fn is_initial(c: char) -> bool {
    matches!(
        c,
        'b' | 'p'
            | 'm'
            | 'f'
            | 'd'
            | 't'
            | 'n'
            | 'l'
            | 'g'
            | 'k'
            | 'h'
            | 'j'
            | 'q'
            | 'x'
            | 'r'
            | 'z'
            | 'c'
            | 's'
            | 'y'
            | 'w'
    )
}

/// 把一个完整拼音音节变换为该方案的两键码；无法表示时返回 None。
///
/// 变换语义与 Rime 官方三套 schema 的 speller/algebra 一致。
pub fn encode(scheme: ShuangpinScheme, syllable: &str) -> Option<String> {
    let s = syllable.to_lowercase();
    if s.is_empty() || s == "xx" {
        return None; // rime: erase/^xx$/
    }
    let i = idx(scheme);

    // 1) 拆 (声母简键, 韵母)
    let (head, fin): (char, &str) = if let Some(rest) = s.strip_prefix("sh") {
        (SH[i], rest)
    } else if let Some(rest) = s.strip_prefix("ch") {
        (CH[i], rest)
    } else if let Some(rest) = s.strip_prefix("zh") {
        (ZH[i], rest)
    } else if is_initial(s.chars().next().unwrap()) && s.len() >= 2 {
        (s.chars().next().unwrap(), &s[1..])
    } else {
        // 零声母
        let first = s.chars().next().unwrap();
        if s.chars().count() == 1 {
            return Some(format!("{first}{first}")); // a → aa
        }
        // 整体韵母键（ang→ah、ai→ad、ou→oz…）
        if let Some(key) = final_key(scheme, &s) {
            return Some(format!("{first}{key}"));
        }
        // 微软 er 特例：or
        if scheme == ShuangpinScheme::Mspy && s == "er" {
            return Some("or".into());
        }
        // 兜底：余部作韵母（aan 形态语义）
        if let Some(key) = final_key(scheme, &s[1..]) {
            return Some(format!("{first}{key}"));
        }
        return if s.chars().count() == 2 {
            Some(s)
        } else {
            None
        };
    };

    // 2) 韵母 → 键
    if fin.is_empty() {
        return None;
    }
    if let Some(key) = final_key(scheme, fin) {
        return Some(format!("{head}{key}"));
    }
    // 单字母韵母（zhi 的 i、zhu 的 u、ma 的 a…）
    if fin.chars().count() == 1 {
        return Some(format!("{head}{fin}"));
    }
    None
}

/// 单字音节表：由音节模块提供全集。
fn all_syllables() -> Vec<String> {
    crate::syllable::SYLLABLES
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// 两键码 → 音节列表（运行时求逆，进程内缓存）。
pub fn decode_map(scheme: ShuangpinScheme) -> &'static HashMap<String, Vec<String>> {
    static MAPS: OnceLock<[HashMap<String, Vec<String>>; 3]> = OnceLock::new();
    &MAPS.get_or_init(|| {
        [
            ShuangpinScheme::Flypy,
            ShuangpinScheme::Ziranma,
            ShuangpinScheme::Mspy,
        ]
        .map(|sc| {
            let mut m: HashMap<String, Vec<String>> = HashMap::new();
            for syl in all_syllables() {
                if let Some(code) = encode(sc, &syl) {
                    m.entry(code).or_default().push(syl.clone());
                }
                // 两键原样形态也入表（ya / er / ou 等零声母直拼）
                if syl.chars().count() == 2 {
                    m.entry(syl.clone()).or_default().push(syl);
                }
            }
            m
        })
    })[idx(scheme)]
}

/// 两键码 → 音节列表。
pub fn decode(scheme: ShuangpinScheme, code: &str) -> &[String] {
    static EMPTY: Vec<String> = Vec::new();
    decode_map(scheme)
        .get(code)
        .map(Vec::as_slice)
        .unwrap_or(&EMPTY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 小鹤关键映射() {
        assert_eq!(
            encode(ShuangpinScheme::Flypy, "shuang").as_deref(),
            Some("ul")
        );
        assert_eq!(
            encode(ShuangpinScheme::Flypy, "zhuang").as_deref(),
            Some("vl")
        );
        assert_eq!(
            encode(ShuangpinScheme::Flypy, "mian").as_deref(),
            Some("mm")
        );
        assert_eq!(encode(ShuangpinScheme::Flypy, "ang").as_deref(), Some("ah"));
        assert_eq!(encode(ShuangpinScheme::Flypy, "an").as_deref(), Some("aj"));
        assert_eq!(encode(ShuangpinScheme::Flypy, "ni").as_deref(), Some("ni"));
        assert_eq!(
            encode(ShuangpinScheme::Flypy, "niang").as_deref(),
            Some("nl")
        );
        assert_eq!(
            encode(ShuangpinScheme::Flypy, "ling").as_deref(),
            Some("lk")
        );
    }

    #[test]
    fn 自然码与微软映射() {
        assert_eq!(
            encode(ShuangpinScheme::Ziranma, "shuang").as_deref(),
            Some("ud")
        );
        assert_eq!(
            encode(ShuangpinScheme::Mspy, "shuang").as_deref(),
            Some("ud")
        );
        assert_eq!(
            encode(ShuangpinScheme::Ziranma, "ling").as_deref(),
            Some("ly")
        );
        assert_eq!(encode(ShuangpinScheme::Mspy, "ling").as_deref(), Some("l;"));
        assert!(encode(ShuangpinScheme::Mspy, "er").is_some());
    }

    #[test]
    fn 同一按键不同方案还原不同() {
        // "ul"：小鹤 → shuang；自然码 → sh+ai = shai
        let fly = decode(ShuangpinScheme::Flypy, "ul");
        let zrm = decode(ShuangpinScheme::Ziranma, "ul");
        assert!(fly.contains(&"shuang".to_string()));
        assert!(zrm.contains(&"shai".to_string()));
        assert!(!fly.contains(&"shai".to_string()));
    }

    #[test]
    fn 解码双向一致() {
        for scheme in ShuangpinScheme::ALL {
            for syl in all_syllables() {
                if let Some(code) = encode(scheme, &syl) {
                    let back = decode(scheme, &code);
                    assert!(
                        back.iter().any(|s| s == &syl),
                        "{scheme:?}: {syl} → {code} 无法还原（得 {back:?}）"
                    );
                }
            }
        }
    }
}
