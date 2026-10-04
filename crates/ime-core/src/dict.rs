//! 码表查询层：加载 `dict.bin`，提供 码 → 词、前缀码 → 词、bigram 查询。

use std::ops::Range;
use std::path::Path;

use fst::{IntoStreamer, Streamer};
use memmap2::Mmap;

use crate::format::{read_word_ref_at, u32_at, DictData, FormatError, WordRef, MAX_POSTINGS_PER_CODE};

/// 词典打开/查询错误。
#[derive(Debug, thiserror::Error)]
pub enum DictError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Format(#[from] FormatError),
    #[error("FST: {0}")]
    Fst(#[from] fst::Error),
}

/// 已加载的词典。内部字节为 mmap（文件）或堆持有（内存）。
#[derive(Debug)]
pub struct Dict {
    raw: RawBytes,
    word_count: u32,
    words_start: usize,
    word_offsets_start: usize,
    postings_range: Range<usize>,
    bigram_range: Range<usize>,
    fst: fst::Map<Vec<u8>>,
}

#[derive(Debug)]
enum RawBytes {
    Mmap(Mmap),
    Owned(Vec<u8>),
}

impl RawBytes {
    fn slice(&self) -> &[u8] {
        match self {
            RawBytes::Mmap(m) => m,
            RawBytes::Owned(v) => v,
        }
    }
}

/// 解包 FST value：`(起点 << 24) | 个数`。
fn unpack_packed(v: u64) -> (u64, u64) {
    (v >> 24, v & (MAX_POSTINGS_PER_CODE - 1))
}

impl Dict {
    /// 从文件打开（mmap，零拷贝）。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DictError> {
        let file = std::fs::File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        Self::build(RawBytes::Mmap(mmap))
    }

    /// 从内存字节打开（便于测试）。
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, DictError> {
        Self::build(RawBytes::Owned(bytes))
    }

    fn build(raw: RawBytes) -> Result<Self, DictError> {
        let data = DictData::parse(raw.slice())?;

        // 解析偏移表到本地 Vec，使 Dict 不再借用 raw
        let mut word_offsets = Vec::with_capacity(data.word_count as usize);
        for i in 0..data.word_count {
            word_offsets.push(
                data.word_offset(i)
                    .ok_or_else(|| FormatError(format!("缺词偏移 {i}")))?,
            );
        }

        // fst::Map 持有校验后的字节副本
        let fst = fst::Map::new(data.fst_bytes().to_vec())?;

        Ok(Self {
            word_count: data.word_count,
            words_start: 12 + data.word_count as usize * 4,
            word_offsets_start: 12,
            postings_range: data.postings_range.clone(),
            bigram_range: data.bigram_range.clone(),
            fst,
            raw,
        })
    }

    pub fn word_count(&self) -> u32 {
        self.word_count
    }

    /// 第 `i` 个 posting 的词 ID。
    fn posting(&self, i: usize) -> Option<u32> {
        let off = self.postings_range.start + i * 4;
        Some(u32::from_le_bytes(
            self.raw.slice().get(off..off + 4)?.try_into().ok()?,
        ))
    }

    /// 词记录（O(1)）。
    pub fn word(&self, id: u32) -> Option<WordRef<'_>> {
        let bytes = self.raw.slice();
        let off = self.words_start
            + u32_at(bytes, self.word_offsets_start + id as usize * 4, "词偏移").ok()? as usize;
        read_word_ref_at(bytes, off).map(|(w, _)| w)
    }

    /// 精确码查询：返回该码下的词 ID（词频降序）。
    pub fn exact(&self, code: &str) -> Option<Vec<u32>> {
        let packed = self.fst.get(code)?;
        let (start, len) = unpack_packed(packed);
        (start..start + len)
            .map(|i| self.posting(i as usize))
            .collect::<Option<Vec<_>>>()
    }

    /// 前缀码查询：所有以 `prefix` 开头的码，逐码调用 `sink(code, 词ID列表)`。
    /// 返回遍历的码数量（可早退：sink 返回 false 停止）。
    pub fn prefix(
        &self,
        prefix: &str,
        mut sink: impl FnMut(&str, &[u32]) -> bool,
    ) -> Result<usize, DictError> {
        let mut n = 0usize;
        let mut stream = self.fst.range().ge(prefix).into_stream();
        while let Some((code, packed)) = stream.next() {
            if !code.starts_with(prefix.as_bytes()) {
                break;
            }
            let (start, len) = unpack_packed(packed);
            let ids: Vec<u32> = (start..start + len)
                .map(|i| self.posting(i as usize))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| FormatError("posting 越界".into()))?;
            let code_str = std::str::from_utf8(code).map_err(|e| FormatError(format!("码非 UTF-8: {e}")))?;
            n += 1;
            if !sink(code_str, &ids) {
                break;
            }
        }
        Ok(n)
    }

    /// bigram 对数概率（log 域），未命中返回 None。
    pub fn bigram_logp(&self, prev: u32, cur: u32) -> Option<f32> {
        let bytes = self.raw.slice();
        let range = self.bigram_range.clone();
        let table = bytes.get(range.clone())?;
        let mut lo: usize = 0;
        let mut hi = range.len() / 12;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let c = table.get(mid * 12..mid * 12 + 12)?;
            let p = u32::from_le_bytes(c[0..4].try_into().ok()?);
            let q = u32::from_le_bytes(c[4..8].try_into().ok()?);
            match (p, q).cmp(&(prev, cur)) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Equal => return Some(f32::from_le_bytes(c[8..12].try_into().ok()?)),
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }
}
