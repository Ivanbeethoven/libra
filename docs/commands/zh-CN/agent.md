# `libra agent`

管理 Claude Code、Codex 与 OpenCode 的外部代理捕获。

## 概要

```bash
libra agent status
libra agent list [--schema-version <1|2>] [--json]
libra agent import (--session <id> | --path <path> | --since <rfc3339> | --all) [--agent <name>] [--limit <n>] [--cursor <n>] --yes
libra agent enable [--agent <name>]...
libra agent add [<name>...]
libra agent disable [--agent <name>]...
libra agent remove [<name>...]
libra agent session <subcommand>
libra agent checkpoint <subcommand>
libra agent skill <subcommand>
libra agent clean [--all]
libra agent doctor [--repair]
libra agent push [--remote <name>] [--force-rewrite]
libra agent rpc <subcommand>
```

## 说明

`libra agent` 管理 Libra 的外部代理捕获表面。它安装和移除提供商 hook，报告已捕获的 session/checkpoint 状态，暴露只读诊断，并可将 `refs/libra/traces` 推送到远程。

支持的 roster 为 `claude-code`、`codex`、`opencode`（首批），三者均可安装 hook：`claude-code` 写 `.claude/settings.json`；`codex` 写用户级 `$CODEX_HOME/hooks.json` 并在 `$CODEX_HOME/config.toml` 写入 Libra 托管的 trust 条目（未受信的 Codex hook 会被静默跳过，trust 条目是安装的一部分）；`opencode` 写 Libra 托管插件 `.opencode/plugin/libra-hooks.js`（注意：`opencode --pure` 会禁用包括捕获在内的全部外部插件）。`gemini` 已从支持 roster 降级为仅卸载通道：`libra agent remove gemini` 可移除历史安装的 Libra 托管 hook（幂等），已捕获会话保持可读；对它或其它非 roster 代理执行 `add`/`enable` 会返回可操作的 unsupported 错误。

## 子命令

| 子命令 | 说明 |
|------------|-------------|
| `status` | 报告已捕获的外部代理会话状态 |
| `list` | 列出受支持代理的能力矩阵（roster、hook、安装状态） |
| `import` | 经明确同意后，发现并导入历史 Claude/Codex transcript 文件，或导入一次受信、沙箱化的 OpenCode export |
| `enable` | 启用一个或多个外部代理并安装 hook |
| `add` | `enable` 的别名：`add <name>` ≡ `enable --agent <name>` |
| `disable` | 禁用一个或多个外部代理并卸载 hook |
| `remove` | `disable` 的别名：`remove <name>` ≡ `disable --agent <name>` |
| `session list` | 列出已捕获会话 |
| `session show <id>` | 显示一个已捕获会话 |
| `session stop <id>` | 将已捕获会话标记为 stopped |
| `session resume <id>` | 将已停止的已捕获会话重新标记为 active |
| `session promote <id>` | 将已捕获会话提升为 Libra intent 元数据 |
| `session derive-tool-calls <id>` | 从已捕获会话推导工具调用记录 |
| `checkpoint list` | 列出已捕获 checkpoint |
| `checkpoint show <id>` | 显示 checkpoint 元数据 |
| `checkpoint rewind <id>` | 检查或应用某个 checkpoint 的工作树回退 |
| `checkpoint export <id>` | 导出 checkpoint transcript：默认脱敏（无需授权）；raw（未脱敏）导出须 `--allow-raw --raw` 并写入 append-only `agent_audit_log`（缺失授权时拒绝并返回 `LBR-AGENT-013`） |
| `skill search` | 按 `--skill`、`--provider`、`--session`、RFC3339 `--since`/`--until` 搜索捕获的 skill events（`--limit`/`--cursor` keyset 分页、`--json`）。基于 checkpoint metadata 的读时投影，无独立表 |
| `skill list` | `skill search` 的别名（同过滤项） |
| `skill registry` | 展示各 agent 的 curated 可发现 skill 注册表（`--provider <slug>` 限定；公开 SkillDiscoverer 面） |
| `clean` | 清理已停止会话的临时 checkpoint（prune 遇到进行中的 checkpoint 写入或 traces 引用可达但无 catalog 行的提交时 fail-closed 拒绝；同时删除因此不可达的 `object_index` 行） |
| `doctor` | 诊断 hook 安装和捕获状态；检测（`--repair` 时修复）checkpoint 存储不一致 |
| `push` | 将 `refs/libra/traces` 推送到远程（`clean` prune 重写后的非快进推送用 `--force-rewrite`，采用 force-with-lease 语义） |
| `rpc list` | 列出 `PATH` 上发现的 `libra-agent-*` 二进制（含 trusted/quarantined 状态）；需先开启 external-agents 开关 |
| `rpc trust <slug>` | 信任一个已发现的二进制——记录 path + sha256 + device/inode/mtime 来源（所在目录 world-writable、或二进制不在受信目录下时拒绝——`LBR-AGENT-005`） |
| `rpc trust --dir <path>` | 注册一个受信目录（`agent.external_agents.trusted_dirs`，默认 `~/.libra/agents`）：外部二进制的 canonical path 必须位于其中之一才可被信任。路径会被 canonicalize，且必须是存在且非 world-writable 的目录 |
| `rpc untrust <slug>` | 撤销信任；二进制回到隔离状态（始终可用，不受开关限制） |
| `rpc invoke` | 在**已信任**的 `libra-agent-*` 二进制上调用一个 JSON-RPC 方法 |

