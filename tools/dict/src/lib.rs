//! 词典编译管线：Rime 文本词库 → 紧凑二进制（ime-core 格式）。
//!
//! 数据源：
//! - luna：单字拼音表（字 / 拼音 / 频度%），提供字频与多音字推断依据
//! - essay：词表（词 / 词频），提供多字词
//! - wubi：五笔 86 码表（字词 / 码 / 码频），显式词组码
//! - t2s：OpenCC 繁→简单字映射（essay 以繁体为主，构建期统一转为简体）

use std::collections::{BTreeMap, HashMap};

use ime_core::format::{DictBuilder, NS_PINYIN, NS_WUBI};

/// 单词拼音码最大音节数（词组级）。
const MAX_SYLLABLES: usize = 6;
/// 单字先验折扣：同频下多字词先验显著高于单字。
const CHAR_PENALTY: f32 = 5.0;

/// 词典构建错误。
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("序列化: {0}")]
    Format(#[from] ime_core::format::FormatError),
    #[error("词典: {0}")]
    Dict(#[from] ime_core::dict::DictError),
}

/// 是否 CJK 汉字（含 〇）。
fn is_han(c: char) -> bool {
    matches!(c, '\u{3007}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}')
}

fn is_han_text(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_han)
}

/// Rime dict.yaml 数据行：跳过 YAML 头（到 `...` 行），滤除空行与注释。
fn rime_data_lines(content: &str) -> impl Iterator<Item = &str> {
    content
        .lines()
        .skip_while(|l| l.trim() != "...")
        .skip(1)
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
}

/// 解析 luna 单字表：`字\t拼音[\t频度%]`，每字保留频度最高的拼音。
pub fn parse_luna(content: &str) -> BTreeMap<char, (String, f32)> {
    let mut map: BTreeMap<char, (String, f32)> = BTreeMap::new();
    for line in rime_data_lines(content) {
        let mut cols = line.split('\t');
        let (Some(text), Some(code)) = (cols.next(), cols.next()) else {
            continue;
        };
        let weight: f32 = cols
            .next()
            .and_then(|w| w.trim().trim_end_matches('%').parse().ok())
            .unwrap_or(1.0);
        let mut chars = text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            continue; // 只要单字
        };
        if !is_han(c) {
            continue;
        }
        let code = code.trim().to_lowercase().replace('ü', "v");
        if code.is_empty() || !code.chars().all(|c| c.is_ascii_lowercase()) {
            continue;
        }
        match map.entry(c) {
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert((code, weight));
            }
            std::collections::btree_map::Entry::Occupied(mut e) => {
                if weight > e.get().1 {
                    e.insert((code, weight));
                }
            }
        }
    }
    map
}

/// 解析 essay 词表：`词\t词频`，仅保留 2..=8 字纯汉字词。
pub fn parse_essay(content: &str) -> Vec<(String, u32)> {
    content
        .lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let text = cols.next()?;
            let freq: u32 = cols.next()?.trim().parse().ok()?;
            let n = text.chars().count();
            if (2..=8).contains(&n) && is_han_text(text) {
                Some((text.to_string(), freq))
            } else {
                None
            }
        })
        .collect()
}

/// 解析五笔码表：`字词\t码[\t码频]`，码为 1..=4 个小写字母。
pub fn parse_wubi(content: &str) -> Vec<(String, String, u32)> {
    rime_data_lines(content)
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let text = cols.next()?;
            let code = cols.next()?.trim().to_lowercase();
            let weight: u32 = cols.next().and_then(|w| w.trim().parse().ok()).unwrap_or(0);
            let n = text.chars().count();
            let code_ok = (1..=4).contains(&code.len()) && code.chars().all(|c| c.is_ascii_lowercase());
            if (1..=8).contains(&n) && is_han_text(text) && code_ok {
                Some((text.to_string(), code, weight))
            } else {
                None
            }
        })
        .collect()
}

/// 解析 OpenCC 繁→简单字映射：`繁字\t简字 [简字2...]`（取首个简字）。
pub fn parse_t2s(content: &str) -> BTreeMap<char, char> {
    content
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let key = cols.next()?.trim().chars().next()?;
            let simplified = cols.next()?.split_whitespace().next()?.chars().next()?;
            Some((key, simplified))
        })
        .collect()
}

