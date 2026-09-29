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

**现状：** `src/command/worktree/operations.rs::move_worktree` 已有 `fs::rename` + `or_else` 复制回退，及 `journal_*` 持久 intent。**实现基本在**，卡点是补丁兼容性证明。

**具体动作：**
- [ ] **补丁兼容源证**（在脚本/测试内可执行的证明）：
  - [ ] 仅 pending EXDEV 的持久 fence：新 `worktree move` 写下的 intent，旧 `worktree prune/remove`（不检查 pending move）不能无视它作删除。
  - [ ] 旧 repair 不能动坏 v2 状态：NUL 哨兵/保留字段确保旧 `repair` 读旧格式、不覆盖新字段。
  - [ ] `down` 与 `v1→v2` 转换同事务串行，且旧进程已退出才允许升级（`migrate_layout`/registry 版本面）。
  - [ ] 用旧二进制与 `worktree prune --dry-run`/`repair` 各做一次跨编译回归（不 bump，只用当前树生成的东西验证语义）。
- [ ] **若任一证不了**：记录 **breaking/minor 方案**（含独立兼容窗口与 ER-08 版本约束），呈给用户/维护者取决定；**不预设 minor 已授权**。
- [ ] 状态更新：`DEP-CM-WT-COMPAT` 由 `blocked` → `passed`（具名 owner 确认）后，FIX-CM-WT-MOVE、CM-05、CM-13 才正式验收。

**关键文件：** `src/command/worktree/operations.rs`（move_worktree/journal_*）、`src/command/worktree/registry.rs`（版本面）、`docs/development/plan/plan-status.md`。

---

## G-3：4 个 Cloud FIX 卡基建

**一般说明：** 这些是 CI/环境/受保护协议工作。用户侧需先配置受保护 `cloud-live-write` environment、七项 secret 迁离 repo 级、四项 vars；本地只做 fake/mock/default C。

### G-3a FIX-CM-LIVE-GATE（接线）
- [ ] `tests/cloud_live_no_skip.sh`（已存在，`--self-test` 全绿）接入 `live-compat.yml` 的 `Run live cloud tests` 步骤，替换旧 `skip=true` 分支。
- [ ] 真实 run 用 JSON list 钉 selected count、no-skip 核 run/pass、保留原始日志。

### G-3b FIX-CM-CLOUD-RECOVERY-AUTH
- [ ] 新建 `cloud-live-recover.yml`（GC-CM-14：AUTH 期仅 `probe-auth`、CLEANUP 后仅 `cloud-live-recovery/v1`）。
- [ ] 实现 `libra-cloud-live-manifest-v1` 的 Python 字节规范/HMAC 校验与 `LIBRA_LIVE_GRANT` parser（GC-CM-15/17）；零删除探针：来源/ref/SHA/nonce/owner/expiry 任一不符即零删除。

### G-3c FIX-CM-CLOUD-RECOVERY-CLEANUP
- [ ] 在 `cloud-live-recover.yml` 增加限界幂等清理：仅按已签 manifest 的 `repo_id`/R2 前缀删除 D1 行与精确 key，重试到零，保留备份与 receipt。

### G-3d FIX-CM-CLOUD-REPO-SCOPE
- [ ] 在真实 CLI、D1 ensure、R2 mutation sink 校验已签 repo/slot 身份；用本地 fake endpoint 证明未登记 repo_id/前缀被拒（零远端写）。

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

**关键文件：** `docs/development/plan/plan-20260927.md`（评审记录/修订历史），`docs/development/plan/plan-status.md`。

---

## 执行规则

- 每张 gap 独立提交、独立验收；提交信息用 `fix(...)`/`feat(...)/`docs(...)` 前缀，带 `plan-20260927` 引用。
- **不 bump 版本**：不触碰 `Cargo.toml`/`install.sh`/`install.ps1`/`Cargo.lock` 的 `=0.30.8`。
- 本计划是纯跟踪文档；后续修复落地时按 G-01..G-11 逐项审计，任一 gap 未完成不得宣称整计划 complete。
