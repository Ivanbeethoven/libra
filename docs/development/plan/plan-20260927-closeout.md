# plan-20260927 收尾临时计划（2026-09-29）

> **性质：** 临时/跟踪计划。核心六命令模块拆分已实现、全量 nextest 8274/8274 绿、`v0.30.8` 已跨平台成功发布。本计划只列出**剩余 gap 的具体修复动作**，逐项可执行、可验收；不 bump 版本、不发新 release，仅提交并推送本计划及相关修复。
>
> **当前基线：** `main` HEAD `beab6b7`（v0.30.8，release 已发布）；工作树干净。
>
> **不 bump 代码：** 所有变更只到 `libra commit` + `libra push origin main`，不改 `Cargo.toml`/`install.sh`/`install.ps1`/`Cargo.lock` 的版本。

---

## 背景与目标

| 剩余项 | 卡点本质 | 目标（checklist 验收） |
|---|---|---|
| G-1 `fsck_heal_restores_object_from_durable_tier` live 测试（已修，21/21） | 测试未先 `cloud sync`，blob 未进 R2 → 已修复 | live `--features test-live-cloud` 21/21 已确认 |
| G-2 FIX-CM-WT-MOVE（EXDEV move） | 旧二进制 patch 兼容性**决策** | 给出补丁兼容源证 ≤ scope；否则呈 breaking/minor 方案 |
| G-3 4 个 Cloud FIX 卡（LIVE-GATE 接线 + RECOVERY-AUTH/CLEANUP/REPO-SCOPE/LIVE-SAFETY） | CI/环境/受保护协议工程 | `cloud_live_prepare.sh`/`cloud_live_resources.rs` 等基建落地 + live 受保护 dispatch 通过 |
| G-4 22 项 EX 批准 + 计划级 Claude `VERDICT: PASS` | **外部评审流程** | 字面 PASS + 具名 reviewer 逐条 EX 同意 |

---

## G-1：修复 `fsck_heal_restores_object_from_durable_tier`

**现象：** 真实 live 运行中 `heal.unrecoverable != 0`，`src/command/fsck.rs:897` 报 `unrecoverable: ‹hash› not available in durable tier`。

**根因假设（逐一验证，避免猜测）：**
1. `object_index` 登记不全——某个可达对象写入本地后未被 `client_storage.rs` 的 `insert_object_index_row`/`upsert_object_index_*` 记录，sync 因此不上传。
2. `is_synced` 标 1 但实际未上传（同步提交时序竞态）。
3. fsck 的 `collect_heal_candidates`（`fsck.rs:802` 起）通过 refs/reflogs/index 发现的对象超出 sync 上传范围（例如只被 `extra_roots` 或 reflog 引用、sync 未覆盖）。

**具体动作（按序）：**
- [x] **复现与取证**：诊断 live run 捕获 `unrecoverable: 539399e63d9f31286e022b931cc2ab29f8107cdb`（blob "durable heal\n"）；测试未先 `cloud sync`，blob 未进 R2。
- [ ] **对照存储**：对同 repo 列举 R2 前缀下 key；查本地 `object_index` 行，确认该 OID 是否有行、`is_synced` 值。
- [x] **定位漏点**：
  - 若 `object_index` 无该行 → 修写入路径（`client_storage.rs` 写对象后确保入对象索引；`db.rs:1200/1261` 的登记点）。
  - 若有行但 `is_synced=1` 却不在 R2 → 修 sync 的 `exist_batch`/上传时序（`sync.rs:157` 起批量上传），保证「标已同步」仅在确实上传成功后发生。
  - 若 fsck 发现对象不被 sync 覆盖 → 让 sync 也上传 reflog/`extra_roots` 可达对象，或调整 fsck 候选集。
- [ ] **加回归**：新增一个测试，构造「离库对象在 R2 存在」场景，断言 `--heal` 后 `unrecoverable==0` 且全部愈合；若本地可跑则并入 `cloud_storage_backup_test`，否则只在 live 门（`cloud_live_no_skip.sh`）下运行。
- [x] **验证**：dispatch `main`（含 `cloud sync` 修复）live runs，`compat-live-cloud` **21/21** 绿（`fsck_heal`、`cloud_sync_name_conflict` 均 ok）。

**G-1 收尾判定（2026-09-29 逐项复核，未执行者不勾）：**

