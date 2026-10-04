//! n-gram 训练 → 合并 → ime-core 加载 的回环验证。


use dict::{build_dict, build_lm, parse_essay, parse_luna, parse_wubi};
use ime_core::format::parse_lm;

/// 迷你词典字节（与 roundtrip 同源结构）。
fn mini_dict_bytes() -> Vec<u8> {
    let luna_text = concat!(
        "# t\n---\nname: t\n...\n",
        "你\tni\t99.0%\n",
        "好\thao\t98.0%\n",
        "天\ttian\t99.0%\n",
        "气\tqi\t90.0%\n",
    );
    let essay_text = concat!("天气\t800\n", "你好\t500\n");
    let wubi_text = concat!("# t\n---\nname: t\n...\n", "中国\tkhlk\t100\n");
    build_dict(
        &parse_luna(luna_text),
        &parse_essay(essay_text),
        &parse_wubi(wubi_text),
        &std::collections::BTreeMap::new(),
        None,
    )
    .unwrap()
}

#[test]
fn bigram训练_合并_加载_打分回环() {
    let dict_bytes = mini_dict_bytes();

    // 语料：空白分词、按行分句；「天气」出现 3 次，其中 2 次前接「你好」
    let corpus = "你好 天气 真好\n你好 天气\n天气\n";

    // 1) 训练 → lm.bin
    // 语料中「你好→天气」共现 2 次（你好 共现前驱 2 次）→ logp = ln(2/2) = 0
    // 「天气 真好」的真好 不在词典，正确跳过
    let lm_bytes = build_lm(&dict_bytes, corpus).expect("训练失败");
    let bigrams = parse_lm(&lm_bytes).expect("lm 解析失败");
    assert_eq!(bigrams.len(), 1, "{bigrams:?}");
    let (_, _, logp) = bigrams[0];
    assert_eq!(logp, 0.0);

    // logp 合法：≤ 0 且有限
    for &(_, _, logp) in &bigrams {
        assert!(logp.is_finite() && logp <= 0.0);
    }

    // 2) 合并进词典
    let merged = build_dict(
        &parse_luna(concat!(
            "# t\n---\nname: t\n...\n",
            "你\tni\t99.0%\n好\thao\t98.0%\n天\ttian\t99.0%\n气\tqi\t90.0%\n"
        )),
        &parse_essay("天气\t800\n你好\t500\n"),
        &parse_wubi("# t\n---\nname: t\n...\n中国\tkhlk\t100\n"),
        &std::collections::BTreeMap::new(),
        Some(&bigrams),
    )
    .unwrap();
    let d = ime_core::dict::Dict::from_bytes(merged).unwrap();

    // 3) ime-core 侧：bigram 命中与回退
    let text_id = |t: &str| {
        (0..d.word_count())
            .find(|&i| d.word(i).map(|w| w.text == t).unwrap_or(false))
            .unwrap()
    };
    let ni_hao = text_id("你好");
    let tian_qi = text_id("天气");
    let hit = d.bigram_logp(ni_hao, tian_qi);
    assert!(hit.is_some(), "你好→天气 应命中 bigram");
    let miss = d.bigram_logp(tian_qi, ni_hao);
    assert!(miss.is_none(), "天气→你好 未共现应未命中（回退 unigram）");
}

#[test]
fn 空语料生成空表且可解析() {
    let dict_bytes = mini_dict_bytes();
    let lm_bytes = build_lm(&dict_bytes, "").unwrap();
    assert!(parse_lm(&lm_bytes).unwrap().is_empty());
}
