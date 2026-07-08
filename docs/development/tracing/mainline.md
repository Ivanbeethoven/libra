# `mainline` 对标改进计划：为 Libra 打通 git-native 可迁移意图记忆（Intent Portability & Pin）

## 文档职责

本文是 `docs/development/tracing/` 目录下的**独立对标提案**（draft proposal），职责是：以竞品项目 **mainline**（Go 实现的 "Git for the AI era"，把工程意图——目标 / 决策 / 被否方案 / 约束 / 风险 / 遗留项 / 语义指纹——存进 **Git refs + Git notes**，随 fetch / branch / merge / fork 天然流转）为参照系，逐能力对照 Libra 现有与计划中的 AI 意图 / agent / projection 基础设施，给出一份**可落地执行**的改进计划，并明确它相对同目录其他计划的**先后顺序与依赖**。

- 本文是**提案草稿**，与 `memory.md` / `sandbox.md` / `web-api.md` 同类，**不属于** [`plan.md`](plan.md) 固定的 `agent.md → code.md`（AG-16~AG-24a → C1~C8）执行链条。按 `plan.md` §0 规则，本文不得从 `memory.md` / `sandbox.md` / `web-api.md` 引入验收标准；可引用的设计权威只有 [`agent.md`](agent.md) 与 [`code.md`](code.md)（以及它们背后的源码事实源 `docs/development/internal/code-agent-runtime.md`）。
- 本文与 [`memory.md`](memory.md) 存在**五轴重叠**（意图记忆 / 决策证据记录 / 上下文注入 / 约束 / Hub 读视图）。二者的收敛策略在 §9 明确：本文成为「意图可迁移 / commit-pin / recall」这一轴的**受纳计划（committed schedule）**，`memory.md` 由其 owner 对齐，避免出现第三条平行平面。
- **完成判定以代码为准**：本文所有源码锚点均在撰写时经过实测核对（见 §11 源码事实索引）；任何实现推进都必须重新核对锚点，且更新代码时同步更新本文。

## 命令实现目标

Libra 已经拥有 mainline 所需的**绝大部分底层机件**（内容寻址 AI 对象、孤儿分支、CAS 追加、git notes、可推送自定义 ref、SQLite 投影重建、MCP、外部 agent 捕获）。因此本计划的目标**不是移植 mainline 的实现**，而是：

1. 把 Libra 已有的、**局部（local-only）**的意图 / 决策记录，改造成**随仓库流转的团队级可迁移意图平面**；
2. 补齐 Libra 缺失的三块高价值能力——**seal + intent↔commit 绑定（pin）**、**决策的「被否方案 + 理由」一等结构**、**改动前意图检索（intent-before-code）**；
3. 在此基础上增量补齐冲突检测、coverage/gaps、hook 上下文注入、Hub 读视图，最后再做多 actor / fork 协作与 eval。

## 对比 Git 与兼容性

- 兼容级别：`intentionally-different`。这是 Libra 的 AI 扩展平面，不是 Git 命令；不追求与任何 Git 子命令同形。
- 与 mainline 的关系是**概念对标**，不是二进制或线格式兼容：Libra 复用**自身**的 `refs/libra/traces` + `history.rs` 传输/存储范式，**不采用** mainline 的 `refs/notes/mainline/*` 机制（原因见 §5 核心设计决策）。

---

## 1. mainline 是什么（对标对象速览）

mainline 的核心论点：**Git 记录了代码「改了什么」，mainline 把「为什么这么改」也放回 Git**，让下一个 agent 在动手前先读到历史判断（被放弃的路线、被取代的决策、跨越代码本身的约束），从而不重复踩坑、不撤销昨天的决策、不违反没见过的硬约束。

其对象与存储模型：

| mainline 概念 | 内容 | 存储位置 |
|---|---|---|
| Intent（意图记录） | goal / summary(what,why,decisions,rejected) / fingerprint / lifecycle | actor 事件日志 `refs/mainline/actors/<id>/log`（append-only JSON-lines） |
| Materialized view | 由重放 actor 日志重建的只读视图 | `.mainline/view.json`（派生、gitignored） |
| Commit pin | sealed intent ↔ main commit 的绑定 | git notes `refs/notes/mainline/intents`（tree_hash→commit_hash→goal_text 级联，抗 squash-merge） |
| 持久信号 | constraint（人类确认硬约束，永不截断，按文件重叠继承）/ risk（顾问式，open/resolved/expired）/ followup（显式遗留） | actor 日志事件 |
| Coverage | 每个 main commit ∈ {covered, skipped, uncovered} | 由 pin + skip trailer 派生；`gaps` 命令 |
| Conflict | phase-1 指纹重叠（加权 Jaccard）/ phase-2 agent 语义判定（`check --prepare/--submit`） | 引擎计算 |

其工作流（agent 视角）：`preflight → context(--current/--files/--query) → 读决策/约束/生命周期告警 → 校验意图 vs 代码 → 编辑 → append → seal(--prepare/--submit) → sync（自动 fetch+rebuild+auto-pin+overlap 告警）`。人类侧：`status --actionable / log / show / trace / gaps / hub open`。协作侧：`publish`（推 actor 日志）、`actor import` / `pr-import`（fork 贡献者意图的上游信任导入，只接受 author-seal）。另有 Hub（意图视图的静态 HTML 导出 + GitHub Pages）与 eval（8 场景×3 seed×2 模式，量化「intent-first vs code-first」的价值）。

```mermaid
flowchart LR
    subgraph MAINLINE["mainline（对标）"]
        MA["actor log<br/>refs/mainline/actors/*/log"] --> MV["view.json（重建）"]
        MA --> MP["commit pin<br/>refs/notes/mainline/intents"]
        MP --> MC["main commit"]
        MV --> MHub["Hub / gaps / conflict"]
    end
    subgraph LIBRA["Libra（现状）"]
        LA["AI 对象<br/>refs 分支 libra/intent（本地！）"] --> LV["SQLite 投影<br/>ai_thread*/ai_index_*（重建）"]
        LA -.->|"缺：commit_sha=None"| LC["code commit"]
        LT["外部 agent 捕获<br/>refs/libra/traces（已可推送）"]
    end
    LT -.->|"复用其传输范式"| LA
```

---