- **定位漏点 → 已勾选。** 根因不是产品侧漏写/漏传，而是**测试操作次序**：`fsck_heal_restores_object_from_durable_tier` 在该 run 里从未先 `cloud sync`，blob `539399e63d9f31286e022b931cc2ab29f8107cdb`（"durable heal\n"）从未进入 R2，而测试已在删除本地对象后断言 durable tier。上面列出的三种代码假设（`object_index` 漏登记 / `is_synced` 早标 / fsck 候选集超出 sync 上传范围）均**未被证据支持**，无需按分支修写入路径或上传时序。修复见 `5296b9f`（删除本地对象前先 `cloud sync`）。证据：live run `36602151296` 日志 `test result: ok. 21 passed; 0 failed; … finished in 539.84s`，对比其后之前的连续 4 个 failure run。
- **对照存储 → 保持未勾，且判定为已失效。** 该项要求对该 repo 列举 R2 前缀 key 并核对本地 `object_index` 行的 `is_synced` 值，是**真 R2 操作**，本地不可执行；且当根因定为测试次序后，该取证对结论不再是必要条件。此为文档化豁免，不得当成产品缺口。
- **加回归 → 保持未勾，但已确认「场景与断言均已存在」。** `5296b9f` 只是给**既有** live 用例补了 1 行 `cloud sync`（`tests/cloud_storage_backup_test.rs:1839` 附近），**未新增**计划字面要求的「新增一个测试」。但计划描述的验收内容在该既有 live 用例里**逐条已具备**（`fsck_heal_restores_object_from_durable_tier`，live-gated）：
  - 恰为「离库对象在 R2 存在」场景：`:1840` 先 `cloud sync` 上传全图 → `:1843` 删除**全部**本地 loose 对象 → 对象只存于 R2。
  - 断言 `unrecoverable == 0`：`:1877`–`:1881`「every object is present in the durable tier, so nothing is unrecoverable」。
  - 断言「全部愈合」：`:1870` `heal.healed >= 2`（固定点多轮）、`:1886` commit 已回写本地、`:1891` `--heal` 退出 0。
  - 归属 live 门：该 target 正是 `live-compat.yml` 与 `cloud_live_no_skip.sh` 选中的 `cloud_storage_backup_test`，符合计划「否则只在 live 门（`cloud_live_no_skip.sh`）下运行」。
  **为何不宜新增：** 若根因定为测试次序（见上「定位漏点」），则**不存在产品侧缺陷可供回归**——可回归的只有测试自身的步骤，而该步骤已被 `5296b9f` 修正并跑在 live 门上；再加一个同场景用例只会重复一个 539s 量级的 live 门耗时，且本地无法验证。故此项是**字面「新增」未满足**，而非覆盖缺失，需 owner 判定为「以既有 live 用例满足并勾选」或「删除该条」，不宜由我单方面勾选。

因此 G-1 不再是产品/行为缺口，但**仍未全部勾选**，不构成可宣称整计划 complete 的依据。

**关键文件：** `src/utils/client_storage.rs`、`src/internal/db.rs`、`src/command/cloud/sync.rs`、`src/command/fsck.rs`、`tests/cloud_storage_backup_test.rs`、`.github/workflows/live-compat.yml`。

---

## G-2：FIX-CM-WT-MOVE（EXDEV 跨设备 move）

**现状：** `src/command/worktree/operations.rs::move_worktree` 已有 `fs::rename` + `or_else` 复制回退，及 `journal_*` 持久 intent。**实现仅部分在**：EXDEV 回退仍是 `fs_extra::dir::copy` → `fs::rename` → `fs::remove_dir_all(src)`，失败时回滚 `fs::remove_dir_all(dest)`；计划要求的完整树复核、三态路径探测、原子 no-replace 发布、owned 隔离与故障注入均未实现。**卡点因此包含实现缺口与补丁兼容性决策两层，不只是「补证」。**

**具体动作：**
- [ ] **补丁兼容源证**（在脚本/测试内可执行的证明）：
  - [ ] 仅 pending EXDEV 的持久 fence：新 `worktree move` 写下的 intent，旧 `worktree prune/remove`（不检查 pending move）不能无视它作删除。
  - [ ] 旧 repair 不能动坏 v2 状态：NUL 哨兵/保留字段确保旧 `repair` 读旧格式、不覆盖新字段。
  - [ ] `down` 与 `v1→v2` 转换同事务串行，且旧进程已退出才允许升级（`migrate_layout`/registry 版本面）。
  - [ ] 用旧二进制与 `worktree prune --dry-run`/`repair` 各做一次跨编译回归（不 bump，只用当前树生成的东西验证语义）。
- [ ] **若任一证不了**：记录 **breaking/minor 方案**（含独立兼容窗口与 ER-08 版本约束），呈给用户/维护者取决定；**不预设 minor 已授权**。
- [ ] 状态更新：`DEP-CM-WT-COMPAT` 由 `blocked` → `passed`（具名 owner 确认）后，FIX-CM-WT-MOVE、CM-05、CM-13 才正式验收。

### G-2 源证判定结论（2026-09-29 源码复核 @ `bdb7727`，呈请用户/维护者决定）

**结论：4 项补丁兼容源证在当前源码上均不成立，进入 G-2 自身的 breaking/minor fallback；未预设 minor 已授权，`DEP-CM-WT-COMPAT` 维持 `blocked`。上面的勾选项按本结论保持未勾。**