## 常用选项

| 标志 | 子命令 | 说明 |
|------|------------|-------------|
| `--agent <name>` | `enable`, `disable` | 选择代理名称；省略时针对支持 roster（`add`/`remove` 以位置参数接收名称） |
| `--schema-version <1\|2>` | `list` | 选择 machine schema；版本 1 保持冻结旧行结构，版本 2 增加 `transcript_discoverable`、`importable`、`export_bridge` 的 `methods[]` 可用性 |
| `--session <id>` / `--path <path>` / `--since <rfc3339>` / `--all` | `import` | 四选一的历史导入范围；`--path` 还必须指定 `--agent`，OpenCode 通过 export bridge 支持显式 `--session` |
| `--yes` | `import` | JSON/非 TTY 导入必需；确认 Libra 可以读取私有 provider session 内容、脱敏并把类型化投影写入当前仓库 |
| `--restore-erased` | `import` | 显式移除本地防复活 tombstone 后重试；要求 `--yes`，并追加审计记录 |
| `--limit <n>` / `--cursor <n>` | `import` | 有界发现分页（默认 20、硬上限 100）以及上一页返回的下一个零基游标；单次调用还有 64 MiB 累计原始输入上限。每源有效 cap 为 `min(agent.max_transcript_read_bytes, 16 MiB adapter hard cap)`；显式配置更大值时会诊断实际有效 cap |
| `--limit <n>` | `session list`, `checkpoint list` | 每页最大行数（默认 50，硬上限 500——超过时钳制并在 stderr 提示；`0` 按 `1` 处理） |
| `--cursor <cursor>` | `session list`, `checkpoint list` | 上一页 `next_cursor` 返回的不透明 keyset 游标；不要手工构造 |
| `--extract-transcript <path>` | `session show` | 将会话元数据中的已捕获 transcript 路径复制到本地文件 |
| `--all` | `clean` | 清理所有已停止会话的 checkpoint，而不只是最近一个 |
| `--gc` / `--retention-days <n>` / `--dry-run` | `clean` | 三窗口保留期 GC：(1) 删除已停止会话中早于 `agent.retention.transcript_days`（默认 90；用 `--retention-days` 覆盖）的 checkpoint；(2) 清理早于 `agent.retention.stderr_days`（默认 30）的**终态** run 的 reviewer stderr 诊断日志，保留聚合记录；(3) **A0-09** 删除早于 `agent.retention.findings_days`（默认 90）的**终态** review/investigate run 整个目录（`findings.md`/`manifest.json`/`state.json`/reviewer 日志）。对象化的 findings blob 是内容寻址对象，交由未来的仓库级 object GC 回收（per-run retention 绝不删除可能被共享的对象）。non-terminal/时间戳不可解析的 run 一律 fail-safe 跳过；永不触碰 `agent_audit_log`。`--dry-run` 仅预览各窗口及配套清理的 would-be 删除（包括 JSON `findings_runs_pruned`/`import_identities_pruned`），不实际删除 |
| `--repair` | `doctor` | 修复检测到的 checkpoint 存储不一致（重建 catalog 行、补插 `object_index` 行、安全 drain 有效的过期普通 writer marker，并忽略普通 TTL 立即 drain `cleanup_pending` marker）；损坏 marker 保持 `manual_required`；省略时仅检测 |
| `--remote <name>` | `push` | 选择用于推送代理 trace 引用的远程 |
| `--force-rewrite` | `push` | 允许本地 `clean` prune 之后的非快进推送（traces 引用由 Libra 托管，prune 即整链重写）；采用针对本仓库最近一次推送记录的 force-with-lease 语义——绝非无条件 force——远程被别处重写时仍 fail-closed 拒绝 |
| `--dry-run` | `checkpoint rewind` | 显示影响而不修改文件；这是默认值 |
| `--apply` | `checkpoint rewind` | 恢复所选 checkpoint 的工作树 |

