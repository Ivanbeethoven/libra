# Live agent gate evidence — M1 (pre-review)

> plan-20260713「本机 live agent 执行验证门」固定字段记录。sanitized-only：
> 无原文、无绝对 source path、无密钥。

- release/tag: `pre-review`
- commit: working tree at `dac20cc` (+ clippy lint fix, uncommitted at run time)
- UTC time: 2026-07-13T14:3xZ
- provider: `claude` (Claude Code CLI) 2.1.207 — real locally-produced session
  transcript (~451 KiB JSONL) from the developer machine's provider root
- scope: M1 = DR-04a TranscriptSource seam + DR-05c-0 coverage gate, live path
- procedure: fresh `libra init` scratch repo → `agent hooks claude-code stop`
  with the real transcript (twice, identical envelope)
- stable result: success
- aggregate counts:
  - checkpoints after first ingest: 1
  - checkpoints after repeated ingest: 1 (gate no-op — no duplicate append)
  - coverage claims: 5 × `catalog_committed`, all `complete`
  - coverage revisions: 5 (one per claimed logical turn)
- interpretation: real Claude-produced content flowed provider-root check →
  seam handle → redaction → coverage-v1 normalize/split (5 logical turns) →
  claim reservation → single atomic ref+catalog+claim commit; the repeated
  event was fully deduplicated by the gate.
- note: OpenCode content source not yet delivered (DR-04b, M3) — per the
  gate's per-DR applicability rule it is not exercised at M1; Codex boundary
  hooks are not part of M1's scope.
