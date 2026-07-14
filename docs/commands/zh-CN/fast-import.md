# `libra fast-import`

把 Git fast-import 流导入 Libra 仓库，是 [`fast-export`](fast-export.md) 的自然反向。

## 用法

```text
libra fast-import [--input <file>] [--max-count <n>] [--quiet]
```

## 说明

支持的指令包括：

- 带 mark 与定长/分隔符 `data` 的 `blob`；
- `commit <ref>` 的 `mark`、`author`、`committer`、消息 `data`、`from`、`merge`、`M`、`D`、`C`、`R`、`N` 与 `deleteall`；
- `M ... inline` 与 `N inline` 的内联 blob；
- annotated `tag`（含可选 mark，以及 commit/tree/blob/tag 目标）；
- 带 `from` 的 `reset <ref>`；省略 `from` 时删除 ref；
- `checkpoint`、`done`，以及宽松接受的 `feature`/`option`/`progress` 前导。

路径支持 Git C 风格 quoting；绝对、空、遍历路径或 Libra tree 模型无法表示的非 UTF-8 路径会被拒绝。commit/tag 消息也必须是 UTF-8，无法表示时会失败而不是有损改写。`M` 会校验文件 mode 与对象类型，避免写出损坏的 tree entry。

分支和 tag 写入正常 ref 存储。`refs/notes/*` commit 无论使用 `N` 记录还是 Git 的 notes-tree 形式，都会翻译为 Libra notes 表。其他 ref 命名空间 fail-closed。

### 事务模型

对象在解析时写入；branch、tag 与 notes 变更先缓冲，并在 `checkpoint`、`done` 或干净 EOF 时由一个 SQLite 事务统一发布。发布前失败不会改变 refs/notes；已写对象不可达，可在 `libra fsck` 后用 `libra gc` 回收。`checkpoint` 会有意让此前批次持久化。

### 安全与资源上限

- 输入总量默认 1 GiB，由 `fastimport.maxInputSize` 控制；配置不可读、非法或为零时 fail-closed。
- command/header 单行上限 1 MiB，避免继续无界分配。
- 顶层 blob、commit、tag 默认最多 1,000,000 个；`--max-count` 可调整。派生 tree 不单独计数。
- 字面对象 ID 必须匹配仓库 SHA-1/SHA-256 格式；mark 必须唯一；被引用对象必须存在且类型正确。

## 选项

| 选项 | 说明 |
|---|---|
| `--input <file>` | 从文件而非 stdin 读取。 |
| `--max-count <n>` | 设置顶层导入对象数量上限。 |
| `--quiet` | 抑制末尾人读汇总。 |

即使设置全局 JSON/machine flag，导入协议仍为原始格式；程序消费 stdout 时使用 `--quiet`。

## 退出码

| 退出码 | 含义 |
|---|---|
| `0` | 流完成，最后一批变更已发布。 |
| `128` | 输入损坏/不支持、配置/ref/path/object 非法、资源超限，或仓库/事务/IO 失败。 |

## 示例

```bash
libra fast-export --all | libra fast-import --quiet
libra fast-import < repository.fi
libra fast-import --input repository.fi --max-count 2000000
```

## 与 Git 对比

| 任务 | Libra | Git |
|---|---|---|
| 导入流 | `libra fast-import` | `git fast-import` |

仍延后的协议命令包括 `cat-blob`、`ls`、`get-mark`；marks 文件导入/导出与真正流式处理多 GiB 对象 payload 也尚未实现。