| 源证项 | 判定 | 证据锚点 |
|---|---|---|
| 旧 `prune`/`remove` 不能无视 pending move fence | ❌ 不成立 | `journal_pending` 调用点仅 `doctor.rs:1573,2313,3348` 与 `operations.rs:1504`（move 自身的迁移守卫）；`prune_worktrees`（`operations.rs:1604`）与 `remove_worktree`（`operations.rs:1786`）只读 registry，不查 pending intent |
| 旧 `repair` 不能改坏 v2 状态（NUL 哨兵/保留字段） | ❌ 未实现 | `registry.rs:126` `REGISTRY_SCHEMA_VERSION = 3`；`parse_document` 仅对未知 `schema_version` fail-closed，源码无 NUL 哨兵/保留字段方案 |
| `down` 与 `v1→v2` 转换同事务串行、旧进程已退出 | ❌ 未实现 | `move_worktree` 无 down/转换串行门；`src/internal/db.rs:744` 的 `migration::run_builtin_migrations` 在普通连接上自动应用全部注册 migration（模块文档 `db.rs:12`），`db.rs:117` 明示不可 down |
| 旧二进制 × `prune --dry-run`/`repair` 跨编译回归 | ❌ 不存在 | `tests/command/worktree_test.rs` 仅 `test_worktree_move_cross_device_error_is_portable`（`:65`）与 `test_worktree_move_across_filesystems_rolls_back_when_supported`（`:1312`） |

**实现缺口（超出「补证」范围）：** `operations.rs:1541-1568` 的 EXDEV 回退无内容复核；`copied_path != dest_path` 时用普通 `fs::rename` 发布（非原子 no-replace）；`remove_dir_all(src)` 失败即删除 `dest` —— 正是 `plan-20260927.md` 风险登记表标记的 P1「删除唯一完整副本」。仓库内唯一的 no-replace 原子发布点在 `doctor.rs:1649`（repair 备份），与 move 无关。

**ER-08 判定：** patch 交付要求「源码 + 旧二进制实证」双证（`plan-20260927.md:185,975`），当前**不满足**；minor 须用户重新定范围（`plan-20260927.md:986`：本卡无 minor 授权）。

**待用户/维护者决定（二选一）：**
- **方案 A（patch）**：先按 `plan-20260927.md:850` 候选设计实现 `move_exdev` + 临时 future-schema capability receipt（新连接与预打开旧 mutator/repair 均被 fence、up/down 与 `v1→v2` 原子排竞、SIGKILL 窗口不泄漏旧 writer、收敛后 down 不被自动 up 重施），再按 `plan-20260927.md:975` 的完成定义取证。
- **方案 B（breaking/minor）**：记录独立兼容窗口与 ER-08 版本约束，由用户另行授权版本范围并修订计划。

决定前，FIX-CM-WT-MOVE 与 CM-05/CM-13 保持 `blocked`，其它无写集冲突卡继续。

**关键文件：** `src/command/worktree/operations.rs`（move_worktree/journal_*）、`src/command/worktree/registry.rs`（版本面）、`docs/development/plan/plan-status.md`。

---

## G-3：4 个 Cloud FIX 卡基建

**一般说明：** 这些是 CI/环境/受保护协议工作。用户侧需先配置受保护 `cloud-live-write` environment、七项 secret 迁离 repo 级、四项 vars；本地只做 fake/mock/default C。

### G-3a FIX-CM-LIVE-GATE（接线）
- [ ] `tests/cloud_live_no_skip.sh`（已存在，`--self-test` 全绿）接入 `live-compat.yml` 的 `Run live cloud tests` 步骤，替换旧 `skip=true` 分支。
- [ ] 真实 run 用 JSON list 钉 selected count、no-skip 核 run/pass、保留原始日志。

### G-3b FIX-CM-CLOUD-RECOVERY-AUTH
- [ ] 新建 `cloud-live-recover.yml`（GC-CM-14：AUTH 期仅 `probe-auth`、CLEANUP 后仅 `cloud-live-recovery/v1`）。
- [x] 实现 `libra-cloud-live-manifest-v1` / `libra-cloud-live-grant-v1` 校验 helper（`tests/helpers/cloud_live_manifest.rs`：顶层 schema/字段集合、时间戳 UTC 无小数、writer_slots repo_id 升序无重复且 `r2_prefix` 恰为 `<repo_id>/`、资源四元组、grant run/attempt/ref/SHA/nonce 匹配即零删除）；单元测试 4 项通过。零删除探针仍在 `cloud-live-recover.yml` 侧接线。

### G-3c FIX-CM-CLOUD-RECOVERY-CLEANUP
- [x] 实现限界幂等清理 helper（`authorized_cleanup_scope`：由 manifest writer_slots + restore_target_slots 计算 D1 repo_id/R2 prefix 集合，未登记 source_repo_id 即 fail-closed；`reject_unregistered_sink_write`：D1/R2 sink 拒绝未登记 repo/越界前缀）。`cloud-live-recover.yml` 的接入与 receipt 保留仍待接线。

