//! 解码性能基准：对照 spec 的 30ms / 150ms 预算。
//!
//! 需先构建真实词典：`cargo run -p dict -- build ... --out assets/dicts/convallaria.dict.bin`
//! 词典缺失时基准自动跳过。

use criterion::{Criterion, criterion_group, criterion_main};

fn dict_path() -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/dicts/convallaria.dict.bin");
    if p.exists() {
        Some(p)
    } else {
        eprintln!("跳过：词典不存在于 {}", p.display());
        None
    }
}

fn bench_word(c: &mut Criterion, dict: &ime_core::dict::Dict) {
    let lm = ime_core::lm::NgramLm::new(dict);
    let mut group = c.benchmark_group("词组级候选(预算30ms)");
    for raw in ["ni", "nih", "niha", "nihao", "zhongguorenmin"] {
        group.bench_function(format!("输入 {raw}"), |b| {
            b.iter_batched(
                || {
                    let mut composer = ime_core::composer::Composer::new(dict, &lm);
                    for ch in raw.chars() {
                        composer.push(ch);
                    }
                    composer
                },
                |composer| composer.word_candidates(50),
                criterion::BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

fn bench_sentence(c: &mut Criterion, dict: &ime_core::dict::Dict) {
    let lm = ime_core::lm::NgramLm::new(dict);
    // 18 个音节，贴近 20 音节预算上限
    let raw = "jintiandetianqizhenhaowomenyaochumen";
    let mut group = c.benchmark_group("整句解码(预算150ms)");
    group.bench_function(format!("输入 {raw}({}音节)", raw.len() / 2), |b| {
        b.iter_batched(
            || {
                let mut composer = ime_core::composer::Composer::new(dict, &lm);
                for ch in raw.chars() {
                    composer.push(ch);
                }
                composer
            },
            |composer| composer.sentence_candidates(5),
            criterion::BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn bench(c: &mut Criterion) {
    let Some(path) = dict_path() else {
        return;
    };
    let dict = ime_core::dict::Dict::open(&path).expect("词典加载失败");
    bench_word(c, &dict);
    bench_sentence(c, &dict);
}

criterion_group!(benches, bench);
criterion_main!(benches);
