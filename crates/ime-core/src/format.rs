//! 词典二进制格式（`dict.bin`）。
//!
//! 同一份布局由 `tools/dict` 写入、`ime-core` 读取，全部小端：
//!
//! ```text
//! MAGIC "CLDI" | version u32
//! word_count u32
//! word_offsets u32 × word_count        // 相对词表区起点的偏移，支持 O(1) 随机访问
//! 词记录 × word_count：len u16 | text(len) | raw_freq u32 | logp f32
//! posting_count u32
//! posting u32 × posting_count          // 按码序排列；同一码内按词频降序
//! fst_len u32 | fst 字节               // Map：key = 码，value = (posting起点 << 24) | 个数
//! bigram_count u32
//! bigram × bigram_count：prev u32 | cur u32 | logp f32   // 按 (prev, cur) 升序
//! ```
//!
//! FST 的 key 以 `p`（拼音）/ `w`（五笔）前缀区分命名空间，两种模式的查询互不干扰。

use std::ops::Range;

pub const MAGIC: [u8; 4] = *b"CLDI";
pub const FORMAT_VERSION: u32 = 1;

/// 码命名空间前缀：拼音模式。
pub const NS_PINYIN: u8 = b'p';
/// 码命名空间前缀：五笔模式。
pub const NS_WUBI: u8 = b'w';

/// 单码允许的最大 posting 数（value 打包用，2^24）。
pub(crate) const MAX_POSTINGS_PER_CODE: u64 = 1 << 24;

/// 词记录。
#[derive(Debug, Clone)]
pub struct WordRecord {
    pub text: String,
    pub raw_freq: u32,
    /// 单词先验对数概率（log 域），由源语料归一后写入。
    pub logp: f32,
}

/// 借用版词记录（热路径零拷贝）。
#[derive(Debug, Clone, Copy)]
pub struct WordRef<'a> {
    pub text: &'a str,
    pub raw_freq: u32,
    pub logp: f32,
}

/// 格式解析错误。
#[derive(Debug, thiserror::Error)]
#[error("词典格式错误: {0}")]
pub struct FormatError(pub String);

pub(crate) fn u32_at(bytes: &[u8], pos: usize, what: &str) -> Result<u32, FormatError> {
    bytes
        .get(pos..pos + 4)
        .ok_or_else(|| FormatError(format!("越界读取 {what}")))
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// 从 `pos` 读取一个词记录，返回记录与下一位置。
/// 借用版：从 `pos` 读取词记录引用（零拷贝，供热路径使用）。
pub(crate) fn read_word_ref_at(bytes: &[u8], pos: usize) -> Option<(WordRef<'_>, usize)> {
    let len = u16::from_le_bytes(bytes.get(pos..pos + 2)?.try_into().ok()?) as usize;
    let total = 2 + len + 8;
    let body = bytes.get(pos..pos + total)?;
    let text = std::str::from_utf8(&body[2..2 + len]).ok()?;
    let raw_freq = u32::from_le_bytes(body[2 + len..6 + len].try_into().ok()?);
    let logp = f32::from_le_bytes(body[6 + len..10 + len].try_into().ok()?);
    Some((
        WordRef {
            text,
            raw_freq,
            logp,
        },
        pos + total,
    ))
}

pub(crate) fn read_word_at(bytes: &[u8], pos: usize) -> Option<(WordRecord, usize)> {
    let len = u16::from_le_bytes(bytes.get(pos..pos + 2)?.try_into().ok()?) as usize;
    let total = 2 + len + 8;
    let body = bytes.get(pos..pos + total)?;
    let text = String::from_utf8(body[2..2 + len].to_vec()).ok()?;
    let raw_freq = u32::from_le_bytes(body[2 + len..6 + len].try_into().ok()?);
    let logp = f32::from_le_bytes(body[6 + len..10 + len].try_into().ok()?);
    Some((
        WordRecord {
            text,
            raw_freq,
            logp,
        },
        pos + total,
    ))
}

/// `dict.bin` 校验与区段定位结果（借用底层字节）。
#[derive(Debug)]
pub struct DictData<'a> {
    bytes: &'a [u8],
    pub word_count: u32,
    words_start: usize,
    pub(crate) postings_range: Range<usize>,
    fst_range: Range<usize>,
    pub(crate) bigram_range: Range<usize>,
}

