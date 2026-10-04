//! 引擎桥接：按键状态机（服务端侧，每个连接独立一份）。

use convallaria_protocol::ServerMsg;
use ime_core::composer::Composer;
use ime_core::dict::Dict;
use ime_core::lm::NgramLm;
use ime_core::mode::InputMode;
use ime_core::settings::{ConfigStore, Settings};

pub struct EngineState {
    dict: Dict,
    settings: Settings,
    raw: String,
    page: usize,
}

impl EngineState {
    pub fn new() -> Self {
        // 每连接一份 Composer 依赖（Dict mmap 共享页，成本低）
        let dict = dict_path()
            .and_then(|p| Dict::open(p).ok())
            .unwrap_or_else(Dict::empty);
        let settings = ConfigStore::default_store()
            .load_or_init()
            .unwrap_or_default();
        Self {
            dict,
            settings,
            raw: String::new(),
            page: 0,
        }
    }

    fn with_composer<R>(&self, f: impl FnOnce(&mut Composer<'_>) -> R) -> R {
        let lm = NgramLm::new(&self.dict);
        let mut composer = Composer::new(&self.dict, &lm);
        composer.set_mode(self.settings.mode);
        composer.set_scheme(self.settings.scheme);
        for ch in self.raw.chars() {
            composer.push(ch);
        }
        f(&mut composer)
    }

    /// 处理一条按键消息，返回发给 DLL 的效果。
    pub fn on_key(&mut self, vk: u32, ch: Option<char>, ctrl: bool, _shift: bool) -> ServerMsg {
        // Ctrl+` 循环切换模式
        if vk == 0xC0 && ctrl {
            self.settings.mode = self.settings.mode.next();
            let _ = ConfigStore::default_store().save(&self.settings);
            self.raw.clear();
            return effect(false, String::new(), None);
        }
        match (vk, ch) {
            (_, Some(c)) if c.is_ascii_lowercase() => {
                self.raw.push(c);
                self.page = 0;
            }
            (0x31..=0x39, _) => {
                let pick = (vk - 0x31) as usize;
                let (items, _, _) = self.page_items();
                if let Some((text, _)) = items.get(pick) {
                    return effect(false, String::new(), Some(text.clone()));
                }
                return effect(false, self.raw.clone(), None);
            }
            (0x20, _) => {
                let first = self
                    .page_items()
                    .0
                    .first()
                    .map(|(t, _)| t.clone())
                    .unwrap_or_else(|| self.raw.clone());
                return effect(false, String::new(), Some(first));
            }
            (0x0D, _) => {
                let raw = self.raw.clone();
                return effect(false, String::new(), Some(raw));
            }
            (0x1B, _) => {
                self.raw.clear();
                self.page = 0;
            }
            (0x08, _) => {
                self.raw.pop();
                self.page = 0;
            }
            (0xBD, _) => {
                self.page = self.page.saturating_sub(1);
            }
            (0xBB, _) => {
                self.page += 1;
            }
            _ => return effect(true, self.raw.clone(), None), // 不认识的键透传
        }
        effect(true, self.raw.clone(), None)
    }

    /// 候选页（含整句首选在首页首位）。
    pub fn page_items(&self) -> (Vec<(String, u32)>, usize, usize) {
        let per = self.settings.page_size.max(1) as usize;
        let mut words: Vec<(String, u32)> = self
            .with_composer(|c| {
                c.word_candidates(60)
                    .into_iter()
                    .map(|w| (w.text, w.word))
                    .collect()
            });
        if self.settings.mode == InputMode::Quanpin
            && let Some(best) = self.with_composer(|c| {
                c.sentence_candidates(1).into_iter().next().map(|s| s.text)
            })
            && self.raw.chars().count() > 2
            && !words.iter().any(|(t, _)| *t == best)
        {
            words.insert(0, (best, u32::MAX));
        }
        let total_pages = words.len().div_ceil(per).max(1);
        let page = self.page.min(total_pages - 1);
        let items = words.iter().skip(page * per).take(per).cloned().collect();
        (items, page, total_pages)
    }

    pub fn candidate_items(&self) -> Vec<String> {
        self.page_items().0.into_iter().map(|(t, _)| t).collect()
    }

    pub fn footer(&self) -> String {
        let (_, page, total) = self.page_items();
        format!(
            "[{}] 第 {}/{} 页",
            self.settings.mode.label(),
            page + 1,
            total
        )
    }

    /// 鼠标点选第 `index` 个候选（index 为候选窗显示编号-1）。
    pub fn select_candidate(&mut self, index: usize) -> Option<ServerMsg> {
        let text = self.page_items().0.get(index)?.0.clone();
        self.raw.clear();
        self.page = 0;
        Some(effect(false, String::new(), Some(text)))
    }
}

fn effect(consumed: bool, preedit: String, commit: Option<String>) -> ServerMsg {
    ServerMsg::Effect {
        consumed,
        preedit,
        commit,
    }
}

fn dict_path() -> Option<PathBuf> {
    // 部署路径：%ProgramFiles%\Convallaria Input\<ver>\dictionary.bin → %APPDATA% 回退
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        let p = PathBuf::from(pf)
            .join("Convallaria Input")
            .join("dictionary.bin");
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
    for _ in 0..4 {
        let cand = dir.join("assets").join("dicts").join("convallaria.dict.bin");
        if cand.exists() {
            return Some(cand);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

use std::path::PathBuf;
