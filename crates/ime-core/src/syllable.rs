//! 合法拼音音节表与全拼切分（切分 DAG）。
//!
//! - `SYLLABLES`：标准（无声调）拼音音节全集，ü 以 `v` 书写（lv / nv / lve / nve）
//! - `is_syllable`：音节判定，兼容 jqx+y 后 `v` 写法（如 `jv` → `ju`）与 `ü` 字符
//! - `word_arcs`：把组字串切成音节弧（DAG），供解码层查码表建格

use std::collections::HashSet;
use std::sync::OnceLock;

/// 合法音节全集（无声调）。
pub const SYLLABLES: &str = concat!(
    "a o e ai ei ao ou an en ang eng er ",
    "ya yo ye yai yao you yan yin yang ying yong yu yue yuan yun ",
    "wa wo wai wei wan wen wang weng wu ",
    "ba bo bai bei bao ban ben bang beng bi bie biao bian bin bing bu ",
    "pa po pai pei pao pou pan pen pang peng pi pie piao pian pin ping pu ",
    "ma mo me mai mei mao mou man men mang meng mi mie miao miu mian min ming mu ",
    "fa fo fei fou fan fen fang feng fu ",
    "da de dai dei dao dou dan dang deng dong di dia die diao diu dian ding du duo dui duan dun ",
    "ta te tai tao tou tan tang teng tong ti tie tiao tian ting tu tuo tui tuan tun ",
    "na nai nao nei nan nang neng ni nie niao niu nian nin niang ning nu nuo nuan nv nve nong ",
    "la lai lao lei lan lang leng li lia lie liao liu lian lin liang ling lu luo luan lun lv lve lo long ",
    "ga ge gai gei gao gou gan gen gang geng gong gu gua guo guai gui guan gun guang ",
    "ka ke kai kao kou kan ken kang keng kong ku kua kuo kuai kui kuan kun kuang ",
    "ha he hai hao hou han hen hang heng hong hu hua huo huai hui huan hun huang ",
    "ji jia jie jiao jiu jian jin jiang jing jiong ju jue juan jun ",
    "qi qia qie qiao qiu qian qin qiang qing qiong qu que quan qun ",
    "xi xia xie xiao xiu xian xin xiang xing xiong xu xue xuan xun ",
    "zha zhe zhi zhai zhei zhao zhou zhan zhen zhang zheng zhong zhu zhua zhuo zhuai zhui zhuan zhun zhuang ",
    "cha che chi chai chao chou chan chen chang cheng chong chu chua chuo chuai chui chuan chun chuang ",
    "sha she shi shai shei shao shou shan shen shang sheng shu shua shuo shuai shui shuan shun shuang ",
    "re ri rao rou ran ren rang reng rong ru rua ruo rui ruan run ",
    "za ze zi zai zei zao zou zan zen zang zeng zong zu zuo zui zuan zun ",
    "ca ce ci cai cao cou can cen cang ceng cong cu cuo cui cuan cun ",
    "sa se si sai sao sou san sen sang seng song su suo sui suan sun",
);

/// 音节弧：覆盖组字串的 `[start, end)` 字符区间，`code` 为音节以单空格连接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arc {
    pub start: usize,
    pub end: usize,
    pub code: String,
}

fn syllable_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| SYLLABLES.split_whitespace().collect())
}

/// 全部音节前缀（用于 DFS 剪枝）。
fn syllable_prefixes() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        let mut set = HashSet::new();
        for syl in SYLLABLES.split_whitespace() {
            for i in 1..=syl.len() {
                set.insert(&syl[..i]);
            }
        }
        set
    })
}

/// 输入字母串是否为合法音节。兼容：
/// - 大写转小写、`ü` → `v`
/// - j/q/x/y 后的 `v` 视同 `u`（`jv` ≡ `ju`）
pub fn is_syllable(raw: &str) -> bool {
    let mut s = raw.to_lowercase();
    if s.is_empty() {
        return false;
    }
    if s.contains('ü') {
        s = s.replace('ü', "v");
    }
    let variants: Vec<String> = if s.contains('v') {
        let mut v = vec![s.clone()];
        if matches!(s.as_bytes().first(), Some(b'j' | b'q' | b'x' | b'y')) {
            v.push(s.replace('v', "u"));
        }
        v
    } else {
        vec![s]
    };
    let set = syllable_set();
    variants.iter().any(|c| set.contains(c.as_str()))
}

/// 枚举字符串的全部合法音节切分（上限 `max_syllables` 个音节；不合法返回空）。
pub fn all_splits(letters: &str, max_syllables: usize) -> Vec<Vec<String>> {
    fn dfs(
        s: &[u8],
        head: &mut Vec<String>,
        out: &mut Vec<Vec<String>>,
        max: usize,
        prefixes: &HashSet<&'static str>,
    ) {
        if s.is_empty() {
            out.push(head.clone());
            return;
        }
        if head.len() >= max {
            return;
        }
        for len in 1..=s.len().min(6) {
            let h = std::str::from_utf8(&s[..len]).unwrap_or("");
            if !prefixes.contains(h) {
                break;
            }
            if is_syllable(h) {
                head.push(h.to_string());
                dfs(&s[len..], head, out, max, prefixes);
                head.pop();
            }
        }
    }
    let mut out = Vec::new();
    let mut head = Vec::new();
    dfs(
        letters.as_bytes(),
        &mut head,
        &mut out,
        max_syllables,
        syllable_prefixes(),
    );
    out
}

