//! 真实词典端到端集成测试（spec 场景在 48.7 万词真实数据上的抽查）。
//!
//! 词典为构建产物（`assets/dicts/convallaria.dict.bin`，不入库）；
//! 文件缺失时本测试自动跳过（CI 环境）。

use ime_core::composer::Composer;
use ime_core::dict::Dict;
use ime_core::lm::NgramLm;
use ime_core::mode::{InputMode, ShuangpinScheme};

fn real_dict() -> Option<Dict> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/dicts/convallaria.dict.bin");
    if !path.exists() {
        eprintln!("跳过：词典不存在于 {}", path.display());
        return None;
    }
    Some(Dict::open(path).expect("词典加载失败"))
}

fn push_all(composer: &mut Composer<'_>, s: &str) {
    for ch in s.chars() {
        composer.push(ch);
    }
}

fn word_texts(composer: &Composer<'_>) -> Vec<String> {
    composer
        .word_candidates(20)
        .into_iter()
        .map(|w| w.text)
        .collect()
}

fn sentence_texts(composer: &Composer<'_>, k: usize) -> Vec<String> {
    composer
        .sentence_candidates(k)
        .into_iter()
        .map(|s| s.text)
        .collect()
}

#[test]
fn 真实词典_金句解码() {
    let Some(d) = real_dict() else { return };
    let lm = NgramLm::new(&d);
    let mut c = Composer::new(&d, &lm);
    push_all(&mut c, "jintiandetianqizhenhao");
    let texts = sentence_texts(&c, 5);
    assert!(
        texts.first().is_some_and(|t| t == "今天的天气真好"),
        "整句首选不理想: {texts:?}"
    );
}

#[test]
fn 真实词典_词组候选() {
    let Some(d) = real_dict() else { return };
    let lm = NgramLm::new(&d);
    let mut c = Composer::new(&d, &lm);

    // nihao → 你好 应在前列
    push_all(&mut c, "nihao");
    let texts = word_texts(&c);
    assert!(
        texts.iter().take(3).any(|t| t == "你好"),
        "你好 应进入前三: 前 8 个 {:?}",
        &texts[..texts.len().min(8)]
    );

    // xi'an → 西安（显式分隔）
    c.clear();
    push_all(&mut c, "xi'an");
    let texts = word_texts(&c);
    assert!(
        texts.iter().take(3).any(|t| t == "西安"),
        "西安 应进入前三: 前 8 个 {:?}",
        &texts[..texts.len().min(8)]
    );
}

#[test]
fn 真实词典_五笔() {
    let Some(d) = real_dict() else { return };
    let lm = NgramLm::new(&d);
    let mut c = Composer::new(&d, &lm);
    c.set_mode(InputMode::Wubi);

    // 词组全码：中国 = khlg（wubi86 显式码）
    push_all(&mut c, "khlg");
    let texts = word_texts(&c);
    assert!(
        texts.first().is_some_and(|t| t == "中国"),
        "五笔 khlg 首候选应为「中国」: {texts:?}"
    );
}

#[test]
fn 真实词典_双拼解码() {
    let Some(d) = real_dict() else { return };
    let lm = NgramLm::new(&d);
    let mut c = Composer::new(&d, &lm);
    c.set_mode(InputMode::Shuangpin);
    c.set_scheme(ShuangpinScheme::Flypy);

    // 小鹤双拼：你(ni) 好(hc) —— 韵母 ao → c
    push_all(&mut c, "nihc");
    let texts = word_texts(&c);
    assert!(
        texts.iter().take(5).any(|t| t == "你好"),
        "小鹤双拼 nihc 应给出 你好: 前 10 个 {:?}",
        &texts[..texts.len().min(10)]
    );
}

#[test]
fn 真实词典_命名空间隔离() {
    let Some(d) = real_dict() else { return };
    // 拼音命名空间的完整码不会被裸五笔码键命中（w 前缀隔离）
    assert!(
        d.exact("ni hao").map(|v| v.is_empty()).unwrap_or(true),
        "无命名空间前缀的键不应命中"
    );
}