## JSON 输出

支持结构化输出的子命令使用全局 `--json` 和 `--machine` 信封。例如：

```bash
libra --json agent status
libra --json agent list
libra --json agent checkpoint list
libra --json agent rpc list
```

`agent list --json` 携带稳定的 `schema_version`，并为每个受支持代理输出一行——首批 roster `claude-code`、`codex`、`opencode`。非首批代理（`gemini`、`cursor`、`copilot`、`factory-ai`）仍保留在注册表中以保证历史会话可读，但不会出现在该列表里。每行携带 `slug`、`agent_kind`、`stability`、`supported`、`support_wave`、`registered`、`transcript_readable`、`hook_installable`、`installed`、`launchable_review`、`launchable_investigate`、`external_binary`、`config_paths`、`protected_dirs`、`capabilities`。行结构是面向自动化的冻结契约。Claude Code 会声明 `capabilities.transcript_preparer=true`：Libra 先安全打开并 pin 已授权 descriptor，再只通过同一 descriptor 短暂等待末尾 JSONL 记录完成 flush；等待与 tail probe 均有界，preparer 不会按路径重新打开文件。

只有调用方理解扩展时才请求 `agent list --schema-version 2 --json`。其
`methods[]` 报告 transcript 发现、历史导入与 OpenCode export bridge 的
支持及当前可用状态；默认版本 1 的 payload 保持原有字段集合，不会隐式
增加扩展字段。OpenCode 的 `transcript_discoverable` 明确为 unsupported（不支持
批量发现）；显式 ID 的 `importable`/`export_bridge` 可用性取决于受信任离线
exporter 与 sandbox。不支持的 schema version 以 usage 错误（exit 129、category
`cli`）返回 `LBR-AGENT-017`。当平台无法提供 Libra 的 provider-root 安全打开
原语时，Claude/Codex 的发现与导入会如实报告为 unavailable。

`agent import` 返回 schema version 1 的 `results`、`skipped`、`partial_results`、
`failures` 与 `next_cursor`。每项状态固定为 `imported`、`noop`、`partial`、
`skipped` 或 `failed`。自动发现的跨仓库/已擦除候选进入 `skipped`，session id
哈希脱敏并携带稳定 reason code；显式 selector 遇到同类条件仍是失败。
`results` 仅含完整成功项；发生耐久 turn 进度但最终
失败的 selection 进入 `partial_results`，不会计入 `succeeded`。批次部分成功时以 `LBR-AGENT-018` 非零退出；结构化错误详情
保留成功摘要及逐项失败，失败 session id 使用哈希脱敏。单项归属、cwd、
已擦除与源授权失败分别保留 `LBR-AGENT-015`、`016`、`019`、`020`；失败详情
仍携带 `schema_version`，并原样保留 nullable `next_cursor`，不会把 `null` 改成 0。

