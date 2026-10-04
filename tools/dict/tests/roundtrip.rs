//! 回环测试：小型样例词库 → build → ime-core 加载 → 查询一致性。

use std::collections::BTreeMap;

use dict::{build_dict, parse_luna, parse_essay, parse_wubi};

/// 最小样例语料（覆盖三种来源与多音字）。
/// 样例语料三元组：(单字表, 词表, 五笔码表)。
type Fixtures = (BTreeMap<char, (String, f32)>, Vec<(String, u32)>, Vec<(String, String, u32)>);

fn fixtures() -> Fixtures {
    let luna_text = concat!(
        "# Rime dict 样例\n---\nname: t\n...\n",
        "你\tni\t99.5%\n",
        "尼\tni\t0.3%\n",
        "好\thao\t98.2%\n",
        "号\thao\t1.7%\n",
        "西\txi\t95.0%\n",
        "安\tan\t90.0%\n",
        "中\tzhong\t99.0%\n",
        "国\tguo\t99.0%\n",
        "天\ttian\t99.0%\n",
    );
    let essay_text = concat!(
        "你好\t5000000\n",
        "西安\t800000\n",
        "中国\t90000000\n",
        "天使\t1000000\n",
    );
    let wubi_text = concat!(
        "# Rime wubi 样例\n---\nname: t\n...\n",
        "你\twqiy\t900000\n",
        "好\tvbg\t800000\n",
        "中国\tkhlk\t1220000000\n",
        "你好\twqvb\t70500000\n",
    );
    (
        parse_luna(luna_text),
        parse_essay(essay_text),
        parse_wubi(wubi_text),
    )
}

#[test]
fn 构建_加载_查询_回环() {
    let (luna, essay, wubi) = fixtures();
    let bytes = build_dict(&luna, &essay, &wubi, &BTreeMap::new(), None).expect("构建失败");
    let d = ime_core::dict::Dict::from_bytes(bytes).expect("加载失败");

    assert!(d.word_count() >= 8, "词表应含 8 个去重文本: {}", d.word_count());

    // 精确码：单字（码内按词频降序 → 你 在 尼 前）
    let ids = d.exact("p ni").expect("缺 p ni");
    let texts: Vec<&str> = ids.iter().filter_map(|&i| d.word(i)).map(|w| w.text).collect();
    assert_eq!(texts, vec!["你", "尼"], "码内应按词频降序: {texts:?}");

    // 精确码：多字词
    let ids = d.exact("p ni hao").expect("缺 p ni hao");
    assert_eq!(ids.len(), 1);
    assert_eq!(d.word(ids[0]).unwrap().text, "你好");

    // 前缀码：p ni 命中 p ni 与 p ni hao 两个码
    let mut seen = Vec::new();
    d.prefix("p ni", |code, _| {
        seen.push(code.to_string());
        true
    })
    .unwrap();
    assert!(seen.contains(&"p ni".to_string()));
    assert!(seen.contains(&"p ni hao".to_string()));

    // 五笔：单字全码 + 显式词组码
    let ids = d.exact("w khlk").expect("缺 w khlk");
    assert_eq!(d.word(ids[0]).unwrap().text, "中国");
    let ids = d.exact("w wqvb").expect("缺 w wqvb");
    assert_eq!(d.word(ids[0]).unwrap().text, "你好");

    // 五笔简码：前缀 vbg 应命中 好（全码 vbg）——码频降序后高频字在前
    let mut hits = Vec::new();
    d.prefix("w vb", |_, ids| {
        hits.extend(ids.iter().copied());
        true
    })
    .unwrap();
    assert!(hits
        .iter()
        .any(|&i| d.word(i).map(|w| w.text == "好").unwrap_or(false)));

    // 词频一致性：同文本多来源合并取较大者（中国：essay 90000000 vs wubi 码频 1220000000）
    let zhongguo = d.exact("p zhong guo").unwrap();
    assert_eq!(d.word(zhongguo[0]).unwrap().raw_freq, 1220000000);
}

#[test]
fn 解析器_单字表_多音字取高频() {
    let (luna, _, _) = fixtures();
    assert_eq!(luna.get(&'你').unwrap().0, "ni");
    assert_eq!(luna.get(&'好').unwrap().0, "hao");
    assert_eq!(luna.get(&'好').unwrap().1, 98.2);
    assert!(!luna.contains_key(&'x')); // 单字母行被过滤
}
