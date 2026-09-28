# `libra code`

The top-level `libra code` command has been removed, including `--control` and MCP `--stdio`.

Capture and inspect external agents with `libra agent`.

Tagged repository commit refs (`repo-commit:<kind>:<hex>`) and `ai_index_task_run.base_commit_ref` are consumed by the agent/projection paths — not by the removed MCP `--stdio` server.

## Examples

```bash
libra agent status
libra agent session list
libra --json agent graph <session>
```