历史导入严格限定当前仓库且 fail-closed：transcript 必须给出唯一无歧义的
`cwd`，Libra 解析其 storage 并要求与当前仓库相同。共享该 canonical storage
的 sibling linked worktree 属于同一仓库；其它仓库不属于。文件源必须位于所选
provider 的 protected root；Unix 使用 descriptor-relative no-follow 单次
打开。provider root 按 component 逐段安全打开；Claude 每个 source 与 Codex 的
year/month/day 各层都在同意前相对 pinned 目录 descriptor 打开，因此 root、嵌套
目录或 source-file symlink 均不能逃逸 provider root；没有等价安全打开的
平台直接拒绝。持久化内容仅含经过字段级脱敏的
coverage-v1 user/assistant/tool 类型投影；原始 provider envelope、provider-home
源路径和未知字段不会落盘；已验证仓库 `working_dir` 是文档化兼容例外。重放
幂等；incomplete turn 可前推为唯一 complete
revision，但不会改变 checkpoint 的结构仓库父节点。若之后有不同的 complete
payload 声明同一逻辑 turn，claim 会停在 `conflicted`，并在
`agent_coverage_conflict` 中只保留**第一个** challenger：typed canonical payload
先脱敏，再连同 digest、source channel、observed time 与确定性 redaction report
持久化；后续 challenger 不覆盖首份证据，raw provider envelope 与命中的密钥原文
永不落盘。operator 显式解决前，incumbent revision 保持 append-only 且仍为 current。
本地 session erase 会先
写耐久防复活 tombstone，再删除 catalog；自动发现和在途 writer 都不能重建。
唯一的本地绕过是经审计的 `--restore-erased --yes`。

首次读取内容或执行 export 前，交互确认会显示 agent 范围、仅当前仓库边界、
候选数/上限、脱敏写入，以及后续 `libra agent push` 可能上传脱敏 traces 的
提示。`--yes` 只确认该隐私影响，不放宽 provider root、仓库、大小、deadline
或平台授权。跨 session 批次 best-effort；后续 turn 失败时仍精确报告该 session
已经耐久提交的进度。64 MiB 批次预算按 held descriptor 实际读取的字节计费；
成功、格式错误、未授权及超限候选均计费，也包括授权后文件继续增长的部分。
120 秒绝对 deadline 在 discovery 前开始，并在遍历、解析、分批 reservation、
对象构造与 CAS 中强制。SQLite commit 一旦开始就不会被超时取消：Libra 在每次
commit 前立即检查 deadline，再等待该 commit 得到确定结果；因此最终 turn 已完整
提交时，即使等待结果期间越过 deadline 也仍报告成功。deadline 失败会 abandon
当前 owner 的全部未提交 lease/marker；若该恢复事务自身失败，命令会连同原错误
报告 cleanup 失败和可执行的 `agent doctor --repair` 提示，而不是静默吞掉。
discovery 遍历/打开与已授权 fd 读取都在可于超时杀死的私有 helper 进程中完成
（读取 helper 消费同一个已打开 descriptor），并由同一个绝对 deadline 包裹；
阻塞文件系统操作不能静默突破 120 秒命令预算。
consent 之后的 provider preparation（secure open、parse、redaction、cwd/storage
归属复核）、checkpoint loose-object 写入，以及 traces ref 拼接所需的 commit/tree
读取也全部位于可杀 helper 边界。对象读取只接受请求的 `commit`/`tree` 类型，校验
完整 OID 与声明长度，并在分配前执行 16 MiB 解压 payload 上限；恶意压缩对象不能把
命令 deadline 转化为无界内存占用。

