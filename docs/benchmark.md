# 解码性能基准报告（任务 3.9）

- 日期：2026-10-04
- 环境：Windows 11，rustc 1.93.1，`cargo bench --quick`（criterion 0.5）
- 词典：真实词库 `assets/dicts/convallaria.dict.bin`（487,654 词条，16.6MB，mmap 加载）
- 代码：`crates/ime-core/benches/decode.rs`

## 结果 vs 规格预算（sentence-decoding「解码响应时间」）

| 场景 | 输入 | 实测（中位） | 预算 | 结论 |
|------|------|--------------|------|------|
| 词组级候选首屏 | `ni` | 41 µs | 30 ms | ✅（≈1/700） |
| 词组级候选首屏 | `nih`（尾音节前缀） | 111 µs | 30 ms | ✅ |
| 词组级候选首屏 | `niha` | 72 µs | 30 ms | ✅ |
| 词组级候选首屏 | `nihao` | 216 µs | 30 ms | ✅ |
| 词组级候选首屏 | `zhongguorenmin`（5 音节） | 722 µs | 30 ms | ✅ |
| 整句解码 | 18 音节 `jintiandetianqizhenhaowomenyaochumen` | 98 ms | 150 ms | ✅ |

## 关键优化记录

初版实现 18 音节整句约 314–495 ms，超预算。两处修改后达标：

1. `syllable.rs::word_arcs`：先做可达边界 DP（`reach` / `syl_min`）筛出可行弧端点，再仅对可行弧枚举切分（每弧 ≤4 种），替代原「对每个 (start, end) 全量枚举切分」。
2. `viterbi.rs`：新增 `STATE_CAP = 64`——每个位置按最优分做全局状态裁剪，防止状态数随输入长度线性膨胀导致转移数爆炸。

## 说明

- 基准在 `--quick`（criterion 缩减采样）下测得；正式发布前建议完整跑一轮 `cargo bench` 并归档 trend 数据。
- 候选收集上限 `cap × 4` 与 `STATE_CAP` 是耗时/质量折中点，如后续引入 bigram 或小模型可适当放宽重测。