## 2. 对比结论（Libra 现状 vs mainline）

经 13 路并行源码复核 + 综合分析 + 逐锚点实测核对，四条承重结论：

1. **Libra 不缺 git-native 存储，缺的是意图平面的 git-native「流转」。** 所有 AI 对象（`Intent`/`Task`/`Run`/`Plan`/`PatchSet`/`Evidence`/`ToolInvocation`/`Provenance`/`Decision`）已经是内容寻址 Git 对象，挂在孤儿分支 `libra/intent`（`src/internal/ai/history.rs:72`，以 kind='Branch'、name='libra/intent' 存于 SQLite reference 表），CAS 追加、GC-root 保护，模块文档明言「可经同一协议传输」。**但 `libra/intent` 从不在默认 push/fetch/clone 中出现**（实测 `src/command/{push,fetch,clone}.rs` 均无 `libra/intent`；`clone.rs:3597` 刻意不写 `+refs/*:refs/*`）→ Libra 的意图记录事实上是**本地私有**的。相反，`refs/libra/traces` 已经有可用传输：`libra agent push` 以 `refspec = "{TRACES_BRANCH}:refs/libra/traces"` + `--force-with-lease` 推送（`src/command/agent/push.rs:29-36`）。**机件齐全，管线未接。**

2. **中心问题的答案是「是」，但要走 Libra 自己的 traces/history.rs 范式，不走 mainline 的 git-notes。** 详见 §5。

3. **单点最高价值缺口是 intent↔commit 绑定（pin）。** IntentSpec 在 plan 期被持久化时 `commit_sha: None // Will be set when completed`（`src/internal/ai/intentspec/persistence.rs:54`），完成后从不回填，`commit.rs` 也不写反向 back-link → 意图与它所授权的 diff 是**两个互不相连的对象**。没有 pin，就没有 coverage、没有 seal 时刻的指纹、没有「这段代码为何如此」的可追溯性。

4. **Libra 的决策记录缺「被否方案」结构**——恰是 mainline 的核心价值主张。`agent_run::MergeDecision` 只有单一 verdict（`src/internal/ai/agent_run/decision.rs`），`ai_final_decision` 只存不透明的 `summary_json`（`src/internal/model/ai_final_decision.rs`）；「权衡了哪些备选 / 为何选 X 弃 Y」不可查询。

另有两条工程约束必须贯穿全程：

- **不要在 `agent_run` 模块上盖楼**：它是 schema-only、受 CP-4 门控、藏在 `subagent-scaffold` feature 后（`src/internal/ai/agent_run/mod.rs:5,13`，类型只实现 Serialize/Deserialize）。真正可用（live、已接线）的平面是 orchestrator + `intentspec/persistence.rs` + git-internal AI 对象 + `ai_index_*` SQLite 投影。
- **Libra 的 hook 是「只捕获」方向**（`src/internal/ai/hooks/runtime.rs` 的 `ingest_agent_traces_payload`），与 mainline「向 agent 注入上下文」正好相反；注入是净新增、只读、绝不 mint 意图。

---

## 3. 能力差距矩阵

按类别归并（源自 32 行逐能力对照）。`状态`：has-current（已具备）/ partial（部分）/ absent（缺失）/ different（有意不同）。`价值`/`工作量`：对 Libra 而言。锚点为实测源码位置或计划锚。

### 3.1 存储与传输（storage）

| mainline 能力 | Libra 状态 | Libra 锚点 | 差距与代价 | 价值/量 |
|---|---|---|---|---|
| 意图 git-native 随 fetch/push/merge/fork 流转 | partial | `history.rs:72`（对象可传输）；push/fetch **无** `libra/intent` | 机件在，管线缺；照搬 traces 传输即可 | 高 / M |
| commit pin（notes 绑定意图↔commit，抗 squash） | **absent** | `persistence.rs:54`（commit_sha=None） | 最大单点缺口；**不走 notes**，走 `history.rs` 结构化事件 | 高 / L |
| log→view 重放式物化视图 | has-current | `projection/rebuild.rs:135`（事务化销毁重建） | 已等价；弱点：无 built-from 水位线，判新鲜度需全量重建 | 低 / S |
| per-actor append-only 日志 + CAS ref 更新 | has-current | `history.rs`（CAS）；`agent_run/event_store.rs:82`（run JSONL） | CAS 有；缺**多 actor 身份合流**（traces 单属主 first-writer-wins） | 中 / M |
| note upsert-merge（同一 squash commit 多意图共存） | different | `notes.rs:101`（无 NOTES_MERGE worktree，2-way 行合并） | notes 不用于 pin；同树 fan-out 去重要在 pin writer 里重写 | 中 / M |
| refspec 接线（元数据随普通 git push/fetch 走） | partial | `agent/push.rs`（traces 可推）；`fetch.rs` 硬编码 heads/tags/mr | 出站可任意目的 ref；入站命名空间不可配；线上可行、文件系统级 git-client 互操作不可及 | 中 / M |
| 历史重写后 notes 恢复（migrate notes 重键） | absent | 无 | pin 落地后重写会悬挂 pin，需重键路径；pin 存在前不急 | 低 / M |

### 3.2 生命周期（lifecycle）

| mainline 能力 | Libra 状态 | Libra 锚点 | 差距与代价 | 价值/量 |
|---|---|---|---|---|
| drafting→sealed→proposed→merged→abandoned/superseded/reverted | partial | `intentspec/types.rs`（Active + IntentEvent + 修订链） | 有本地生命周期；缺团队可见的 seal/proposed/merged 推进 | 中 / M |
| **seal**（在 commit/PR 边界冻结 summary+fingerprint） | **absent** | `persistence.rs:25`（plan 期存 Active，从不冻结） | 关键拱心石；pin/指纹/coverage 的前置 | 高 / L |
| 决策记「被否方案+理由」为一等字段 | **absent** | `decision.rs`（单 verdict）；`ai_final_decision.rs`（不透明 summary_json） | mainline 核心价值；需在 **live 平面**的 git-internal Decision 对象上加结构 | 高 / M |
| 持久信号 constraint/risk/followup（open/resolved/expired + 继承） | partial | `intentspec/types.rs`（声明式 per-task 约束）；`profiles.rs`（风险档默认） | 有静态 per-spec 策略块；缺团队共享、带生命周期的信号队列与跨意图继承 | 中 / L |
| backfill（`--commits/--range` 认领既有 commit） | absent | 无 | pin 存在后便宜；救援/coverage 用 | 低 / S |
| abandon/supersede 团队可见事件 | partial | `repair.rs`；`new_revision_chain` | 本地够用；缺随 view 流转的 reason/provenance | 低 / S |