impl<'a> DictData<'a> {
    /// 从完整文件字节解析（只做定位与边界校验，不复制）。
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FormatError> {
        if bytes.len() < 12 || bytes.get(..4) != Some(MAGIC.as_slice()) {
            return Err(FormatError("MAGIC 不匹配".into()));
        }
        let version = u32_at(bytes, 4, "version")?;
        if version != FORMAT_VERSION {
            return Err(FormatError(format!("不支持的版本 {version}")));
        }

        let mut pos = 8usize;
        let word_count = u32_at(bytes, pos, "word_count")?;
        pos += 4;
        let offsets_start = pos;
        pos += word_count as usize * 4;
        let words_start = pos;

        let mut cur = words_start;
        for i in 0..word_count {
            let (_, next) = read_word_at(bytes, cur)
                .ok_or_else(|| FormatError(format!("词表损坏 @ 词 {i}")))?;
            let expected = words_start
                + u32_at(bytes, offsets_start + i as usize * 4, "word_offsets")? as usize;
            if cur != expected {
                return Err(FormatError(format!("词 {i} 偏移不符: {cur} != {expected}")));
            }
            cur = next;
        }
        pos = cur;

        let posting_count = u32_at(bytes, pos, "posting_count")? as usize;
        pos += 4;
        let postings_start = pos;
        pos += posting_count * 4;

        let fst_len = u32_at(bytes, pos, "fst_len")? as usize;
        pos += 4;
        let fst_start = pos;
        pos += fst_len;

        let bigram_count = u32_at(bytes, pos, "bigram_count")? as usize;
        pos += 4;
        let bigram_start = pos;
        pos += bigram_count * 12;
        if pos != bytes.len() {
            return Err(FormatError(format!("长度不符: {pos} != {}", bytes.len())));
        }

        Ok(Self {
            bytes,
            word_count,
            words_start,
            postings_range: postings_start..postings_start + posting_count * 4,
            fst_range: fst_start..fst_start + fst_len,
            bigram_range: bigram_start..bigram_start + bigram_count * 12,
        })
    }

    /// 第 `i` 个词的文件内偏移（相对词表区起点）。
    pub fn word_offset(&self, i: u32) -> Option<u32> {
        let off = 12 + i as usize * 4;
        Some(u32::from_le_bytes(
            self.bytes.get(off..off + 4)?.try_into().ok()?,
        ))
    }

    /// 第 `id` 个词记录（O(1)，经偏移表）。
    pub fn word(&self, id: u32) -> Option<WordRecord> {
        let off = self.words_start + self.word_offset(id)? as usize;
        read_word_at(self.bytes, off).map(|(w, _)| w)
    }

    pub fn posting_count(&self) -> usize {
        (self.postings_range.end - self.postings_range.start) / 4
    }

    /// 第 `i` 个 posting 的词 ID。
    pub fn posting(&self, i: usize) -> Option<u32> {
        let off = self.postings_range.start + i * 4;
        Some(u32::from_le_bytes(
            self.bytes.get(off..off + 4)?.try_into().ok()?,
        ))
    }

    /// FST 字节区（交给 `fst::Map::new` 校验并打开）。
    pub fn fst_bytes(&self) -> &'a [u8] {
        &self.bytes[self.fst_range.clone()]
    }

    /// 遍历 bigram 表（prev, cur, logp）。
    pub fn bigrams(&self) -> impl Iterator<Item = (u32, u32, f32)> + '_ {
        self.bytes[self.bigram_range.clone()]
            .chunks_exact(12)
            .map(|c| {
                (
                    u32::from_le_bytes(c[0..4].try_into().unwrap()),
                    u32::from_le_bytes(c[4..8].try_into().unwrap()),
                    f32::from_le_bytes(c[8..12].try_into().unwrap()),
                )
            })
    }
}

/// 增量构建器：收集数据后一次性序列化。
#[derive(Debug, Default)]
pub struct DictBuilder {
    words: Vec<WordRecord>,
    /// 码 → 词 ID 列表（码必须互异；写入前码内按词频降序整理）。
    codes: Vec<(String, Vec<u32>)>,
    bigrams: Vec<(u32, u32, f32)>,
}

impl DictBuilder {
    pub fn push_word(&mut self, text: impl Into<String>, raw_freq: u32, logp: f32) -> u32 {
        self.words.push(WordRecord {
            text: text.into(),
            raw_freq,
            logp,
        });
        (self.words.len() - 1) as u32
    }

    pub fn push_code(&mut self, code: impl Into<String>, word_ids: Vec<u32>) {
        self.codes.push((code.into(), word_ids));
    }

    pub fn push_bigram(&mut self, prev: u32, cur: u32, logp: f32) {
        self.bigrams.push((prev, cur, logp));
    }

    pub fn word_count(&self) -> u32 {
        self.words.len() as u32
    }

    /// 序列化为完整文件字节。要求：码互异（调用方聚合保证）。
    pub fn finish(self) -> Result<Vec<u8>, FormatError> {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());