/// 词条来源（决定 logp 计算方式）。
#[derive(Clone, Copy, PartialEq)]
enum Source {
    /// luna 单字：weight 为频度百分比。
    Char(f32),
    /// essay 词：freq 即词频。
    Word,
    /// 仅五笔来源：低先验。
    WubiOnly,
}

/// 编译词典二进制（可合并 n-gram 表）。
///
/// `t2s` 为繁→简单字映射（可为空表 = 不转换）。essay 词库以繁体为主，
/// 构建期统一转简体；繁简同形词条归并时词频取较大者。
pub fn build_dict(
    luna: &BTreeMap<char, (String, f32)>,
    essay: &[(String, u32)],
    wubi: &[(String, String, u32)],
    t2s: &BTreeMap<char, char>,
    lm: Option<&[(u32, u32, f32)]>,
) -> Result<Vec<u8>, BuildError> {
    let conv = |c: char| *t2s.get(&c).unwrap_or(&c);
    let char_mass: f32 = luna.values().map(|(_, w)| *w).sum();
    let word_mass: u64 = essay.iter().map(|(_, f)| *f as u64).sum();

    // —— 阶段一：词条归并（简体为键；繁简同形取词频较大者）——
    // text → (raw_freq, source)
    let mut lexicon: BTreeMap<String, (u32, Source)> = BTreeMap::new();
    let merge = |text: String, freq: u32, source: Source, lexicon: &mut BTreeMap<String, (u32, Source)>| {
        match lexicon.entry(text) {
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert((freq, source));
            }
            std::collections::btree_map::Entry::Occupied(mut e) => {
                let (f, src) = e.get_mut();
                if freq > *f {
                    *f = freq;
                    *src = source;
                }
            }
        }
    };

    // 转换后字符 → 最高频读音（供词的拼音码推断）
    let mut char_pinyin: BTreeMap<char, (String, f32)> = BTreeMap::new();
    for (&c, (pinyin, weight)) in luna {
        let cs = conv(c);
        merge(cs.to_string(), (weight * 10_000.0).round() as u32, Source::Char(*weight), &mut lexicon);
        match char_pinyin.entry(cs) {
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert((pinyin.clone(), *weight));
            }
            std::collections::btree_map::Entry::Occupied(mut e) => {
                // 同一简体字多个繁体来源：保留更高字频的读音
                if *weight > e.get().1 {
                    e.insert((pinyin.clone(), *weight));
                }
            }
        }
    }

    for (text, freq) in essay {
        let simplified: String = text.chars().map(conv).collect();
        merge(simplified, *freq, Source::Word, &mut lexicon);
    }
    for (text, _, weight) in wubi {
        let simplified: String = text.chars().map(conv).collect();
        merge(simplified, *weight, Source::WubiOnly, &mut lexicon);
    }

    // —— 阶段二：产出词表与码表 ——
    let mut builder = DictBuilder::default();
    let mut ids: BTreeMap<String, u32> = BTreeMap::new();
    for (text, (freq, source)) in &lexicon {
        let logp = match source {
            Source::Char(w) => (w / char_mass).ln() - CHAR_PENALTY,
            Source::Word => {
                if word_mass > 0 {
                    (*freq as f64 / word_mass as f64).ln() as f32
                } else {
                    -25.0
                }
            }
            Source::WubiOnly => -25.0,
        };
        let id = builder.push_word(text, *freq, logp);
        ids.insert(text.clone(), id);
    }

    let mut pinyin_codes: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut wubi_codes: BTreeMap<String, Vec<u32>> = BTreeMap::new();

    // luna 单字拼音码
    for (&c, (pinyin, _)) in luna {
        let cs = conv(c);
        if let Some(&id) = ids.get(cs.to_string().as_str()) {
            pinyin_codes
                .entry(format!("{} {}", NS_PINYIN as char, pinyin))
                .or_default()
                .push(id);
        }
    }

    // essay 词拼音码（按各字最高频读音推断；生僻字词跳过拼音码）
    for (text, _) in essay {
        let simplified: String = text.chars().map(conv).collect();
        let Some(&id) = ids.get(simplified.as_str()) else {
            continue;
        };
        let mut syllables = Vec::with_capacity(text.chars().count());
        let mut ok = true;
        for c in simplified.chars() {
            match char_pinyin.get(&c) {
                Some((p, _)) => syllables.push(p.clone()),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && syllables.len() <= MAX_SYLLABLES {
            pinyin_codes
                .entry(format!("{} {}", NS_PINYIN as char, syllables.join(" ")))
                .or_default()
                .push(id);
        }
    }

    // 五笔码（文本已转换）
    for (text, code, _) in wubi {
        let simplified: String = text.chars().map(conv).collect();
        if let Some(&id) = ids.get(simplified.as_str()) {
            wubi_codes
                .entry(format!("{} {}", NS_WUBI as char, code))
                .or_default()
                .push(id);
        }
    }

    for (code, mut list) in pinyin_codes.into_iter().chain(wubi_codes) {
        list.sort();
        list.dedup();
        builder.push_code(code, list);
    }
    if let Some(lm) = lm {
        for &(prev, cur, logp) in lm {
            builder.push_bigram(prev, cur, logp);
        }
    }
    Ok(builder.finish()?)
}

/// 训练词级 bigram 表：输入以空白分词、按行分句的语料。
///
/// 词通过词典文本映射为词 ID（词典中不存在的词跳过）。
/// 输出独立 `lm.bin`（ime-core `parse_lm` 可加载），logp = ln(共现数 / 前词数)。
pub fn build_lm(dict_bytes: &[u8], corpus: &str) -> Result<Vec<u8>, BuildError> {
    let d = ime_core::dict::Dict::from_bytes(dict_bytes.to_vec())?;
    let mut text_to_id: HashMap<&str, u32> = HashMap::with_capacity(d.word_count() as usize);
    for id in 0..d.word_count() {
        if let Some(w) = d.word(id) {
            text_to_id.entry(w.text).or_insert(id);
        }
    }

    let mut pair_count: HashMap<(u32, u32), u32> = HashMap::new();
    let mut prev_count: HashMap<u32, u32> = HashMap::new();
    for line in corpus.lines() {
        let mut prev: Option<u32> = None;
        for token in line.split_whitespace() {
            let Some(&id) = text_to_id.get(token) else {
                continue;
            };
            *prev_count.entry(id).or_insert(0) += 1;
            if let Some(p) = prev {
                *pair_count.entry((p, id)).or_insert(0) += 1;
            }
            prev = Some(id);
        }
    }

    let mut bigrams: Vec<(u32, u32, f32)> = pair_count
        .into_iter()
        .map(|((p, c), n)| {
            let pc = prev_count.get(&p).copied().unwrap_or(1).max(1);
            (p, c, (n as f64 / pc as f64).ln() as f32)
        })
        .collect();
    bigrams.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    Ok(ime_core::format::write_lm(&bigrams))
}

/// 读原始文件 → 输出二进制词典（可选合并 `lm.bin`；`t2s_path` 提供繁简映射）。
pub fn build_from_files(
    luna_path: &std::path::Path,
    essay_path: &std::path::Path,
    wubi_path: &std::path::Path,
    t2s_path: Option<&std::path::Path>,
    lm_path: Option<&std::path::Path>,
    out_path: &std::path::Path,
) -> Result<(), BuildError> {
    let luna = parse_luna(&std::fs::read_to_string(luna_path)?);
    let essay = parse_essay(&std::fs::read_to_string(essay_path)?);
    let wubi = parse_wubi(&std::fs::read_to_string(wubi_path)?);
    let t2s = match t2s_path {
        Some(p) => parse_t2s(&std::fs::read_to_string(p)?),
        None => BTreeMap::new(),
    };
    let lm = match lm_path {
        Some(p) => Some(ime_core::format::parse_lm(&std::fs::read(p)?)?),
        None => None,
    };
    let bytes = build_dict(&luna, &essay, &wubi, &t2s, lm.as_deref())?;
    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out_path, bytes)?;
    Ok(())
}