### 3.3 检索 / 冲突 / coverage（retrieval / conflict / coverage）

| mainline 能力 | Libra 状态 | Libra 锚点 | 差距与代价 | 价值/量 |
|---|---|---|---|---|
| 编辑前三模检索（--current/--files/--query） | **absent** | 无（context_budget 是 prompt 窗口分配，非意图检索） | mainline 量化价值集中处；可作为 memory.md recall 在 sealed intent 上的落地 | 高 / L |
| 确定性加性相关度打分器（无 embedding） | absent | `intentspec/scope.rs`（仅文件重叠原语） | 净新增、自包含、可测 | 中 / M |
| 检索态分类（current/superseded/abandoned/stale） | absent | 无 | 需文件 churn 索引 + supersede 边 | 中 / M |
| 高危继承约束在 seal 时浮现 + 确认 | absent | `intentspec/types.rs`（约束仅 per-task） | Hub「改此文件前先读」的承重面 | 中 / L |
| phase-1 确定性指纹重叠冲突检测（加权 Jaccard） | absent | `scope.rs`（仅文件维度） | 依赖 sealed fingerprint 存在 | 中 / L |
| phase-2 agent 语义冲突判定 | absent | 无 | 需 phase-1 在先；可复用 orchestrator | 低 / L |
| 语义指纹（files/subsystems/arch/behavioral/api/tags） | partial | `types.rs`（touchHints/inScope）；`scope.rs` | 有路径 hint，无多维指纹；seal 时挂指纹是冲突+检索共同前置 | 中 / M |
| coverage 分类（covered/skipped/uncovered） | absent | 无（checkpoint 记 parent_commit 但无 rollup） | 硬依赖 pin | 中 / M |
| gaps + 可逆序救援建议 | absent | 无 | coverage 存在后便宜 | 低 / S |

### 3.4 协作 / hooks / Hub-eval / CLI