### G-3d FIX-CM-CLOUD-REPO-SCOPE
- [x] 实现 repo/slot 身份 sink 守卫（`reject_unregistered_sink_write`、`assert_registered_repo`、`r2_key_in_registered_scope`），本地 fake mock 下证明未登记 repo_id/前缀被拒；真实 CLI sink 的接线仍待 fake endpoint 桩。

### G-3e FIX-CM-CLOUD-LIVE-SAFETY
- [x] 新建 `tests/cloud_live_prepare.sh`（Nextest 前生成写者槽位 + 多仓 `test-repo-<uuid>` repo ID；已验证幂等可运行）。
- [x] 新建 `tests/helpers/cloud_live_resources.rs`（写者槽位读取、D1/R2 身份探针、repo 作用域校验、全局 manifest；`cloud_live_resources_test` 3/3 绿）。
- [ ] CI YAML 增加 D1 全库 SQL(encrypted)/bookmark/全局前像/每例清单的 artifact v4 上传/下载/校验，全部成功后才启动两个完整 live target。
- [ ] 真实 `workflow_dispatch` 通过 Safety 写前门，产出 `E-CM-L3-SAFETY`；CM-10/11 各自产出 `E-CM-L3-10/11`。

**关键文件：** `tests/cloud_live_no_skip.sh`、`tests/cloud_live_prepare.sh`、`tests/helpers/cloud_live_resources.rs`、`.github/workflows/live-compat.yml`、`.github/workflows/cloud-live-recover.yml`（新增）。

**G-3e 缺陷修正（2026-09-29 复核 @ `e9c46b1`）：**

- **现象：** `cloud_live_resources_test` 在 `Cargo.toml:254` 注册时**没有 `required-features`**，因此属于默认 L1 套件；但它里面的 `tests::cloud_live_resources_helper_compiles` 直接调用 `resource_identity_probe`，而该函数在身份缺失时 `assert!` 硬 panic（`tests/helpers/cloud_live_resources.rs:86`）。在**未导出** `LIBRA_D1_ACCOUNT_ID` 的 shell 里实测 `7 passed; 1 failed`。已排除仓库级注入：`.cargo/config.toml` 的 `[env]` 只设 `RUST_MIN_STACK`。
- **影响：** `AGENTS.md:99`（「`cargo test --all` runs the default L1 suite (the acceptance gate)」）与 `AGENTS.md:129`（PR 自检清单）都把 `cargo test --all` 当验收门，且 AGENTS.md 全文**不含** `.env.test` 字样；因此干净 checkout 上按文档跑验收门会直接失败。CI 因 `compat-offline-core` 注入 L2/L3 secrets 才不复现，属于「本地不可复现的假绿」风险。
- **附带的空断言：** 原测试体是 `assert!(!x.is_empty() || x.is_empty())`（恒真），不验证任何东西，与同文件头「runs its unit tests in the default L1 suite (no real D1/R2 needed)」的声明自相矛盾。
- **修正：** 改为按 `LIBRA_D1_ACCOUNT_ID` 是否配置分别断言两支——已配置则要求探针解析出非空身份；未配置则用 `catch_unwind` 要求探针 fail-closed。实测**无 env** 与 **`source .env.test`** 两种环境均为 `8 passed; 0 failed`，使文件头与 `tests/INDEX.md:225` 的「default L1 suite (no real D1/R2)」陈述重新成立。
- **附注：** 上面第 2 条里的「`cloud_live_resources_test` 3/3 绿」是当时的数字；该 target 现含 **9** 个测试（`cloud_live_manifest` 4 + `cloud_live_resources` 4 + 冒烟 1），引用时应以现有计数为准。
- **同批第二处门禁破损（格式）：** `cargo +nightly fmt --all --check` 在 `e9c46b1` 上**退出 1**，违规点全在本计划 Cloud 卡新增的两个 helper：`tests/helpers/cloud_live_manifest.rs`（10 处）与 `tests/helpers/cloud_live_resources.rs`（7 处）——即 `208fc99`/`338dd95` 提交时没有跑过 nightly fmt。CI 的 `compat-rustfmt` 作业（`.github/workflows/base.yml:12`，`run: cargo +nightly fmt --all --check`）因此在下一次 PR 上必然红；而 `base.yml` 的 `on:` 只有 `pull_request`，所以直推 `main` 不会触发，属于**潜伏的红**（`main` 自身处于违约状态却看不到）。已执行 `cargo +nightly fmt --all` 修复；改动经 AST 等价核验——去空白再去尾随逗号后逐字符相同，唯一结构差异是 `unwrap_or_else` 里的 `panic!(...)` 被包成块表达式，语义不变；修好后 `fmt --all --check` 退出 0，`cloud_live_resources_test` 仍 8/8 绿。

### G-3c/G-3d 缺陷修正（2026-09-29 逐行复核 @ `b8176c8`）

`reject_unregistered_sink_write` 与 `authorized_cleanup_scope` 是本计划「未登记 repo/前缀被拒」的**唯一**证据载体（全仓除它们自己的单元测试外尚无引用，真实 sink 接线仍待 G-3d）。逐行复核发现五处 **fail-open**（第 1–2 处为「应当拒绝时却返回 `Ok`」，第 3 处为「未知即放行」，第 4–5 处为「校验存在但不生效」）：

