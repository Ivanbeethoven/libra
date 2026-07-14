# fast-export 命令开发设计

## 命令实现目标

`libra fast-export [--all] [REV...]` 以 Git fast-import 协议导出多个 ref、范围、tag 与 notes；命令只读。P1-11 的成功标准是常见离线迁移保真和双向真实 Git 互操作，不是完整复刻 `git fast-export` 的所有过滤器。

## 对比 Git 与兼容性

- 兼容级别：`partial`。
- 已支持：默认 `HEAD`、multi-revision、`A..B`、`^A`、`--all`、local branches、lightweight/annotated commit tags、Libra notes→`N`、Git C-style UTF-8 path quoting、共享 marks、`done`。
- 有意实现形态：每个 commit 用 `deleteall` + 全树 `M`，比 parent diff 大但 tree 等价。
- 签名边界：commit signing header 在 fast-import commit record 中不可表达，导出时剥离；tag message 原样保留。
- fail-closed：坏 ref/object/tag row、缺对象、note target 无法由 stream mark 表示、stdout 写失败均返回 `CliError`，不宣称成功。

## 设计方案

- 入口：`src/cli.rs::Commands::FastExport` → `command::fast_export::execute_safe`。
- `build_export_plan` 归一化 branches/tags/positive specs/exclusions；所有 ref 共享一份 `HashMap<ObjectHash, mark>`。
- `collect_commit_ids` 先计算 exclusions，再收集闭包；`topological_order` 用迭代后序 DFS 保证 parent mark 先定义。范围外 parent 使用 literal OID，形成 incremental prerequisite。
- `flatten_tree` 递归生成稳定路径序；blob 去重后发出，gitlink 使用 literal commit OID。
- annotated tag 以 `tag` record + mark 发出，使 note 可以引用 tag object；`emit_notes` 按 notes ref 分组并写 root notes commit/N records。
- `quote_path` 按字节输出 Git C escapes；Libra tree 名是 UTF-8，非 UTF-8 Git path 仍为明确边界。
- 输出使用 `BufWriter<StdoutLock>`；不写 SQLite、对象库、index/ref/worktree；全局 JSON/machine 不包协议流。

## 实现历史

- 2026-06-30（GGT-13）：首版单 revision、blob/commit、整树 M。
- 2026-07-14（P1-11）：multi-ref/range、annotated tag、notes、quoting、真实 Git 双向 round-trip、SHA-256 smoke 与 fail-closed note closure。

## 当前状态

- 公开：`Commands::FastExport`。
- 核心证据：`compat_import_export_roundtrip::libra_all_export_round_trips_refs_tags_notes_ranges_and_quoted_paths`、`fast_streams_interoperate_bidirectionally_with_real_git`、SHA-256 case；unit tests pin path quoting/range/message boundary。
- 用户文档：`docs/commands/fast-export.md` 与 `docs/commands/zh-CN/fast-export.md`。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| revision | `A...B` | 不承诺；使用显式正/负 revisions。 |
| marks/filter | marks files、`--anonymize`、path/blob filtering | 延后。 |
| tag | 最终目标非 commit 的 annotated tag | fail-closed `LBR-UNSUPPORTED-001`。 |
| stream size | parent-diff 压缩 | 继续使用正确但更大的全树形式。 |

## 维护要求

任何 emitter 改动都必须同时通过 Libra→Libra、Libra→system Git、system Git→Libra 与 SHA-256 测试；严禁重新引入 silent note dropping 或未定义 mark。
