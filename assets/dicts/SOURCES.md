# 词典数据来源与许可说明

本目录的原始词库文件均来自 [Rime 输入法引擎](https://rime.im) 的开源数据仓库，许可均为 **LGPL-3.0**（各文件头部附 LICENSE 摘录原文）。

| 文件 | 来源仓库 | 内容 | 用途 |
|------|----------|------|------|
| `luna_pinyin.dict.yaml` | [rime/rime-luna-pinyin](https://github.com/rime/rime-luna-pinyin) | 单字拼音表（字 / 拼音 / 频度占比） | 单字码表、多音字推断、字频 |
| `essay.txt` | [rime/rime-essay](https://github.com/rime/rime-essay) | 词库（词 / 词频，约 44 万行） | 多字词词表与词频 |
| `wubi86.dict.yaml` | [rime/rime-wubi](https://github.com/rime/rime-wubi) | 五笔 86 码表（字 / 词，含显式词组码与码频） | 五笔模式码表 |
| `t2s.txt` | [BYVoid/OpenCC](https://github.com/BYVoid/OpenCC) TSCharacters.txt | 繁→简单字映射（Apache-2.0） | essay 繁体词库构建期转简体 |

`*.LICENSE` 为对应来源仓库 LICENSE 的原文拷贝。

`*.bin` 为 `tools/dict` 生成的紧凑二进制词典（构建产物，不入库，见仓库根 `.gitignore`）。

## 再分发说明

- 上述原始数据文件按 LGPL-3.0 随本仓库再分发，保留来源与许可声明即满足其要求。
- 本项目代码自身的许可证待定（见 README），引入新的第三方语料前必须先核对其许可条款并更新本文件。
