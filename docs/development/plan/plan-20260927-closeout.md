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
- [ ] **定位漏点**：
  - 若 `object_index` 无该行 → 修写入路径（`client_storage.rs` 写对象后确保入对象索引；`db.rs:1200/1261` 的登记点）。
  - 若有行但 `is_synced=1` 却不在 R2 → 修 sync 的 `exist_batch`/上传时序（`sync.rs:157` 起批量上传），保证「标已同步」仅在确实上传成功后发生。
  - 若 fsck 发现对象不被 sync 覆盖 → 让 sync 也上传 reflog/`extra_roots` 可达对象，或调整 fsck 候选集。
- [ ] **加回归**：新增一个测试，构造「离库对象在 R2 存在」场景，断言 `--heal` 后 `unrecoverable==0` 且全部愈合；若本地可跑则并入 `cloud_storage_backup_test`，否则只在 live 门（`cloud_live_no_skip.sh`）下运行。
- [x] **验证**：dispatch `main`（含 `cloud sync` 修复）live runs，`compat-live-cloud` **21/21** 绿（`fsck_heal`、`cloud_sync_name_conflict` 均 ok）。

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

## 执行规则

- 每张 gap 独立提交、独立验收；提交信息用 `fix(...)`/`feat(...)/`docs(...)` 前缀，带 `plan-20260927` 引用。
- **不 bump 版本**：不触碰 `Cargo.toml`/`install.sh`/`install.ps1`/`Cargo.lock` 的 `=0.30.8`。
- 本计划是纯跟踪文档；后续修复落地时按 G-01..G-11 逐项审计，任一 gap 未完成不得宣称整计划 complete。