每个可能新建的 loose object 在写入前，都会先把 OID 作为 provisional preclaim
持久化到 attempt marker；只有本 writer 赢得不覆盖发布，并把 OID 耐久迁入
`created_oids` 后，才获得可删除的所有权。发布后、确认前崩溃会选择安全泄漏：
未确认 preclaim 永远不会被自动删除。
对象先压缩到共享私有目录 `objects/info/libra-tmp` 中的唯一临时文件，再以
不覆盖方式发布；若 final path 已存在，必须解压并逐字节验证后才可复用。后续写入
每次最多检查该私有目录 64 项，只清理超过 24 小时且名称精确匹配
`.<40-or-64-lowercase-hex-oid>.tmp-<decimal-pid>-<uuid>` 的普通文件；其它文件和目录保留。
只有启用 `--sync-data` 或 `LIBRA_SYNC_DATA` 时执行文件/目录 fsync；不覆盖原子发布
始终启用。正常 append、失败 finalizer 与 erase 都不执行全仓库 reachability drain；
被拒绝 append 会把精确 generation 标为耐久 `cleanup_pending`，由
`agent doctor --repair`（及未来仓库 GC）忽略普通 writer TTL，立即执行有界全 roots
诊断并退役 ownership。同 session cleanup job 未退役时 erase 会拒绝；erase 本身
不运行该 drain。inline recovery 绝不 unlink 共享
loose object，也不删除其 `object_index` 行：worktree index writer 不共享 SQLite lock，
所以物理回收交给具备 grace/locking 策略的仓库 GC。诊断性证明覆盖 refs、
reflog、全部已注册 worktree index 与进行中的 sequencer 状态：先在 SQLite 写事务外
快照并遍历，再在最终 ownership 退役事务内复核完整快照。每次 marker 注册还带
随机 writer `generation`；对象 preclaim、ownership 确认、最终 ref CAS 与 marker
清理都必须精确比较该 generation，过期 writer 不能接管或删除同 checkpoint 的
takeover marker。证明会在
64 MiB 单对象加载上限内
读取 loose、pack 与 alternate 对象并复算 OID，同时受 250,000 个访问对象和 30 秒
总遍历上限约束（单次 storage read 也受该 deadline）；注册 index 快照位于同一 deadline
控制的可杀 helper 中，以 no-follow/nonblocking 打开并要求普通文件，从持有 descriptor
执行一次 `limit + 1` 增长检查，再对同一批精确字节校验 checksum/解析。index 最多 256 个且合计
最多读取 64 MiB。任一上限触发时均 fail-closed 延后，保留 durable ownership
供诊断和后续重试；成功 drain 只退役 attempt ownership，孤儿 payload 留给仓库 GC。
lease takeover 后，零进度 provisional session 按其
持久化 `import_provisional` 标记清理；live/export 在 marker 注册后的失败会释放
claim/job lease，并且只清普通 marker，绝不清除 `cleanup_pending` job。

`agent clean --gc` 会物理删除最终 coverage 已被清除的终态、无 owner import identity，
也覆盖零 checkpoint identity；不会把它们重置为可再次 replay 的 `discovered`。
dry-run 在回滚事务中模拟 coverage 清理，因此 `import_identities_pruned` 与相同真实
GC 运行一致，同时不修改 catalog。
冲突证据跟随 claim 生命周期，不形成独立 retention root：erase session 或 prune
最终 coverage claim 会级联删除首个 challenger，dry-run 模拟同一删除；checkpoint
history rebuild/prune 将 current claim 回退到更早的存活 revision 时，会删除已经
失效的 challenger 证据，并把 claim 恢复为非冲突的 committed 状态。

tombstone 目前仅在本地生效。已镜像到 D1/R2 的 capture 行不会被删除，
tombstone 也不会传播；在 cloud delete/tombstone propagation 落地前，
`libra cloud restore` 或跨机 re-sync 仍可能复活本地已擦除 capture。因此不能把
此本地控制当作唯一的跨设备擦除机制。

`agent session list --json` 与 `agent checkpoint list --json` 每次返回一页：`data` 携带 `schema_version`、位于 `sessions` / `checkpoints` 下的行（单行结构不变），以及 `next_cursor`——传回 `--cursor` 的不透明游标，列表耗尽时为 `null`。页序为最新在前（`started_at` / `created_at` 降序，行 id 作为并列时的次序键）。

人类可读的 `agent session list` 表格会把 `started_at` 按当前机器时钟显示为相对时间（例如 `2 hours ago`）；JSON 输出仍保留原始 Unix 时间戳，供自动化使用。

每个 checkpoint 行携带 `scope`。`committed` checkpoint 在 turn/session 边界（`Stop` / `SessionEnd`）写入，携带脱敏的 transcript 快照。`subagent` checkpoint 在被观测 agent 的子代理边界（`SubagentStart` / `SubagentEnd`）物化：它们是**独立** checkpoint——可 list/show/export/prune，且 doctor 可见——通过 `parent_checkpoint_id` 链回所属 turn，使嵌套运行成为一等公民，而非只作为主 checkpoint 上的 metadata。

`agent checkpoint show --json` 额外报告 `layout` 摘要（`e4-libra`、`legacy-v1` 表示 AG-20 之前的存量 checkpoint、`unknown` 表示 checkpoint tree 本地不可读），包含 manifest 角色、按 manifest 顺序列出的 transcript 分片、`content_hash` 格式校验，以及 transcript `availability` 标志（`present`/`missing`/`unknown`）——全程不读取 transcript blob 内容。