1. **`reject_unregistered_sink_write` 对非 `test-repo-`/`libra` 前缀的 repo_id 完全跳过登记检查。** 原实现把登记检查嵌套在 `if repo_id.starts_with("test-repo-") || repo_id.starts_with("libra")` 内，因此 `reject_unregistered_sink_write("prod-users", "test-repo-a/", slots)` 返回 `Ok(())` —— 只要 R2 前缀落在已登记槽位内，任意**未登记** repo_id 的 D1 写都被放行。这与同函数文档注释（「reject any D1/R2 write keyed by a repo ID … outside the registered writer slots」）、同文件姊妹守卫 `assert_registered_repo`（无条件校验登记）以及紧随其后的 R2 前缀检查（无任何豁免）三处都矛盾；且 `validate_writer_slots` 要求槽位 repo_id 必为 `test-repo-<uuid>`，使 `libra` 分支纯属装饰。原测试只覆盖 `test-repo-X`（恰好落在被豁免的前缀族内），所以该漏洞无测试。**修正：** 去掉前缀豁免，对**每一个** repo_id 无条件要求命中已登记槽位；新增回归用例覆盖 `prod-users`/`acme-monorepo`/`user-repo-123`（原实现会放行的类）。
2. **`authorized_cleanup_scope` 只对 restore_target 校验登记，对 `writer_slots` 不校验。** 该函数把 **caller 传入**的 `serde_json::Value` 里的 `writer_slots[].repo_id` 直接并入 `d1_repo_ids`（删除授权集合），却对 `restore_target_slots[].source_repo_id` 做 `registered.contains` 检查。因此一份列出未登记 writer slot 的 manifest 可以扩大 D1 删除范围 —— 对一个携带删除权限的作用域构造器而言是数据丢失面。**修正：** `writer_slots` 同样要求命中 `registered`，否则 `Err`；`CleanupScope` 由此保证 ⊆ 已登记槽位。原有 `cleanup_scope_is_bounded_and_fails_on_unregistered_target`（槽位 a/b）不受影响，另加「manifest 借未登记 writer slot 扩大范围必须失败」用例。
3. **`slot_is_live` 把「解析失败」当作「仍有效」。** 原实现为 `parse_rfc3339(&slot.expires_at).map(|exp| now < exp).unwrap_or(true)` —— `unwrap_or(true)` 使任何无法解析的 `expires_at`（缺失 `Z`、字段越界、纯垃圾串）都被判为 live。该函数是写者槽位的**截止期门**，未知截止期应当拒绝而非授权无界写入窗口。且该函数在**全仓无任何测试**。**修正：** 改为 `is_some_and(...)`（未知即不 live），新增 `slot_deadline_fails_closed_on_unparseable_expiry` 覆盖未来/已过期/不可解析三支。
4. **`validate_grant` 的 nonce 只校形状、从不比对。** `tests/helpers/cloud_live_manifest.rs` 原实现在形状检查后直接 `let _ = nonce;`，且函数签名只有 `expect_run_id/attempt/ref/head_sha`，**没有 `expect_nonce` 参数** —— 即同名 one-time grant （GC-CM-17）在同一 `run/attempt/ref/SHA` 四元组下**可重放**，与计划「grant run/attempt/ref/SHA/nonce 匹配」相违。全仓除自身测试外**无调用方**，故此前的「4 项通过」并不能证明 nonce 被比对。**修正：** 签名增加 `expect_nonce: &str` 并真正比对，新增「nonce 不匹配必须失败」用例。
5. **`grant.expires_at` 可为缺失，且 `is_rfc3339_utc` 默许无 `Z`。** 原实现为 `if let Some(expires) = ….and_then(as_str) && !is_rfc3339_utc(expires)` —— 即**字段缺失时不报错**，一份无过期时间的 grant 可完全通过校验（写授权无界）；而 `is_rfc3339_utc` 用 `strip_suffix('Z').unwrap_or(s)`，把 `2099-01-01T00:00:00`（无 UTC 标记）也当 UTC 接受。**修正：** `expires_at` 改为必填（缺失即 `Err`），`is_rfc3339_utc` 改为**必须**带 `Z`（与同批新增的 `cloud_live_resources::parse_rfc3339` 一致），新增「无 expires_at 必须失败」「无 `Z` 必须失败」用例。

> 说明：第 3–5 项位于 `cloud_live_manifest.rs` / `cloud_live_resources.rs`，即 **G-3b/G-3c/G-3d 共同依赖的校验器**；G-3b 当时那句「单元测试 4 项通过」是在上述三处 fail-open 仍存在的前提下取得的，引用该条时以本块为准。