        // 词表与偏移表
        out.extend_from_slice(&(self.words.len() as u32).to_le_bytes());
        let mut offset: u32 = 0;
        let mut offsets: Vec<u8> = Vec::with_capacity(self.words.len() * 4);
        let mut body: Vec<u8> = Vec::new();
        for w in &self.words {
            offsets.extend_from_slice(&offset.to_le_bytes());
            let text = w.text.as_bytes();
            let record_len = 2 + text.len() + 8;
            body.extend_from_slice(&(text.len() as u16).to_le_bytes());
            body.extend_from_slice(text);
            body.extend_from_slice(&w.raw_freq.to_le_bytes());
            body.extend_from_slice(&w.logp.to_le_bytes());
            offset += record_len as u32;
        }
        out.extend_from_slice(&offsets);
        out.extend_from_slice(&body);

        // postings：码升序；码内按词频降序，并列按词 ID 升序（确定性）
        let mut sorted_codes = self.codes;
        sorted_codes.sort_by(|a, b| a.0.cmp(&b.0));
        let mut postings: Vec<u32> = Vec::new();
        let mut starts: Vec<(String, u64)> = Vec::new();
        for (code, mut ids) in sorted_codes {
            ids.sort_by(|&a, &b| {
                let fa = self.words[a as usize].raw_freq;
                let fb = self.words[b as usize].raw_freq;
                fb.cmp(&fa).then(a.cmp(&b))
            });
            let start = postings.len() as u64;
            if ids.len() as u64 >= MAX_POSTINGS_PER_CODE {
                return Err(FormatError(format!("码 {code} 的 posting 数超出上限")));
            }
            starts.push((code, (start << 24) | ids.len() as u64));
            postings.extend(ids);
        }
        out.extend_from_slice(&(postings.len() as u32).to_le_bytes());
        for id in &postings {
            out.extend_from_slice(&id.to_le_bytes());
        }

        // FST：key 升序插入
        let mut builder = fst::MapBuilder::memory();
        for (code, packed) in &starts {
            builder
                .insert(code.as_bytes(), *packed)
                .map_err(|e| FormatError(format!("FST 构建（码 {code:?}）: {e}")))?;
        }
        let fst_bytes = builder
            .into_inner()
            .map_err(|e| FormatError(format!("FST 收尾: {e}")))?;
        out.extend_from_slice(&(fst_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&fst_bytes);

        // bigram：升序
        let mut bigrams = self.bigrams;
        bigrams.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        out.extend_from_slice(&(bigrams.len() as u32).to_le_bytes());
        for (prev, cur, logp) in bigrams {
            out.extend_from_slice(&prev.to_le_bytes());
            out.extend_from_slice(&cur.to_le_bytes());
            out.extend_from_slice(&logp.to_le_bytes());
        }
        Ok(out)
    }
}

/// 独立 n-gram 数据文件（`lm.bin`）魔数。
pub const LM_MAGIC: [u8; 4] = *b"CLGR";

/// 序列化独立 n-gram 文件：LM_MAGIC | version | count | (prev,cur,logp)×count（升序）。
pub fn write_lm(bigrams: &[(u32, u32, f32)]) -> Vec<u8> {
    let mut sorted = bigrams.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut out = Vec::with_capacity(12 + sorted.len() * 12);
    out.extend_from_slice(&LM_MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(sorted.len() as u32).to_le_bytes());
    for (prev, cur, logp) in sorted {
        out.extend_from_slice(&prev.to_le_bytes());
        out.extend_from_slice(&cur.to_le_bytes());
        out.extend_from_slice(&logp.to_le_bytes());
    }
    out
}

/// 解析独立 n-gram 文件，返回升序 (prev, cur, logp) 表并做数值合法性校验。
pub fn parse_lm(bytes: &[u8]) -> Result<Vec<(u32, u32, f32)>, FormatError> {
    if bytes.len() < 12 || bytes.get(..4) != Some(LM_MAGIC.as_slice()) {
        return Err(FormatError("LM MAGIC 不匹配".into()));
    }
    let version = u32_at(bytes, 4, "lm version")?;
    if version != FORMAT_VERSION {
        return Err(FormatError(format!("不支持的 LM 版本 {version}")));
    }
    let count = u32_at(bytes, 8, "bigram_count")? as usize;
    if bytes.len() != 12 + count * 12 {
        return Err(FormatError(format!(
            "LM 长度不符: {} != {}",
            bytes.len(),
            12 + count * 12
        )));
    }
    let mut out = Vec::with_capacity(count);
    for c in bytes[12..].chunks_exact(12) {
        let logp = f32::from_le_bytes(c[8..12].try_into().unwrap());
        if !logp.is_finite() || logp > 0.0 {
            return Err(FormatError(format!("非法 logp: {logp}")));
        }
        out.push((
            u32::from_le_bytes(c[0..4].try_into().unwrap()),
            u32::from_le_bytes(c[4..8].try_into().unwrap()),
            logp,
        ));
    }
    Ok(out)
}