## 示例

```bash
# 显示已捕获会话数量和最近 checkpoint 摘要
libra agent status

# 显示代理能力矩阵（支持 roster、hook、安装状态）
libra agent list

# 协商带 import/export 方法的版本化能力矩阵
libra agent list --schema-version 2 --json

# 明确同意后导入一个历史 Claude Code session
libra agent import --session <provider-session-id> --agent claude-code --yes

# 导入某时间之后修改的一页 Codex rollout
libra agent import --since 2026-07-01T00:00:00Z --agent codex --limit 20 --yes --json

# 启用 Claude Code 捕获并安装它的 hook（enable 的别名）
libra agent add claude-code

# 启用 Claude Code 捕获并安装它的 hook
libra agent enable --agent claude

# 一次启用所有支持的代理
libra agent enable

# 禁用 Claude Code 捕获并卸载它的 hook（disable 的别名）
libra agent remove claude-code

# 移除历史 gemini hook（仅卸载通道；幂等）
libra agent remove gemini

# 禁用 Claude Code 捕获并卸载它的 hook
libra agent disable --agent claude

# 列出已捕获会话
libra agent session list

# 显示一个会话并复制其已捕获 transcript
libra agent session show <session-id> --extract-transcript /tmp/session.jsonl

# 停止一个已捕获会话
libra agent session stop <session-id>

# 继续一个已停止的已捕获会话
libra agent session resume <session-id>

# 列出已捕获 checkpoint
libra agent checkpoint list

# 分页浏览 checkpoint（默认每页 50；JSON 携带 next_cursor）
libra agent checkpoint list --limit 100
libra agent checkpoint list --cursor <next_cursor>

# 按 id 显示单个 checkpoint
libra agent checkpoint show <id>

# 将 checkpoint 回放为 JSONL transcript
libra agent checkpoint rewind <id>

# 从最近停止的会话中丢弃临时 checkpoint
libra agent clean

# 从每个已停止会话中丢弃临时 checkpoint
libra agent clean --all

# 诊断 hook 安装和捕获状态
libra agent doctor

# 将 refs/libra/traces 推送到默认远程
libra agent push

# 将 refs/libra/traces 推送到具名远程
libra agent push --remote origin

# `libra agent clean` 重写 traces 链后重新推送（force-with-lease）
libra agent push --force-rewrite

# 发现 PATH 上的 libra-agent-<name> RPC 二进制文件
libra agent rpc list

# 在 libra-agent-<slug> 二进制文件上调用单个 JSON-RPC 方法
libra agent rpc invoke <slug> <method> --params '<json>'

# 面向代理的结构化 JSON 信封
libra agent --json status
```

`libra agent --help` 会渲染同一横幅，因此文档和 CLI 表面保持同步（跨命令 `--help` EXAMPLES 推出，见 `docs/development/commands/_general.md` 条目 B）。

## 说明

- 外部 `libra-agent-*` 代理**默认禁用**。使用 `libra config set agent.external_agents.enabled true`（仓库级）显式开启；开启前 `rpc list`/`rpc trust`/`rpc invoke` 会以 `LBR-AGENT-002` 拒绝（`rpc untrust` 始终可用——撤销信任只会收紧安全面）。已发现的二进制在 `rpc trust <slug>` 记录来源前保持隔离（world-writable 目录中的二进制拒绝信任）；每次 invoke 都会复验来源（漂移即撤销信任，`LBR-AGENT-005`）；子进程环境被清空为白名单注入，stderr 被捕获/限长/脱敏——绝不继承。invoke 超时、broken pipe、malformed frame 映射 `LBR-AGENT-012`；IO 硬上限超限映射 `LBR-AGENT-007`。

