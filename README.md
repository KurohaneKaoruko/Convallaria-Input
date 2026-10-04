# Convallaria Input（铃兰输入法）

轻量、干净、可扩展的输入法。Rust 核心引擎 + 平台原生前端，首期交付 Windows 中文输入（全拼 / 双拼 / 五笔 / 整句），为日文、移动端与内嵌小模型解码预留架构。

## 项目结构

```
crates/ime-core/      核心引擎（平台无关）：音节切分、码表、n-gram、Viterbi 整句解码、设置
platforms/windows/    Windows 前端：TSF 文本服务 + 候选窗（开发中）
tools/dict/           词典编译工具：Rime 文本词库 → 紧凑二进制 + n-gram 训练
assets/dicts/         开源词库原始文件（Rime luna_pinyin / essay / wubi86，LGPL-3.0，见 SOURCES.md）
assets/lm/            n-gram 语料说明
docs/benchmark.md     解码性能基准报告
openspec/             规格与变更管理（OpenSpec 工作流，不入库发布物）
```

## 构建

需要 Rust stable（建议 ≥ 1.93）。

```powershell
# 1) 编译词典（约 3 秒，产出 16.6MB 二进制，不入库）
cargo run -p dict --release -- build `
  --luna assets\dicts\luna_pinyin.dict.yaml `
  --essay assets\dicts\essay.txt `
  --wubi assets\dicts\wubi86.dict.yaml `
  --out assets\dicts\convallaria.dict.bin

# 2) 测试引擎
cargo test -p ime-core -p dict

# 3) 基准（可选）
cargo bench -p ime-core --bench decode
```

性能参考（RTX 平台 / Windows 11，详见 docs/benchmark.md）：词组级候选 ≤ 1ms（预算 30ms），18 音节整句 ≈ 98ms（预算 150ms）。

## 安装（Windows，开发中）

```powershell
# 管理员 PowerShell
platforms\windows\install.ps1     # 注册 TSF 文本服务
platforms\windows\uninstall.ps1   # 卸载
```

> 当前状态：MVP 骨架（可注册、可吃键、回车上屏原文）。组字串显示与候选窗交互在后续任务交付。

## 使用

安装后通过系统语言栏选择「Convallaria Input」：

- 输入模式：全拼 / 双拼（小鹤、自然码、微软）/ 五笔 86
- 整句输入：全拼模式下连续键入整句拼音，空格上屏首选
- 候选：数字键 1-9 选词，`-`/`=` 翻页；回车上屏原文；Esc 清空
- Shift 切换中英文；Ctrl+` 循环切换输入模式
- 配置文件：`%APPDATA%\Convallaria\config.toml`，修改后即时生效

## 已知局限

- Windows 前端候选窗运行在宿主进程内：UWP / 沉浸式应用 / 全屏独占的兼容性不保证（进程外 UI 为后续演进）
- 双拼词组的拼音码按「各字最高频读音」推断，多音字词存在个别误码（已知取舍）
- bigram 语料（zh Wikipedia 分词统计）尚未接入，整句解码暂用 unigram + 词长先验，质量可继续提升
- MVP 未做：日文输入、macOS / Linux / Android / iOS、内嵌小模型（架构已预留 `LanguageModel` 接口）

## 路线图

1. Windows 中文 MVP（本期）：TSF 接入、组字串、候选窗、三种输入模式、整句
2. 日文输入（罗马字 → 假名 → 汉字，复用同一解码管线）
3. Android / iOS 前端
4. 内嵌小模型解码器（ONNX，替换 `LanguageModel` 实现）
