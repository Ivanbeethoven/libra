# fast-import 命令开发设计

## 命令实现目标

`libra fast-import` 以资源有界、ref/note 原子发布的方式消费常见 Git fast-import 流，并翻译 Git notes tree 与 Libra notes rows。

## 对比 Git 与兼容性

- 兼容级别：`partial`。
- 指令：blob；commit 的 mark/author/committer/data/from/merge/M/D/C/R/N/deleteall；M/N inline；annotated tag；reset update/delete；checkpoint/done；feature/option/progress。
- refs：`refs/heads/*`、`refs/tags/*`、`refs/notes/*` 持久化；其他 namespace fail-closed。
- path/metadata：Git C-style quoted UTF-8；absolute/traversal/empty/non-UTF-8 path 与非 UTF-8 commit/tag message 拒绝，不做 lossy conversion。
- 未实现：cat-blob/ls/get-mark、marks files、多 GiB payload 真正流式处理。

## 设计方案

- 入口：`execute_safe` require_repo，读取 stdin 或 `--input`；`configured_max_input` 对 unreadable/invalid/zero config fail-closed。
- `Importer<R: BufRead>` 维护 marks、pending refs、note delta/replacement、1-line pushback。命令行 1 MiB cap；总输入默认 1 GiB；top-level save 默认 10^6 cap。
- counted `data` 精确读取并消费可选 LF；here-doc 逐行读取。blob/tag/commit 经 shared `save_object`；tree 经 `tree_plumbing::write_tree_from_leaves`。
- commit state 从 first parent flatten；M/D/C/R 处理 file/subtree replacement。M 强制 mode↔object type（blob/executable/link→blob，gitlink→commit；040000 目录 M 拒绝）。
- `N` 直接转 pending note delta；Git tree-shaped notes commit 把 fanout path 拼回 hash，并形成 notes-ref replacement snapshot。root N commit 与 reset-without-from 同样 replacement/delete。
- branch/tag/note 变更只在 `flush_refs` 中执行；同一个 SeaORM transaction 内 upsert/delete refs，先清 replacement notes ref，再应用 note rows。unsupported namespace/DB error 全部 rollback。checkpoint 是明确 durability boundary；对象写入失败后的孤儿由 fsck/gc 处理。
- reset branch target必须为 commit；tag target 必须为可读对象；merge parent、M object、note blob/target 均验证后才发布。
- raw protocol 不受 JSON wrapper 影响；`--quiet` 关闭 summary。

## 实现历史

- 2026-06-30（GGT-13）：blob/commit/reset/M/D/deleteall 与缓冲 branch refs。
- 2026-07-14（P1-11）：quoted/inline/C/R/tag/N、Git notes tree translation、atomic branch+tag+note publish、reset deletion、type safety、line/config bounds、真实 Git 与 SHA-256 互操作。

## 当前状态

- 公开：`Commands::FastImport`。
- 测试：`compat_import_export_roundtrip` 覆盖 manual directive、transaction rollback、reset delete、bad mode/object、invalid config、Libra/Git 双向与 SHA-256；module unit tests覆盖 parser/path/line bound。
- 用户文档：EN/zh-CN `docs/commands/fast-import.md`。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| query commands | cat-blob/ls/get-mark | 拒绝 128。 |
| marks | import/export marks files | 延后。 |
| byte model | non-UTF-8 Git paths | Libra String tree model无法表示，fail-closed。 |
| scale | multi-GiB single payload streaming | 1 GiB total cap；对象 data 仍单对象缓冲。 |

## 维护要求

不得把 ref publication 移出 transaction；新增 namespace 必须同时定义 delete/upsert/rollback、recovery、docs 与 error behavior。任何 tree operation 都必须保持 mode/type 校验。