- 顶层 `agent hooks` 入口是隐藏的，面向由 `libra agent enable` 安装的 hook 配置；用户通常不会直接调用它。若 hook envelope 未通过大小 / UTF-8 / JSON / schema / transcript 路径校验，会以 `LBR-AGENT-008`（退出码 128）fail-closed 拒绝——绝不回显 raw stdin。对不一致 store 执行 checkpoint 操作（如 `checkpoint rewind`）——catalog 行的 `parent_commit` 非法或指向缺失的 traces 对象——会以 `LBR-AGENT-009`（退出码 128）失败；运行 `libra agent doctor` 检查 store。
- `checkpoint rewind --apply` 只恢复工作树文件；代理自身的 transcript 文件不会被重写。
- Hook 和捕获诊断采用 best-effort 方式，设计目标是报告可操作的安装状态，而不是静默忽略缺失的提供商。

### Doctor checkpoint 存储修复（`--repair`）

`libra agent doctor` 按 AG-20 修复矩阵扫描 checkpoint 存储及 writer marker registry；不带 `--repair` 时严格只读，仅报告 `--repair` 将执行的动作：

| `inconsistency_type` | 含义 | `--repair` 动作 |
|----------------------|------|----------------|
| `stale_catalog_row` | `agent_checkpoint` 行的 `traces_commit`/`tree_oid`/`metadata_blob_oid` 与仍可从 `refs/libra/traces` 到达的 checkpoint 不一致 | 从 ref 重建该行的 OID 列（幂等 UPDATE） |
| `missing_objects` | checkpoint 对象在对象库中真正缺失（且无法从 ref 重建）——检查覆盖完整 E4 树：`manifest.json`、`events/lifecycle.jsonl`、`transcript/<agent_kind>.jsonl`（含分片）、`redaction_report.json`、`content_hash.txt`、中间 tree，以及 manifest 声明的全部 blob | 无——标记 `manual_required`；doctor 绝不执行破坏性动作（可尝试 `libra fsck --heal` 或从云端/备份恢复） |
| `missing_catalog_row` | ref 可达的 checkpoint 没有 catalog 行（崩溃窗口 B 残留） | 通过 writer 同款「先探测再插入」的幂等路径重插该行，字段从 commit 的 `metadata.json` 重建（v1 与 v2 两种 shape 均可解析） |
| `missing_object_index` | checkpoint 对象在 `object_index` 中缺行（`libra cloud sync` 看不到）——覆盖 traces commit 加完整 E4 对象集 | 按 writer 行语义幂等补插（tree 记 `tree`，transcript blob 记 `agent_transcript`，sidecar 记 `blob`） |
| `expired_inflight_marker` | 有效 traces writer marker 已过 TTL，包括 provisional preclaim 与已确认新建 loose-object OID 的崩溃残留 | 在最终 ref 事务中 fence 过期 writer，执行串行化的全仓库 root 证明并退役 marker；inline recovery 不 unlink 共享 payload、不删除 `object_index`，物理回收交给仓库 GC |
| `invalid_inflight_marker` | marker JSON、行身份、commit 或 OID 损坏 | 无——标记 `manual_required`；无法解码所有权时自动删除不安全 |
| `conflicted_coverage_claim` | 两个不同的完整 payload 声明了同一逻辑 turn；doctor 只报告哈希化 turn 身份、incumbent revision/digest，以及保留的首个 challenger 的 digest/source/time/redaction 证据 | 无——标记 `manual_required`；检查两份耐久脱敏候选并显式选择恢复方案，不能静默丢弃 provenance；`--repair` 绝不替 operator 选择 winner |

补充规则：

- **legacy-v1 checkpoint**（升级前布局，无 `manifest.json`）计入 `legacy_v1_checkpoints`，永不进入 checkpoint 对象修复类别，也永不被 `--repair` 改写。
- 被**存活的 traces in-flight marker** 覆盖的 checkpoint 是写入中的 writer，不算不一致，会被跳过。
- **没有 checkpoint 的 session 是合法中间态**（active session 尚未产生首个 stop），绝不被标记；只有 checkpoint-without-session 才算 orphan。
- 已捕获的 **gemini 行保持可读**且绝不被标记；残留的 gemini hook **配置**会得到指向仅卸载通道（`libra agent remove gemini`）的提示。
- 所有修复均幂等——连续两次运行 `doctor --repair`，第二次不会做任何事。带 `--repair` 时，每次修复尝试发出一个 `agent.doctor.repair` tracing span（`inconsistency_type`、`repaired`、`manual_required`），transcript 内容绝不进入日志。