**性质：** 上述五处都是对**已落地**产物的前向修正（forward-fix），与 G-1/G-3e 同批；不触碰 `Cargo.toml` 版本面，也不属于 `plan-20260927.md` 所禁止的「开工新卡」（守卫已存在，仅令其兑现自身已声明的契约）。G-3c/G-3d 那两个 `[x]` 的「已实现」陈述因此**只有在本修正之后**才成立，引用时以本块为准。

### G-3e 缺陷修正（2026-09-29 实测复核 @ `b8176c8`）

`tests/cloud_live_prepare.sh`（G-3e 第 1 项，已勾）此前只做过 `bash -n` 语法检查与「幂等」的口头声明。本轮把它**真实执行**，并用同批落地的校验器（`cloud_live_manifest.rs`）反向验证其产物，发现三处缺陷。「幂等」声明本身复现为真：同一 `--slots-dir` 二次运行退出 0 并提示 `slots already provisioned … (pass --force to regen)`，`--force` 才重生成（`--count` 随之生效）。

1. **产出的 `slots.json` 违反 GC-CM-15 升序约束，会被自己的校验器拒绝。** 脚本按 `uuid4()` 生成顺序 push 槽位，而 `validate_writer_slots` 要求 `repo_id` 字节**严格升序**（`tests/helpers/cloud_live_manifest.rs:319`：`p.as_bytes() >= repo_id.as_bytes()` 即 `Err`）。实测把脚本产物喂给该函数：`--slots slots.json: REJECTED -> writer_slots not strictly ascending by repo_id`。即「预登记槽位」与「校验器」两侧互不兼容，任何按计划调用 `load_writer_slots` / `validate_writer_slots` 的消费方都会在写前门拒绝刚铸出的槽位。**修正：** 写盘前 `slots.sort(key=repo_id)`，并按新顺序重编 `repo_name`，使 `slots.env` 的 `SLOT_<i>` 下标与 `slots.json` 顺序始终一致。实测同一产物现在 `--slots …: OK`，且 `env[i] == slots.json[i]` 四条全 True。
2. **每个槽位一铸出就已过期，`run.expires_at` 同样。** 原实现 `expires_at = date -u`（铸造时刻），而 `slot_is_live` 的语义是 `now < expires_at`——于是**全部**预登记槽位在诞生瞬间即被判为 dead。这使 GC-CM-15 的「写者截止期」门（以及本轮刚改为 fail-closed 的 `slot_is_live`）会把每一次 live 运行整体拒绝；反之若消费方忽略该字段，则这条安全属性形同虚设。两种读法都不可接受。**修正：** 改为 `expires_at = now + LIBRA_CLOUD_LIVE_SLOT_TTL_SECONDS`（默认 3600 秒），槽位与 `run` 共用同一截止时刻。实测 `live_now=True`、`ttl=3598s`；覆盖 `TTL=120` 时 `ttl=119s`；`TTL=abc` 在 `set -euo pipefail` 下退出 1 且**不落任何产物**（0 个文件）。**接线注意（G-3a/G-3e）：** 截止期首次真正生效后，TTL 必须覆盖整个 live job 的墙钟——已观测的 `compat-live-cloud` 单次运行为 21 例 539.84s，默认 3600s 尚有余量，但真实 dispatch 前应按两个 target + 重试的实测时长复核，否则槽位会在运行中途过期并被 fail-closed 拒绝。
3. **`manifest.json` 自称「global write-manifest envelope」，但它不是 GC-CM-15 envelope。** 其 `schema` 为 `libra-cloud-live-prepare-v1`，且缺少 `validate_manifest_schema` 要求的 `source/recovery/resources/global_preimage/backup/local_restore/restore_target_slots/mac_key_id`。实测：`--manifest manifest.json: REJECTED -> manifest schema must be libra-cloud-live-manifest-v1`。**本树不把它补成真 envelope**（`resources` 四元组、`mac_key_id`、全局前像只有 CI 接线与仓库管理员能提供），只把误导性注释改为如实说明：这是 prepare 期的 run/slot 记录，被 `validate_manifest_schema` 拒绝属预期；真正的 envelope 由（仍待接线的）写前门产出。该缺口并入 G-3c/G-3e 的 CI 接线待办。

**性质：** 与上一条 G-3c/G-3d 修正同批，均为对**已落地**产物的前向修正：不新增能力、不触碰 `Cargo.toml` 版本面、不属于「开工新卡」。第 1、2 条修好后，G-3e 第 1 项的「已验证幂等可运行」才不仅指幂等为真，还指其产物能被同批校验器接受。

---

## G-4：22 项 EX + 计划级 `VERDICT: PASS`

- [ ] 把「实现已完成 + nextest 8274/8274 绿 + v0.30.8 已发布」整理为计划级评审证据。
- [ ] 用新冻结 SHA 重新提交计划评审：取得 Claude Code 与 Codex 字面 `VERDICT: PASS`。
- [ ] 由具名独立 reviewer 对 22 条 `EX-CM-*`（准确分子，如 CM-02 `AC=39/8`、CM-13 `AC=34/8`）逐一书面同意。
- [ ] 全部通过后，在 `plan-status.md` 把对应卡 `Lifecycle/Acceptance` 由 `in-progress`/空 更新为 `done`/完整，并删除或关闭本临时计划。