| mainline 能力 | Libra 状态 | Libra 锚点 | 差距与代价 | 价值/量 |
|---|---|---|---|---|
| 多 actor 身份 + fork 信任导入（author-seal-only） | absent | `agent/push.rs`（单属主）；`publish/ai_export.rs`（单向托管） | 最大工作量、与现模型最不契合；殿后 | 中 / XL |
| team digest 滚动汇总 | absent | 无 | 依赖多 actor 合并视图 | 低 / S |
| hook 作上下文提供者：会话开始注入意图/团队快照 | partial | `hooks/runtime.rs`（只捕获，不回注） | 方向相反；加 SessionStart 只读注入，高价值、增量 | 高 / L |
| skill 分发 + AGENTS.md 托管块 | absent | `internal/ai/skills/`（内部 loader，非外部分发） | 仅在采用 provider 模型时需要 | 低 / M |
| 多 agent hook 适配（Claude/Codex/Cursor/Pi） | partial | `hooks/runtime.rs`（claude+gemini 已接，CHECK 容 7 种）；`agent.md` roster | 增量适配；agent.md 已跟踪 roster | 低 / M |
| 静态 HTML Hub / 意图读视图 + 团队健康 | partial | `publish/ai_export.rs`（单向）；`code.md` C4（/api/code/* observe-only） | 复用 web 基建；**必须建在 C4 /api/code/***，不用 web-api.md /api/v1 | 中 / L |
| webhook 领域事件扇出 | absent | 无 | 净新增、可后置 | 低 / M |
| eval 证明意图记忆可度量价值 | absent | 无（有 L1/L2/L3 分层但无意图价值 eval） | 值得借来立项 + 守检索质量 | 中 / M |
| 统一 JSON envelope + 新鲜度门控 auto-sync | partial | `utils/output.rs`（--json/--machine）；无 auto-sync | 无 team-sync 可门控；传输落地后才相关 | 低 / S |
| doctor / migrate-notes 修复 + 信任诊断 | partial | `agent/`（doctor/clean 已有）；无 notes-migrate | pin + 共享视图存在后再做 | 低 / M |

**兜底声明（no silent caps）**：本矩阵按类别归并了 32 行逐能力对照，未丢弃任何能力；被降优先级的（migrate-notes、phase-2、digest、webhook、skill 分发、多 agent roster 扩展）在上表均保留为独立行并标注了「后置/依赖」，不得被理解为「已覆盖」。

---

## 4. Libra 已具备的可复用机件（避免重复造轮子）

落地前必须先认清 Libra **已有**什么，任务卡一律优先复用：

| 机件 | 源码锚点 | 复用于 |
|---|---|---|
| AI 对象内容寻址 + 孤儿分支 `libra/intent` + CAS 追加 | `src/internal/ai/history.rs`（`AI_REF`、`create_append_commit`、`update_ref_if_matches`） | ML-01 传输、ML-02 pin 事件、ML-03 Decision |
| `refs/libra/traces` 传输范式（refspec + force-with-lease + tracking tip） | `src/command/agent/push.rs:29-40` | ML-01 传输 |
| SQLite 投影事务化重建 | `src/internal/ai/projection/rebuild.rs:135`、`resolver.rs`、`scheduler.rs` | ML-01 fetch 后重投影、各读模型 |
| IntentSpec 规范化/草稿/校验/评审/scope | `src/internal/ai/intentspec/{canonical,draft,validator,review,scope}.rs` | ML-02 seal、ML-04 指纹（scope 文件维） |
| git-internal Intent/Decision 对象 + MCP create | `src/internal/ai/mcp/`、`intentspec/persistence.rs`、`workflow_objects.rs` | ML-02/ML-03 |
| 外部 agent 捕获 + redaction | `src/internal/ai/hooks/runtime.rs` | ML-08 注入（复用 Redactor） |
| 嵌入式 Next.js + 单向 publish 导出 + C4 observe-only API | `src/internal/publish/ai_export.rs`、`src/internal/ai/web/`（C4 /api/code/*） | ML-09 Hub |
| 稳定错误码 + `--json/--machine` 输出 | `src/utils/{error,output}.rs` | 全部命令 |

> 关键取舍已在 §5 定论：**git notes 不复用于 pin**。Libra 的 notes 是「blob 存对象库、(notes_ref,object)→blob 映射存 SQLite `notes` 表」（`src/internal/notes.rs:3-4`），且无 NOTES_MERGE worktree（`notes.rs:101`），不按 git 标准 notes-tree 往返；`ConfigKind` 仅 Branch/Tag/Head（`src/internal/model/reference.rs:37`，无 Note/Intent 种类）。

---

## 5. 核心设计决策：git-native「流转」走 traces/history.rs 范式，不走 git-notes

这是本计划的拱心石决策，所有存储类任务卡都以它为前提。

**中心问题**：Libra 是否应把意图 / 决策记录从「SQLite 本地」变为「git-native 团队可迁移」？

**结论：是——但通过 Libra 自己的 `refs/libra/traces` + `history.rs` 提交链范式实现，而非移植 mainline 的 `refs/notes/mainline/*` 机制。**

理由（均经实测）：

1. **mainline 的 notes 机制在 Libra 里有硬摩擦**：Libra 的 ref 存于 SQLite（`ConfigKind` 仅 Branch/Tag/Head，无「Note」种类，`reference.rs:37`），没有 git 客户端可见的磁盘 `refs/notes/*` fanout tree；Libra notes 的映射在 SQLite 侧表、且做的是非 git 的 2-way 行合并（`notes.rs:3,101`）。把 pin 建在 notes 上会立刻撞上这层摩擦，且无法与外部 git 客户端标准 notes 互操作。
2. **Libra 已有等价且更契合的机件**：`refs/libra/traces` 是可用的提交链结构化存储（`history.rs` 的 write-tree / CAS），且**已经**经 `libra agent push` 以 `--force-with-lease` 流转。`libra/intent` 上的 AI 对象本就内容寻址、「可经同一协议传输」（`history.rs:72`）。
3. **因此**：把 pin 实现为 `libra/intent` 上的**结构化 `Pin`/`SealedEvent` 事件对象**（复用 `history.rs`），squash/rebase 抗性用「同树 fan-out」在 pin writer 里显式处理（对应 mainline `merge.go` 的语义，但不继承 notes 引擎）；可选再向 code commit 写一个 `Libra-Intent:` trailer，仅为纯 git 可见性。

**边界与继承的约束**：任何被推送的意图/决策平面都继承 Libra 现有的**合规实现面延期**——擦除 / 保留期 / GC 被 `plan.md` 归入 **Task A8.5**（`plan.md` 第 63/80 行：audit 表 / `--allow-raw` / retention / GC / erasure，明确「不得降级为纯文档验收」），且本地删除不保证传播到云端 durable tier（`object_index` → D1/R2，见 `agent.md` 关于 disable 不删除已捕获数据的约束）；本计划不得对可迁移意图平面宣称比现有 traces 平面更强的擦除保证。此外，Libra 现有的声明式 provenance/seal 字段（`types.rs` 的 `embedIntentSpecDigest`/`requireSlsaProvenance`/`transparencyLog`）是**策略位，无人计算摘要/签名**；唯一真正落地的完整性原语是内容寻址对象 id。ML-02 若要宣称「sealed / 防篡改」，必须落地真实的 digest/seal 步骤（当前缺失）。

---

## 6. 改进任务分解（ML-01 ~ ML-12）

每张卡格式：目标 / 范围（触达文件）/ 依赖 / 迁移与兼容 / 稳定错误码 / 验收与测试 / 风险。优先级 P0=可迁移脊梁，P1=价值层，P2=协作与外围。

### ML-01（P0）打通 `libra/intent` 团队传输（对齐 traces 范式）

- **目标**：让 `libra/intent`（`AI_REF`）像 `refs/libra/traces` 一样可 push/fetch，使意图/决策对象随仓库流转；SQLite 投影退化为纯可重建缓存，fetch/merge 后重投影。
- **范围**：新增 AI_REF 的推送路径，仿 `src/command/agent/push.rs`（refspec `libra/intent:refs/libra/intent`，`--force-with-lease` 对齐已记录的 tracking tip）；复用 `src/internal/ai/history.rs` 追加/CAS；在 `src/command/fetch.rs` 增加接收侧命名空间持久化（现仅路由 heads/tags/mr）；fetch 后触发 `src/internal/ai/projection/rebuild.rs` 重投影。附带小增强：给投影记 built-from 水位线（`libra/intent` head commit），使「视图是否新鲜」无需全量重建。
- **依赖**：无（traces 传输 + history.rs 机件已在）。
- **迁移与兼容**：新增 ref 命名空间为增量；接收侧对未知 `refs/libra/*` 采默认忽略→显式持久化，需声明 schema/protocol 窗口。存量单机仓库无 `libra/intent` 远端时命令须 no-op 且明确提示。
- **稳定错误码**：新增 `LBR-INTENT-00x`（远端拒绝 / lease 失配 / 非快进发散），登记 `docs/error-codes.md`（`compat_error_codes_doc_sync` 守卫）。
- **验收与测试**：新增 `tests/ai_intent_transport_test.rs`：两仓库间 push→fetch→重投影往返、force-with-lease 拒绝非快进、投影从 fetch 后的 head 重建一致；`rg -n "libra/intent" src/command/push.rs src/command/fetch.rs` 应从「无」变「有」。三门禁（fmt/clippy/`cargo test --all`）全绿。
- **风险**：不得与 `code.md` C6 的 MCP 传输、plan.md A8.5 擦除/GC 合规面延期冲突；多 actor 合流不在本卡（ML-10）。

### ML-02（P0）seal + intent↔commit pin（关闭 `commit_sha=None`）

- **目标**：新增 seal 转移——在 commit/PR 边界冻结 IntentSpec + summary + fingerprint，并写一个把它绑定到 code commit SHA + tree hash 的 `Pin`；这是 mainline 单点最高价值能力。
- **范围**：扩展 `src/internal/ai/intentspec/persistence.rs` 在 seal 时回填 `commit_sha`；在 `AI_REF` 上经 `history.rs` 新增 `Pin`/`SealedEvent` 结构化对象（**非 git note**）；pin writer 内实现「同树 fan-out」抗 squash/rebase（对应 mainline `merge.go` 语义）；可选在 `src/command/commit.rs` 写 `Libra-Intent:` trailer 供纯 git 可见。fingerprint 文件维复用 `intentspec/scope.rs::effective_write_scope`。落地真实 digest/seal 步骤（见 §5 边界）。
- **依赖**：ML-01（使 pin 随仓库流转）。
- **迁移与兼容**：存量以 `commit_sha=None` 持久化的 IntentSpec 需可读且可被 backfill（见 ML-06 backfill）；trailer 为增量、不改 commit 语义。
- **稳定错误码**：`LBR-INTENT-01x`（seal 于脏工作树未 --allow-dirty / pin 目标 commit 不存在 / 同 commit 冲突意图）。
- **验收与测试**：`tests/ai_intent_seal_pin_test.rs`：seal 冻结后 summary/fingerprint 不可变、pin 绑定 commit+tree、squash 后同树 fan-out 仍命中、trailer 可解析；`persistence.rs` 不再出现 `commit_sha: None` 的完成态路径。
- **风险**：seal 触发时机要清晰（commit / PR 边界，类比 mainline `seal --submit`）；「防篡改」宣称必须有真实 seal 步骤支撑。

### ML-03（P0）决策一等化：结构化「被否方案 + 理由」

- **目标**：把「权衡了哪些备选 / 为何选 X 弃 Y」提升为 git-internal Decision 对象的一等字段，随意图平面 seal/pin/流转。
- **范围**：扩展 **live 平面**的 git-internal `Decision` 对象 + orchestrator/`persistence.rs`，携带 `alternatives[] { option, rationale, rejected_reason }`；在 `ai_final_decision` 旁加投影列/表供查询；在 seal/pin 中随 commit 流转。**严禁**建在 `agent_run`（schema-only、CP-4 门控、`subagent-scaffold` feature 后）。
- **依赖**：与 ML-02 配套；独立于 ML-01。
- **迁移与兼容**：`ai_final_decision.summary_json` 保留，新增结构化字段为叠加；投影列走 `sql/migrations/` 幂等前向 + `_down.sql`。
- **稳定错误码**：复用 seal 路径错误码；无新增网络码。
- **验收与测试**：`tests/ai_decision_alternatives_test.rs`：可写入/查询 rejected 备选与理由、随 pin 流转、投影可重建；`rg -n "alternatives" src/internal/ai/agent_run` 应仍为空（未误建在 scaffold）。
- **风险**：勿触碰 CP-4 门控的 scaffold；决策来源需标注 provenance。

### ML-04（P1）语义指纹 + phase-1 确定性冲突检测

- **目标**：sealed intent 带多维指纹后，在 seal/sync 时用加权 Jaccard 对「并发竞争工作」出粗粒度告警（screen），phase-2 语义评审留作后续。
- **范围**：给 sealed IntentSpec 加 fingerprint 结构（files/subsystems 从 `scope.rs` 派生；behavioral/api 尽力而为）；新增加权 Jaccard 打分器，在 seal 时对 proposed + base 之后 merged 的意图跑重叠；置于 `src/internal/ai/intentspec/` 或新 conflict 兄弟模块。
- **依赖**：ML-02（seal/fingerprint）。
- **验收与测试**：`tests/ai_intent_conflict_phase1_test.rs`：指纹重叠命中/阈值/假阳性调参；确定性、无 embedding。
- **风险**：假阳性调参；behavioral/subsystems 派生的可靠性是难点。

### ML-05（P1）改动前意图检索（context modes + 相关度打分器）

- **目标**：给 agent「编辑前先读相关 sealed intent/decision」的读面——确定性（无 embedding）加性打分器 + 检索态叠加（current/superseded/abandoned/stale）。这是 mainline 量化价值集中处（eval CF-IF delta）。作为 `memory.md` recall 在 sealed intent 上的**具体落地**，而非平行系统。
- **范围**：在投影 + sealed intent 上建读面（query/files/current 三模）；打分器基于 `scope.rs` 文件重叠 + recency + supersession lineage；检索态分类器（stale 由 age/file-churn，superseded 由 lineage）。经 CLI 与 MCP 暴露——MCP 走 `code.md` C6 的 `libra code --stdio`，**不用** `memory.md` 提议的 `libra mcp --stdio`。
- **依赖**：ML-02（有 sealed 记录可检索）；projection（has-current）；**与 memory.md recall/inject 收敛**（§9）。
- **验收与测试**：`tests/ai_intent_retrieval_test.rs`：三模检索、打分确定性可复现、检索态正确、abandoned 命中告警；MCP 面经 C6 stdio 而非 `libra mcp --stdio`。
- **风险**：依赖 seal+pin；与 memory.md 重叠必须先按 §9 定案再实现。

### ML-06（P1）coverage / gaps over main commits

- **目标**：pin 到位后回答「哪些 main commit 没有意图记录」，并给可逆序救援（backfill/skip）。
- **范围**：coverage 分类器（有 pin=covered；`Libra-Skip:` trailer/config=skipped；否则 uncovered）+ `libra intent gaps` 命令；复用 `agent_checkpoint.parent_commit` 约定；附带 backfill（`--commits/--range` 认领既有 commit，对应 ML-02 存量回填）。
- **依赖**：ML-02（pin）。
- **验收与测试**：`tests/ai_intent_coverage_test.rs`：covered/skipped/uncovered 三态、gaps 列表、backfill 认领后转 covered。
- **风险**：硬依赖 pin。

### ML-07（P1）与 memory.md 收敛，确立 mainline.md 为该轴受纳计划

- **目标**：解决 §9 五轴重叠，避免第三条平行平面。把 ML-01~ML-06 表述为 `memory.md` Phase A（可审计存储）+ Phase B/C（recall/inject）在「意图-pin 轴」的具体优先落地，本文成为该轴 committed schedule；`memory.md` owner 随后对齐其枚举（A4 已加 SubagentStart/End=13 变体，`memory.md` 的 11 变体断言过时）、MCP 传输（C6 `libra code --stdio`）、web 契约（C4 /api/code/*）。
- **范围**：文档级——本文引用 agent.md/code.md 为唯一设计权威；对五轴逐条给「subsume（收纳）/ defer（各自保留）」决策；标注 memory.md 欠对齐的枚举/MCP/web 项。
- **依赖**：无（是 ML-05/ML-08/ML-09 的治理前置）。
- **验收**：§9 决策表落定；`memory.md` 头部补 out-of-scope banner（由其 owner 执行，本文只声明冲突项）。

### ML-08（P2）hook 上下文注入：会话开始向外部 agent 注入意图快照

- **目标**：给 `hooks/runtime.rs` SessionStart 增加「渲染并发出上下文块」（脱敏后的意图/决策/约束快照），把 mainline eval 证明有效的「写前先读」上下文给到外部 agent。方向与现有「只捕获」相反，是净新增。
- **范围**：扩展 `src/internal/ai/hooks/runtime.rs` 的 SessionStart 渲染（类比 mainline dispatcher 的 RenderSessionStartContext）；**严守只捕获边界**——注入只读、绝不 mint 意图；内容过默认 Redactor；尊重 A4 的 13 生命周期变体。
- **依赖**：ML-05（选注入什么）；与 memory.md `with_memory` 注入收敛（§9）。
- **验收与测试**：`tests/ai_hook_context_injection_test.rs`：SessionStart 注入只读、脱敏、不写任何意图对象、13 变体对齐。
- **风险**：与 memory.md §8.5 `with_memory` 重叠；不得模糊捕获/注入边界。

### ML-09（P2）Hub / web 意图读视图 + digest（建在 C4 /api/code/*）

- **目标**：把可迁移意图平面变成共享只读读面（按文件约束、决策、coverage、关系、team digest），即 mainline 的 Hub。
- **范围**：sealed intent/pin/decision 的读模型 + 页面；扩展 C4 的 `/api/code/*` 路由（`src/internal/ai/web/`）与嵌入式前端；digest 汇总命令；可选扩展 `publish/ai_export` 做静态导出。**必须建在 C4 observe-only /api/code/***，**不采用** web-api.md 的 mutating `/api/v1`（plan.md 冲突）。
- **依赖**：ML-02（pin）、ML-05（检索/打分）。
- **验收与测试**：`tests/ai_intent_hub_test.rs` + C4 wire 测试扩展；确认无 `/api/v1` 引入。
- **风险**：必须复用 C4 契约，不得引入 web-api.md /api/v1。

### ML-10（P2）多 actor 合流 + fork 信任导入（殿后）

- **目标**：mainline 的 per-actor 日志 + fork 导入（author-seal-only 信任边界）；与 Libra 现「单属主 per-ref traces（first-writer-wins）」最不契合，工作量最大，放最后。
- **范围**：把单属主 traces 模型扩展为 `libra/intent` 上的 per-actor 日志命名空间；跨 actor 的 merge/view-fold（类比 `projection/rebuild.rs` 但跨 actor）；导入命令带 author-seal-only 校验边界。触达面大（history.rs / protocol / projection）。
- **依赖**：ML-01、ML-02（传输 + sealed 记录）。
- **验收与测试**：`tests/ai_intent_multiactor_test.rs`：多 actor 日志合并成一致视图、fork 导入只接受 author-seal、拒绝 fork 侧约束/风险提权。
- **风险**：需扩展单属主模型；面大，务必在单仓库脊梁（ML-01~06）验证后再动。

### ML-11（P2）eval 谐架：证明并守护意图记忆价值

- **目标**：借 mainline 的方向性证据谐架（fixtures + 检索前置 + LLM-as-judge + CF-IF delta）立项并回归守护检索质量。契合 Libra L1/L2/L3 分层。
- **范围**：fixture 目录 + 基于 ML-05 检索的确定性检索前置打分器 + 可选 LLM-as-judge（L3，`test-live-ai` 门控）；新 `tests/` 区。
- **依赖**：ML-05（检索面）。
- **验收**：确定性部分 L1 可跑；live-judge 部分缺 key 打印 skip 不失败（`env_var_is_set` 模式）。

### ML-12（P2，可选）持久信号 constraint/risk/followup 生命周期

- **目标**：把 IntentSpec 的静态 per-task 约束升级为团队共享、带 open/resolved/expired 生命周期的信号队列，含人类确认的 guard 层与按文件重叠的继承。
- **范围**：在意图平面加信号事件（constraint/risk/followup）+ 生命周期状态机 + seal 时高危继承约束浮现（配合 ML-05 检索）。
- **依赖**：ML-02、ML-05；**与 memory.md `procedural.*` 规则轴重叠，先按 §9 收敛再建**。
- **风险**：最易与 memory.md 重复；未收敛前不得实现。

---

## 7. 内部执行顺序（阶段图）

```mermaid
flowchart TD
    subgraph P0["阶段 0-1 · 可迁移脊梁（P0）"]
        ML01["ML-01 libra/intent 传输"]
        ML02["ML-02 seal + pin"]
        ML03["ML-03 Decision 一等化"]
    end
    subgraph P1["阶段 2-4 · 价值层（P1）"]
        ML04["ML-04 指纹 + phase-1 冲突"]
        ML05["ML-05 编辑前检索"]
        ML06["ML-06 coverage/gaps"]
        ML07["ML-07 memory.md 收敛（治理前置）"]
    end
    subgraph P2["阶段 5-6 · 协作与外围（P2）"]
        ML08["ML-08 hook 注入"]
        ML09["ML-09 Hub/web 视图"]
        ML10["ML-10 多 actor/fork"]
        ML11["ML-11 eval"]
        ML12["ML-12 持久信号"]
    end
    ML01 --> ML02 --> ML04
    ML03 --> ML04
    ML02 --> ML05 --> ML06
    ML07 -.治理前置.-> ML05
    ML07 -.治理前置.-> ML08
    ML07 -.治理前置.-> ML12
    ML05 --> ML08
    ML02 --> ML09
    ML05 --> ML09
    ML01 --> ML10
    ML02 --> ML10
    ML05 --> ML11
    ML05 --> ML12
```

固定内部序：**(0) 传输 ML-01 →(1) seal+pin+Decision ML-02/03 →(2) 指纹/冲突 ML-04 →(3) 检索 ML-05 →(4) coverage ML-06 →(5) hook 注入 & Hub ML-08/09 →(6) 多 actor/fork & eval & 信号 ML-10/11/12**。理由：pin 依赖 seal；冲突/检索/coverage/Hub/注入都依赖「可流转的 sealed 记录」；多 actor 是最大工作量、与单属主模型最不契合，殿后。ML-07 是 ML-05/08/12 的**治理前置**（先与 memory.md 定案再动重叠轴）。

---

## 8. 相对同目录其他计划的先后顺序（跨文档 ordering）

本文严格遵守 `plan.md` §0 的执行纪律，八条约束：

1. **整体排在 plan.md 固定链之后**：mainline.md 必须排在 `plan.md` 已受纳的 `agent.md`（AG-16~AG-24a 外部捕获）**整体完成** → `code.md`（C1~C8 内部 runtime）**整体完成**之后，且**不得**与其中任一阶段交错插入。plan.md §0/§10 以代码+测试判完成，新平面不得插队或劈开在途的 Agent/Code 环。
2. **内部严格阶段序**：见 §7（0→6）。
3. **不得引入三份 out-of-scope 草稿的验收项**：mainline.md 不得从 `memory.md` / `sandbox.md` / `web-api.md` 引入验收标准或实现项；可引用的设计权威仅 `agent.md` / `code.md`（plan.md §0 明确三者为 out-of-scope draft 且有已知冲突）。
4. **与 memory.md 五轴收敛**：把 ML-01~ML-06 作为「意图-pin/recall 轴」的 committed schedule，memory.md（proposed、未排期、无 §10 schedule）对齐之，避免第三平面（§9）。
5. **任何 Hub/web 视图建在 C4**：只用 `code.md` C4 的 observe-only `/api/code/*`，**绝不**用 `web-api.md` 的 mutating `/api/v1`（plan.md 已标为冲突并留待独立仲裁）。
6. **任何意图检索 MCP 面走 C6**：走 `code.md` C6 的 `libra code --stdio`（及 `libra code-control --stdio`），**不用** `memory.md` 提议的 `libra mcp --stdio`（plan.md 记为与 C6 冲突）。
7. **hook 注入对齐 A4 的 13 变体**：ML-08 必须对齐 `agent.md` A4 落地的 13 个 LifecycleEventKind（新增 SubagentStart/SubagentEnd）；memory.md 的 11 变体断言已过时；注入严格只读、绝不 mint 意图，保持 hook 只捕获边界。
8. **继承合规面延期（Task A8.5）**：任何被推送的意图/决策平面（ML-01 起）继承 plan.md 把擦除/保留期/GC 归入 Task A8.5 的延期（plan.md 第 63/80 行），且本地擦除不保证传播到云端 durable tier（object_index→D1/R2），不得宣称比 traces 平面更强的擦除保证。

```mermaid
flowchart LR
    AG["agent.md<br/>AG-16~AG-24a（先）"] --> CO["code.md<br/>C1~C8（后）"] --> MLN["mainline.md<br/>ML-01~ML-12（本文，之后）"]
    MEM["memory.md（proposed·未排期）"] -. "五轴收敛：本文为受纳计划" .- MLN
    C4["code.md C4 /api/code/*"] -. "ML-09 建于此，非 web-api /api/v1" .-> MLN
    C6["code.md C6 libra code --stdio"] -. "ML-05 MCP 面走此，非 libra mcp --stdio" .-> MLN
    A4["agent.md A4 · 13 变体"] -. "ML-08 对齐" .-> MLN
    A85["plan.md A8.5 擦除/GC 合规面延期"] -. "ML-01 起继承" .-> MLN
```

---

## 9. 与 memory.md 的关系与五轴收敛

`memory.md` 是 Libra **自己**的近似提案：git-native `refs/libra/memory*`、`MemoryNote`/`MemoryEvent`、可重建 SQLite 投影、跨平面 `evidence_refs` 指向 git-internal Decision/Run/Evidence、branch-aware 查询、`with_memory` prompt 注入、Phase-E web 视图——与 mainline 在**五轴**重叠。它未开始、未排期（out-of-scope of plan.md，无 §10 schedule）。为避免出现「memory.md 平面 + mainline 平面 + 现有 intentspec 平面」三条平行结构，收敛决策如下（ML-07 落定）：

| 重叠轴 | mainline 视角 | memory.md 视角 | 收敛决策 |
|---|---|---|---|
| 意图记忆存储（git refs + 投影重建） | actor log + view.json | `refs/libra/memory*` + MemoryNote/Event | **共用底座**：都落在 `libra/intent` 传输（ML-01）+ projection 重建；memory 若需独立 ref 命名空间，复用 ML-01 的传输/接收机制，不另造传输 |
| 决策/证据记录 | Decision(rejected/rationale) | `evidence_refs` 指向 Decision/Run/Evidence | **本文主导**：ML-03 在 live git-internal Decision 上加结构；memory 的 evidence_refs 指向同一对象，不重定义 |
| 上下文注入 | hook 会话开始注入 | `with_memory` prompt 注入 | **合流为一条注入管线**：ML-08 SessionStart 注入 = memory `with_memory` 的落地；先按本轴定案再实现（ML-07 前置 ML-08） |
| 约束 | constraint/risk/followup 队列 | `procedural.*` 规则轴 | **先收敛后建**：ML-12 与 memory 的 procedural 规则轴重叠，未定案不实现 |
| Hub / 读视图 | 静态 HTML Hub | Phase-E web 视图 | **共用 C4**：都建在 code.md C4 `/api/code/*`；ML-09 与 memory Phase-E 合为一个读面 |

**归属**：本文成为「意图可迁移 / pin / recall / 注入 / 约束」这些轴的 **committed schedule**；`memory.md` 头部**已有** out-of-scope banner（其 §0 第 3 行已注明 11→13 变体与 `libra mcp --stdio`→C6 两处冲突），owner 另需在其正文对齐枚举描述（§ 887 行的 11 变体）、MCP 面（§ 1036 行的 `libra mcp --stdio` 改走 C6）、web 契约（走 C4）。修订 memory.md 的设计断言由其 owner 负责，不在本文范围。

---

## 10. 风险与未闭环项

| 类别 | 风险 | 当前处理 |
|---|---|---|
| 传输与云删除 | 推送意图平面后本地擦除不保证传播到 D1/R2 | 继承 plan.md 合规面延期（Task A8.5，第 63/80 行）；不宣称强擦除；ML-01 文档明记 |
| notes 误用 | 若有人用 git notes 实现 pin，会撞 ConfigKind/SQLite 侧表/无 NOTES_MERGE 摩擦 | §5 定论：pin 走 history.rs 结构化事件，**禁用** notes；ML-02 验收含「无 notes 依赖」检查 |
| 建错平面 | 在 CP-4 门控的 `agent_run` scaffold 上建决策/pin 会被 feature gate 挡住且非 live | §2/§4 明令用 orchestrator + persistence.rs + git-internal 对象；ML-03 验收含「scaffold 无 alternatives」反向检查 |
| seal 语义 | 无真实 digest/签名却宣称「防篡改」 | ML-02 必须落地真实 digest/seal；否则只宣称「内容寻址完整性」 |
| memory.md 重复 | 不收敛会造第三平面 | ML-07 治理前置，五轴决策先落定（§9） |
| 多 actor 契合度 | 现单属主 first-writer-wins（runtime.rs:650）与多 actor 合流不契合 | ML-10 殿后，单仓库脊梁验证后再动 |
| 冲突假阳性 | phase-1 指纹重叠误报干扰 agent | ML-04 定为粗筛 screen；阈值可调；phase-2 语义评审后置 |
| ref 命名一致性 | `reference.rs:13` 文档注释列出「Intent」种类，但 `ConfigKind` 枚举（:37）实为 Branch/Tag/Head——注释与实现漂移；规范存法为 `AI_REF="libra/intent"`、kind='Branch' | 实现以 `history.rs::AI_REF` 为唯一事实源；ML-01 落地时顺手订正 `reference.rs:13` 注释漂移 |
| 历史重写悬挂 pin | rebase/filter 后 pin 悬挂 | migrate-notes 类重键路径列为后置（pin 落地并观察到重写后再做） |

---

## 11. 源码事实索引（撰写时实测核对）

以下锚点为本文承重结论的证据，实现推进前必须重新核对：

- `src/internal/ai/history.rs:72` — `AI_REF = "libra/intent"`，所有 AI 对象（Intent/Task/Run/Plan/PatchSet/Evidence/ToolInvocation/Provenance/Decision）挂此孤儿分支，以 kind='Branch'、name='libra/intent' 存于 reference 表，CAS 追加、GC-root。
- `src/command/{push,fetch,clone}.rs` — 实测**无** `libra/intent`；`clone.rs:3597` 刻意不写 `+refs/*:refs/*` → 意图平面本地私有。
- `src/command/agent/push.rs:29-36` — traces 传输范式：`refspec="{TRACES_BRANCH}:refs/libra/traces"` + `--force-with-lease`（ML-01 直接对齐）。
- `src/internal/ai/intentspec/persistence.rs:54` — `commit_sha: None, // Will be set when completed`（pin 缺口）。
- `src/internal/ai/agent_run/mod.rs:5,13` — 「schema-only … gated on CP-4」，类型仅 Serialize/Deserialize，藏在 `subagent-scaffold` feature 后（禁止在其上建）。
- `src/internal/model/reference.rs:37` — `ConfigKind` 仅 Branch/Tag/Head（无 Note/Intent 种类）。
- `src/internal/notes.rs:3-4,101` — notes blob 在对象库、(notes_ref,object)→blob 映射在 SQLite `notes` 表、无 NOTES_MERGE worktree、2-way 行合并（notes 不用于 pin 的依据）。
- `src/internal/ai/projection/rebuild.rs:135` — 事务化销毁重建投影（log→view 已等价，has-current）。
- `src/internal/ai/agent_run/decision.rs`、`src/internal/model/ai_final_decision.rs` — 决策仅单 verdict / 不透明 summary_json（ML-03 依据）。
- `src/internal/ai/hooks/runtime.rs`（`ingest_agent_traces_payload`）— hook 只捕获、不回注（ML-08 是反方向净新增）。
- `src/internal/ai/intentspec/scope.rs::effective_write_scope` — 文件重叠原语（ML-02 指纹文件维 / ML-04 Jaccard 复用）。
- 竞品事实源：`/Volumes/Data/competition/mainline/mainline` 的 `docs/specs/{intent-record-v0,agent-context-protocol-v0}.md`、`docs/reference.md`、`internal/engine/{seal,pin,notes,conflict,coverage,context_retrieval}.go`、`internal/cli/`、`internal/hub|webhook|eval/`（对标源，非 Libra 代码）。

> 说明：本文所依据的是 13 路并行源码复核 + 综合分析，其中「意图生命周期」一路 agent 中途 stalled，但该子系统已由 `docs/specs/intent-record-v0.md` 与 `agent-context-protocol-v0.md` 直读补全，不影响结论完整性。

---

## 12. 维护要求

- 改进本轴前，必须先阅读并遵循 [`docs/development/commands/_general.md`](../commands/_general.md) 与 [`plan.md`](plan.md) §0 执行纪律。
- 本文是 out-of-scope draft：任何实现前，先按 §8/§9 确认它未插队 plan.md 固定链、未从 memory/sandbox/web-api 引入验收项、未违反 C4/C6/A4 约束与 A8.5 合规面延期。
- 任何触达 public JSON、DB row、object layout、RPC、hook envelope、ref 命名空间的 PR，必须声明：变更边界、未触碰项、schema/protocol version 与 compat 窗口、migration/backfill 状态、用户可见行为、稳定错误码、回滚方式、测试命令。
- 落地判定以**代码 + 测试**为准，三门禁必须本地全绿后才算完成：`cargo +nightly fmt --all --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`source .env.test && cargo test --all`（L2/L3 缺 key 打印 skip 不失败）。
- 新增命令/flag/JSON 字段/MCP tool/ref 命名空间/稳定错误码时，同步 `COMPATIBILITY.md`、`docs/commands/*`（含 zh-CN）、`docs/error-codes.md`、`tests/INDEX.md`，并把新测试 target 注册进 `Cargo.toml`。
- 本文源码锚点会随代码演进漂移；引用任何锚点前先 `rg` 复核，发现漂移先更新本文再推进（**代码为唯一事实源**）。