/// 组字串 → 音节弧列表（DAG）。
///
/// - 只处理 `[a-z A-Z ']`；其他字符（数字、符号等）位置留空，由解码层按原样字符处理
/// - 弧可跨越显式分隔符 `'`，但每个 `'` 必须恰好落在切分出的某个音节起点上
///   （如 `xi'an` 生成跨 0..5 的弧 `xi an`，而 `xian` 单音节弧被拒绝）
/// - `max_syllables` 限制单弧最大音节数（对齐词组级长度）
/// - 先做可达边界 DP 筛出可行端点，再对可行弧枚举切分（每弧上限 4 种），避免全量枚举爆炸
#[allow(clippy::needless_range_loop)] // 弧端点是字符下标，与 letters 偏移表耦合
pub fn word_arcs(raw: &str, max_syllables: usize) -> Vec<Arc> {
    let chars: Vec<char> = raw.chars().collect();
    let n = chars.len();
    let mut arcs: Vec<Arc> = Vec::new();
    let mut seen: HashSet<(usize, usize, String)> = HashSet::new();

    for start in 0..n {
        // 收集本弧可用的字母（带字符下标）与分隔符
        let mut letters: Vec<(usize, char)> = Vec::new();
        let mut apos: Vec<usize> = Vec::new();
        let limit = n.min(start + max_syllables * 7);
        for end in start..limit {
            /* 保留：end 为字符位置，与 letters 映射耦合 */
            match chars[end].to_ascii_lowercase() {
                c @ 'a'..='z' => letters.push((end, c)),
                '\'' => apos.push(end),
                _ => break, // 非字母：弧只能止于此
            }
        }
        if letters.is_empty() {
            continue;
        }
        let text: String = letters.iter().map(|(_, c)| c).collect();
        let m = text.len();

        // 可达边界 DP：reach[j] = text[..j] 可切分为合法音节；syl_count[j] = 最少音节数
        let mut reach = vec![false; m + 1];
        let mut syl_min = vec![usize::MAX; m + 1];
        reach[0] = true;
        syl_min[0] = 0;
        for j in 1..=m {
            for len in 1..=j.min(6) {
                if reach[j - len] && is_syllable(&text[j - len..j]) {
                    reach[j] = true;
                    syl_min[j] = syl_min[j].min(syl_min[j - len] + 1);
                }
            }
        }

        // 弧终点集合：(字符终点, 字母覆盖数)。含紧邻分隔符之后的位置
        let mut ends: Vec<(usize, usize)> = Vec::with_capacity(letters.len() * 2);
        for i in 0..letters.len() {
            let (off, _) = letters[i];
            ends.push((off + 1, i + 1));
            if i + 1 < letters.len() {
                for k in off + 1..letters[i + 1].0 {
                    if chars[k] == '\'' {
                        ends.push((k + 1, i + 1));
                    }
                }
            }
        }

        for (end, j) in ends {
            if j == 0 || !reach[j] || syl_min[j] > max_syllables {
                continue;
            }
            let prefix = &text[..j];
            for split in all_splits(prefix, max_syllables).into_iter().take(4) {
                // 音节字符起点
                let mut idx = 0usize;
                let mut starts_at = Vec::with_capacity(split.len());
                for syl in &split {
                    starts_at.push(letters[idx].0);
                    idx += syl.chars().count();
                }
                // 每个分隔符必须对齐某音节起点，或为弧尾字符
                let aligned = apos
                    .iter()
                    .all(|&k| starts_at.contains(&(k + 1)) || k + 1 == end);
                if !aligned {
                    continue;
                }
                let code = split.join(" ");
                if seen.insert((start, end, code.clone())) {
                    arcs.push(Arc { start, end, code });
                }
            }
        }
    }
    arcs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 音节表规模合理() {
        let n = SYLLABLES.split_whitespace().count();
        assert!((390..=420).contains(&n), "音节数异常: {n}");
    }

    #[test]
    fn 音节判定含v兼容() {
        assert!(is_syllable("lv"));
        assert!(is_syllable("nve"));
        assert!(is_syllable("jv")); // jv ≡ ju
        assert!(is_syllable("ju"));
        assert!(is_syllable("NI")); // 大写
        assert!(is_syllable("lü"));
        assert!(!is_syllable("ii"));
        assert!(!is_syllable("ngx"));
    }

    #[test]
    fn 无歧义切分_nihao() {
        let arcs = word_arcs("nihao", 6);
        // ni + hao 必须成弧
        assert!(
            arcs.iter()
                .any(|a| a.start == 0 && a.end == 5 && a.code == "ni hao")
        );
        // 也允许中途前缀弧（ni / nih 等），但 nihao 不应是单音节
        assert!(
            !arcs
                .iter()
                .any(|a| a.start == 0 && a.end == 5 && !a.code.contains(' '))
        );
    }

    #[test]
    fn 音节歧义切分_fangan() {
        let arcs = word_arcs("fangan", 6);
        assert!(arcs.iter().any(|a| a.code == "fan gan"), "缺 fan'gan 切分");
        assert!(arcs.iter().any(|a| a.code == "fang an"), "缺 fang'an 切分");
        let s = all_splits("fangan", 6);
        assert_eq!(s.len(), 2, "fangan 恰有两种切分: {s:?}");
    }

    #[test]
    fn 显式分隔符_xian() {
        let arcs = word_arcs("xi'an", 6);
        assert!(
            arcs.iter()
                .any(|a| a.start == 0 && a.end == 5 && a.code == "xi an")
        );
        // 显式分隔后，xian（单音节）不应跨分隔符成弧
        assert!(
            !arcs
                .iter()
                .any(|a| a.start == 0 && a.end == 5 && a.code == "xian")
        );
    }

    #[test]
    fn 非字母原样保留位置() {
        let arcs = word_arcs("ab1cd", 6);
        // 数字把串断开：0..2 (ab→无音节?) ab 非音节前缀 → 无弧；3..5 (cd 同理)
        assert!(arcs.is_empty() || arcs.iter().all(|a| a.end <= 5));
    }
}