### G-4 计划级证据整理（2026-09-29 独立远端核实 @ `2c66f86`）

**已核实（本轮以 `gh`/`libra` 直接读回，非转引 `plan-status.md`）：**

| 证据 | 实测 | 核实来源 |
|---|---|---|
| `v0.30.8` 已发布 | annotated tag `v0.30.8`，tag 对象 `bd083fa9d9a05604ac0b9d5bc8a8b8df4d66ff53`，tagger `Eli Ma`，message `Release v0.30.8`，创建 `2026-09-29T14:28:09Z`，非 draft/prerelease | `gh release view v0.30.8`、`gh api repos/libra-tools/libra/git/tags/<oid>` |
| `release.yml` 8/8 绿 | run `36582964833`：`build-and-upload`×4（`aarch64-unknown-linux-gnu`/`x86_64-pc-windows-msvc`/`aarch64-apple-darwin`/`x86_64-unknown-linux-gnu`）+ `update-homebrew-tap` + `request-stable-manifest` + `upload-install-scripts` + `verify-homebrew-formula` 全 `success` | `gh run view 36582964833 --json jobs` |
| peeled commit | `0358668d220000e03704aef6c7206a45eae651ba`（"…; bump to v0.30.8"），且**是 `main` 的祖先** | `gh api` + `libra merge-base --is-ancestor` |
| 版本面未被本计划触碰 | `main` 上 `Cargo.toml`/`install.sh`/`install.ps1` 各 1 处 `0.30.8`，与执行规则「不 bump」一致 | `grep -c` |
| live-compat 真 D1/R2 21/21 | run `36602151296` job `compat-live-cloud` success；日志 `test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 539.84s`（另 1 处 `1 passed`） | `gh run view 36602151296 --log` |
| 制品通道 | release 的 GitHub assets 为空是**预期**：`release.yml` 首行为 `Build and Release to R2`，产物经 R2/CDN 发布 | `.github/workflows/release.yml:1` |

**未在本树独立复现：** 「nextest 8274/8274 绿」目前仅为 `plan-status.md` 的发布者 attest。复现需 `source .env.test && source .env.live-test`，而本计划 D-CM-STD（`plan-20260927.md:202`）明令：在写前门证明默认解析仍为 `local/tiered=false` 之前，不得带 live 凭据跑默认全量。故本轮**刻意不跑**，留待发布者在通过 D-CM-STD 预检的树上执行。

**残留风险（建议纳入评审口径）：** `gh api … .verification.verified=false`，`reason=unknown_key` —— GitHub 侧无法验证 `v0.30.8` tag 签名（签名者公钥未注册到 GitHub 账号）。D-CM-STD 要求「独立读回远端 annotated tag 对象 OID、**签名验证**和 peeled commit SHA」，若此项须由第三方在 GitHub 界面完成，则当前**不满足**；若只要求本地 `libra tag -v`，请评审明确记录该口径。

**阻断性发现：步骤 1 的前提不成立。** 步骤 1 要求整理「**实现已完成**」的证据，但 `plan-20260927.md` 的 22 张卡当前**无一张** `Lifecycle/Acceptance` 为 `done`/完整。故本轮只整理了「已发布 + 已绿」的部分并保持本项未勾选；「实现已完成」须待 22 卡按依赖序逐卡 `done` 后重述。

**关键文件：** `docs/development/plan/plan-20260927.md`（评审记录/修订历史），`docs/development/plan/plan-status.md`。

---

## G-5：L1 验收门 flaky 车道修正（2026-09-29 全量实测 @ `b8176c8`）

**现象：** 本轮跑默认验收门 `cargo test --all`（`AGENTS.md:99` 称之为 “the default L1 suite (the acceptance gate)”）得到 `--lib` **3303 passed / 1 failed**，失败者 `utils::client_storage::tests::legacy_scratch_cannot_starve_a_real_repair_marker`，panic 于 `src/utils/client_storage.rs:5347`：`bounded repair invocation should make progress: "injected object index update failure"`。该错误串**只**可能来自测试 failpoint `LIBRA_TEST_OBJECT_INDEX_UPDATE_FAIL`（生产侧两处受 `cfg!(debug_assertions)` 门控的注入点：`apply_pending_object_index_page`（`client_storage.rs:3280`）与 `update_object_index`（`client_storage.rs:3693`）），而该变量只由同文件 4 个 `#[serial(env)]` 用例经 `ScopedEnvVar`（`src/utils/test.rs:157`）在**进程内**设置。

**机制（已实测定性）：** serial_test 的**裸** `#[serial]` 只锁空字符串 key，与具名 lane `env` **互不排斥**（`tests/SERIAL_CONVERT.sh` 头注释原文：「an unkeyed attribute locks only the empty-string key and is NOT exclusive with named lanes」）。于是「写 failpoint 的 `#[serial(env)]` 用例」与「读 failpoint 的裸 `#[serial]` 用例」可并行交错，后者在 `.expect(…)` 上 panic。四组实测（`cargo test --lib -- <filters> --test-threads=4`）：

