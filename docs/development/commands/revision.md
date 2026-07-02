# revision 命令开发设计

## 命令实现目标

lore.md §1.16：可重建的 SQLite 侧表映射 commit OID↔单调递增序号，承载 Lore
`revision find number`（正查）与反查；`find --metadata` 显式延后（见下）。

## 对比 Git 与兼容性

- 兼容级别：`intentionally-different`。最近类比 `git rev-list --first-parent
  --count`（反向）与 `<tip>~<k>`（正向）。退出码 0 / 1（未命中）/ 128 / 129。

## 设计方案

- **序号语义**：按 ref 的 FIRST-PARENT 链，1=root、N=tip。决定性：编号是
  tip OID 的纯函数（每提交恰有一个 first parent，无并列需破），重建在任何
  机器任何时刻复现同一 `(ref, ordinal, oid)` 投影（测试钉住——比较投影而非
  全表 dump：AUTOINCREMENT id 经 DELETE 后不复位）。仅经非首父可达的提交
  **无序号**（反查明说，绝不编造）。逐 ref 键控：切分支互不干扰、懒构建。
- **新鲜度（1.1 never-lie）**：指纹 = tip OID + **refs/replace 集摘要**
  （审阅 must-fix：replace 变更改变 load_object 解析的有效链而不动 tip——
  loose 文件 `.libra/refs/replace/` 排序 name=target 哈希）。每次读在**与
  查询同一事务**内 ensure_fresh：tip 前进且旧 tip 在新链上 → APPEND 后缀
  （既有序号不变——Lore 单调性）；其它任何不匹配（重写/回退到祖先/replace
  变更）→ 全量重建。并发读者绝不见半编号。
- **迁移**：2026070301（幂等 + down），注册表 16 项——`builtin_migrations` +
  migration.rs 单测 + tests/db_migration_test.rs **四处**版本清单（versions/
  applied/rolled 前插/reapplied 追加，审阅 must-fix：此前漏过三处）。
- **属主 API**：`internal::revision_ordinal::RevisionOrdinalIndex`
  （`_with_conn` 全套；插入 500 行分块防 SQLite 绑定上限）。prune 的真实
  触发器：`revision index --rebuild` 先于事务列活分支（池死锁教训：事务内
  再开池连接会超时），事务内清扫消失 ref 的行+meta。
- **CLI**：`Commands::Revision`（find/number/index），默认当前分支，
  detached 无 `--ref` → 128/LBR-REPO-003 语义；`--json` 结构化。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| find --metadata | 按 1.10 revision 元数据检索 | 序号索引提供确定性迭代序；查询语义未决（存在性 vs 等值、typed 比较、trailer/notes 命中优先级）且每查询需逐提交读 notes——独立设计轮。草图：按序走索引链，对每提交 `MetadataKv::revision_get`。 |
| 首查成本 | 长分支首查 O(chain) 对象读 | 文档化；后续可做后台预热（1.11 服务可承载）。 |
| 非首父覆盖 | 全 DAG 编号 | 可行方案存在（merge 点确定性子序），但偏离 Lore 线性模型；如需求出现再议。 |

## 实现历史

- 2026-07-03（lore.md Phase 1 / 1.16）：初版全套 + 3 e2e + 迁移注册。

## 维护要求

- 改进前先读 [_general.md](_general.md)。两表读写必须经属主 API；任何新
  链遍历必须走 `load_object`（replace/分层存储感知）并保持指纹契约。
