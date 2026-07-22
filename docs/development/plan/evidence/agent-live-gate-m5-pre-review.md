# Live agent gate evidence — M5 (pre-review)

> plan-20260713「本机 live agent 执行验证门」固定字段记录。sanitized-only；
> provider session ids、home/source paths、transcript text、digests and raw command output are omitted.

- release/tag: `pre-review`
- commit: working tree over `1d6e2f6`
- UTC time: 2026-07-18T11:03:32Z
- providers: real local `claude-code` 2.1.210 and `codex` 0.144.4
- scope: M5 = DR-06 source-scoped subagent content revisions and explicit
  resolved/unresolved boundary attribution
- command: `LIBRA_RUN_LIVE_AGENT_GATE=1 cargo test --features test-live-agent
  --test agent_live_gate_test live_m5_subagent_boundary_content_attribution
  -- --exact --nocapture --test-threads=1`
- sanitized aggregate results:
  - real bounded Claude parent sessions with subagent files selected: 1
  - Claude current subagent content sources: at least 1; all links unresolved,
    all structural parent checkpoint ids absent, all source ids opaque SHA-256 keys
  - replay revision delta: 0
  - real Codex native subagent boundary checkpoints: at least 1
- stable result: success (1 passed, 0 failed; gated provider/source absence is
  a failure, not a skip)