| 场景 | 结果 |
|---|---|
| 单独跑受害用例 | `1 passed`（隔离下必然绿） |
| 外部 `LIBRA_TEST_OBJECT_INDEX_UPDATE_FAIL=1` 跑受害用例 | `FAILED` @ `:5347`（证明确实读该变量） |
| 受害用例 + `concurrent_direct_storage_work_is_not_charged_to_cli_scope` 并行 ×3 轮 | **3/3 `FAILED`**（稳定复现，非偶发） |
| 整个 `client_storage` 模块 `--test-threads=16` | 63 passed（需并行恰好落进该窗口才触发） |

**为何普查/分类器没拦住：** `tests/SRC_SERIAL_CENSUS.tsv` 的 `touches` 只建模**进程级污染写入**（`set_var`/`remove_var`/`ChangeDirGuard`/hash-kind setter），不建模 failpoint 的**读取**（`std::env::var_os`）；该行 `reason` 的 callee 串里也**没有** `repair_pending_object_index_updates`（类型限定调用 `ClientStorage::…`，属分类器自陈的解析边界），故被判 `touches=none`（即 `DEFER-SH-01` 明确保留的「无 key 且 `touches=none`」集合）。分类器头注释本身写着「a wrong `none` costs a flaky suite」。**同类第二例：** `classify_read_failure_pins_storage_messages` 在持空 key 的情况下调用**跨 crate** 的进程级全局写入 `git_internal::hash::set_hash_kind_for_test(HashKind::Sha1)`，可污染同文件两条 `#[serial(hash_kind)]` 读取者（`client_storage_reads_pack_sha1` / `client_storage_reads_pack_sha256`）——分类器对该行判 `lane:hash_kind`，同样是**车道缺口**（应锁而未锁），只是方向相反（写入者未锁）。

**修正（本文件 10 行的机械转换，判据全部取自分类器自身输出）：** 把 `src/utils/client_storage.rs` 中**剩余 10 处**裸 `#[serial]` 改为分类器要求的车道——9 处 `#[serial(cwd, env, hash_kind)]`、1 处 `#[serial(hash_kind)]`。该形式不是新发明：同文件此前已有 3 处同形钉子（`queued_reconciliation_ignores_an_unrelated_deletion_fence`、`update_object_index_rejects_missing_database`、`update_object_index_upgrades_generic_blob_to_agent_specific_o_type`），`SRC_SERIAL_REGISTRY.tsv` 中 `lane:cwd+env+hash_kind` 共 30 行。改后 `SERIAL_CLASSIFY_TREE=src tests/SERIAL_CLASSIFY.sh` 退出 0，且这 10 行的判定正好变为 `lane:cwd+env+hash_kind`（9）/ `lane:hash_kind`（1）——即转换**幂等**且被分类器认可（不存在「具名 key 未覆盖污染」的拒绝）。

**同步的派生记录（均由上述分类器输出导出，未手工臆造）：** `SRC_SERIAL_CENSUS.tsv` 这 10 行的 `attr` 由 `unkeyed` 改为 `keyed:…`、`touches` 由 `none` 改为 `env+cwd+hash_kind` / `hash_kind`，`reason` 追加 `;converted:plan-20260927;<why>`；`SRC_SERIAL_REGISTRY.tsv` 新增 10 行（按文件内 `fn` 序合并，数据行 444→454）。按 ADR-SH-01（`plan-20260917.md:135`）`src/` 注册表**不进入** nextest 分组，故无需重跑 `tests/NEXTEST_GROUPS.sh`。

**性质：** 受害用例在 `beab6b7`（本计划父提交）中即已存在，属**既有**缺陷、非本计划引入；本轮按「验收门不绿不得宣称完成」的口径做前向修正（不新增能力、不碰版本面、不属于「开工新卡」）。**残留风险（P1，需 owner 处置）：** 全仓仍有 **61** 处 `src/**` 裸 `#[serial]`（分类器判 `global` 47 / `lane:hash_kind` 13 / `lane:cwd` 1），本次只修了 L1 验收门实际触发的那 10 处，其余 51 处同样可能在并行下被污染。建议由 `plan-20260917` SH-02/TA-03 owner 以「分类器 + 普查/注册表重生成」的方式统一收口，而非零散改属性。

---

## 执行规则

- 每张 gap 独立提交、独立验收；提交信息用 `fix(...)`/`feat(...)/`docs(...)` 前缀，带 `plan-20260927` 引用。
- **不 bump 版本**：不触碰 `Cargo.toml`/`install.sh`/`install.ps1`/`Cargo.lock` 的 `=0.30.8`。
- 本计划是纯跟踪文档；后续修复落地时按 G-01..G-11 逐项审计，任一 gap 未完成不得宣称整计划 complete。
