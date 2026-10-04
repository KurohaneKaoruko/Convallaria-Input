//! ime-core 桥接：词典/设置加载与组字器会话。

use std::path::PathBuf;

use ime_core::composer::Composer;
use ime_core::dict::Dict;
use ime_core::lm::NgramLm;
use ime_core::mode::InputMode;
use ime_core::settings::{ConfigStore, Settings};
use ime_core::viterbi::Pin;

/// 引擎上下文：词典 + 设置 + 组字串状态。
pub struct Engine {
    pub dict: Dict,
    pub settings: Settings,
    store: ConfigStore,
    /// 组字串（按键原文）。
    pub raw: String,
    /// 逐段修正钉选（字符区间 → 词 ID）。
    pub pins: Vec<Pin>,
    /// 当前页码（候选翻页）。
    pub page: usize,
}

impl Engine {
    /// 词典查找顺序：环境变量 → %APPDATA%\Convallaria\dictionary.bin → 工作区 assets（开发态）。
    fn dict_path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("CONVALLARIA_DICT") {
            let p = PathBuf::from(p);
            if p.exists() {
                return Some(p);
            }
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let p = PathBuf::from(appdata).join("Convallaria").join("dictionary.bin");
            if p.exists() {
                return Some(p);
            }
        }
        let exe = std::env::current_exe().ok()?;
        let mut dir = exe.parent().map(PathBuf::from)?;
        for _ in 0..5 {
            let cand = dir.join("assets").join("dicts").join("convallaria.dict.bin");
            if cand.exists() {
                return Some(cand);
            }
            dir = dir.parent()?.to_path_buf();
        }
        None
    }

    /// 加载引擎；词典缺失时返回 None（前端退化为纯原文上屏）。
    pub fn load() -> Option<Self> {
        let path = Self::dict_path()?;
        let dict = Dict::open(path).ok()?;
        let store = ConfigStore::default_store();
        let settings = store.load_or_init().ok().unwrap_or_default();
        Some(Self {
            dict,
            settings,
            store,
            raw: String::new(),
            pins: Vec::new(),
            page: 0,
        })
    }

    /// 重建组字器并重放组字串（Composer 借用词典，按需构造）。
    pub fn with_composer<R>(&self, f: impl FnOnce(&mut Composer<'_>) -> R) -> R {
        let lm = NgramLm::new(&self.dict);
        let mut composer = Composer::new(&self.dict, &lm);
        composer.set_mode(self.settings.mode);
        composer.set_scheme(self.settings.scheme);
        for ch in self.raw.chars() {
            composer.push(ch);
        }
        for &(s, e, w) in &self.pins {
            composer.pin(s, e, w);
        }
        f(&mut composer)
    }

    pub fn push(&mut self, c: char) {
        self.raw.push(c);
        self.page = 0;
        self.trim_pins();
    }

    pub fn backspace(&mut self) {
        self.raw.pop();
        self.page = 0;
        self.trim_pins();
    }

    pub fn clear(&mut self) {
        self.raw.clear();
        self.pins.clear();
        self.page = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// 清除失效钉选（区间越界时）。
    fn trim_pins(&mut self) {
        let n = self.raw.chars().count();
        self.pins.retain(|&(_, e, _)| e <= n);
    }

    /// Ctrl+` 循环切换模式并持久化。
    pub fn cycle_mode(&mut self) -> InputMode {
        self.settings.mode = self.settings.mode.next();
        self.clear();
        let _ = self.store.save(&self.settings);
        self.settings.mode
    }

    /// 词组级候选（整页）。
    pub fn word_candidates(&self) -> Vec<(String, u32)> {
        self.with_composer(|c| {
            c.word_candidates(60)
                .into_iter()
                .map(|w| (w.text, w.word))
                .collect()
        })
    }

    /// 整句候选（首选在前）。
    pub fn sentence_candidates(&self, k: usize) -> Vec<String> {
        self.with_composer(|c| c.sentence_candidates(k).into_iter().map(|s| s.text).collect())
    }

    /// 候选窗数据：每页 page_size 条词组候选；整句首选合并进第一页首位。
    pub fn page_items(&self) -> (Vec<(String, u32)>, usize, usize) {
        let per = self.settings.page_size.max(1) as usize;
        let mut words = self.word_candidates();
        // 首页首位插入整句首选（与词组候选去重）
        if self.settings.mode == InputMode::Quanpin
            && let Some(best) = self.sentence_candidates(1).first()
            && self.raw.chars().count() > 2
            && !words.iter().any(|(t, _)| t == best)
        {
            words.insert(0, (best.clone(), u32::MAX));
        }
        let total_pages = words.len().div_ceil(per).max(1);
        let page = self.page.min(total_pages - 1);
        let items = words
            .iter()
            .skip(page * per)
            .take(per)
            .cloned()
            .collect();
        (items, page, total_pages)
    }

    /// 钉选「天气 → 田七」式修正：把整句首选中匹配 text 的段替换为 replacement 词。
    #[allow(dead_code)] // 5.4 候选窗「修选」交互接入时启用
    pub fn replace_segment(&mut self, segment_text: &str, replacement: u32) -> bool {
        let slot = self.with_composer(|c| {
            c.sentence_candidates(1)
                .first()
                .and_then(|s| {
                    s.slots
                        .iter()
                        .find(|slot| {
                            c.word_text(slot.word)
                                .is_some_and(|t| t == segment_text)
                        })
                        .cloned()
                })
        });
        match slot {
            Some(s) => {
                self.pins.retain(|&(ps, pe, _)| !(ps == s.start && pe == s.end));
                self.pins.push((s.start, s.end, replacement));
                true
            }
            None => false,
        }
    }
}


