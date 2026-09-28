# `libra code`

顶层 `libra code` 命令已移除，包括 `--control` 与 MCP `--stdio`。

外部 agent 捕获与查看请用 `libra agent`。

带 kind 的仓库 commit 引用（`repo-commit:<kind>:<hex>`）与 `ai_index_task_run.base_commit_ref` 由 agent/投影路径消费，不再经已移除的 MCP `--stdio` 服务。

## Examples

```bash
libra agent status
libra agent session list
libra --json agent graph <session>
```
