//! dict：把文本词库编译为 ime-core 可加载的紧凑二进制。

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build") => cmd_build(&args[1..]),
        Some("lm") => cmd_lm(&args[1..]),
        Some("info") => cmd_info(&args[1..]),
        _ => {
            eprintln!("用法:");
            eprintln!("  dict build --luna <yaml> --essay <txt> --wubi <yaml> [--lm <bin>] --out <bin>");
            eprintln!("  dict lm --dict <bin> --corpus <txt> --out <bin>");
            eprintln!("  dict info <bin>");
            std::process::exit(2);
        }
    }
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn cmd_build(args: &[String]) {
    let (Some(luna), Some(essay), Some(wubi), Some(out)) = (
        flag_value(args, "--luna"),
        flag_value(args, "--essay"),
        flag_value(args, "--wubi"),
        flag_value(args, "--out"),
    ) else {
        eprintln!("build 需要 --luna/--essay/--wubi/--out 四个参数");
        std::process::exit(2);
    };
    let lm = flag_value(args, "--lm");
    let t0 = std::time::Instant::now();
    dict::build_from_files(
        &PathBuf::from(luna),
        &PathBuf::from(essay),
        &PathBuf::from(wubi),
        lm.as_deref().map(PathBuf::from).as_deref(),
        &PathBuf::from(out),
    )
    .unwrap_or_else(|e| {
        eprintln!("构建失败: {e}");
        std::process::exit(1);
    });
    println!("构建完成，耗时 {:?}", t0.elapsed());
}

/// 语料分词训练 bigram（输入需按空白分词、按行分句）。
fn cmd_lm(args: &[String]) {
    let (Some(dict_path), Some(corpus), Some(out)) = (
        flag_value(args, "--dict"),
        flag_value(args, "--corpus"),
        flag_value(args, "--out"),
    ) else {
        eprintln!("lm 需要 --dict/--corpus/--out 三个参数");
        std::process::exit(2);
    };
    let t0 = std::time::Instant::now();
    let dict_bytes = std::fs::read(&dict_path).unwrap_or_else(|e| {
        eprintln!("读取词典失败: {e}");
        std::process::exit(1);
    });
    let corpus_text = std::fs::read_to_string(&corpus).unwrap_or_else(|e| {
        eprintln!("读取语料失败: {e}");
        std::process::exit(1);
    });
    let lm = dict::build_lm(&dict_bytes, &corpus_text).unwrap_or_else(|e| {
        eprintln!("训练失败: {e}");
        std::process::exit(1);
    });
    std::fs::write(&out, lm).unwrap_or_else(|e| {
        eprintln!("写出失败: {e}");
        std::process::exit(1);
    });
    println!("bigram 表完成，耗时 {:?}", t0.elapsed());
}

fn cmd_info(args: &[String]) {
    let Some(path) = args.first() else {
        eprintln!("info 需要词典路径");
        std::process::exit(2);
    };
    let d = ime_core::dict::Dict::open(path).unwrap_or_else(|e| {
        eprintln!("打开失败: {e}");
        std::process::exit(1);
    });
    println!("词条数: {}", d.word_count());

    for (label, ns) in [
        ("ni", ime_core::format::NS_PINYIN),
        ("ni hao", ime_core::format::NS_PINYIN),
        ("xi an", ime_core::format::NS_PINYIN),
        ("khlk", ime_core::format::NS_WUBI),
    ] {
        let key = format!("{} {}", ns as char, label);
        match d.exact(&key) {
            Some(ids) if !ids.is_empty() => {
                let show: Vec<String> = ids
                    .iter()
                    .take(5)
                    .filter_map(|&id| d.word(id).map(|w| w.text.to_string()))
                    .collect();
                println!("{key:<12} -> {} 个候选: {}", ids.len(), show.join("、"));
            }
            _ => println!("{key:<12} -> <无>"),
        }
    }
}
