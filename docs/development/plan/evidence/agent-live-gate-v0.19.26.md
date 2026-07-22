# Live agent gate evidence — M4 (`v0.19.26` post-release)

> plan-20260713「本机 live agent 执行验证门」固定字段记录。sanitized-only；
> provider session ids、home/source paths、transcript text、digests and raw command output are omitted.

- release/tag: `v0.19.26`
- commit: `93f8ac3b5e463db2742cd83c6d461ba069d12cae`
- UTC time: 2026-07-19T19:42:49Z
- providers: real local `claude-code` 2.1.210, `codex` 0.144.4, and
  `opencode` 1.17.18
- scope: M4 = consented historical import, current-repository scoping,
  per-turn replay idempotency, and erased-session tombstone fencing
- runner note: the repository database had already advanced to M5 schema
  `2026071407`, so the post-release behavior gate ran on the forward-compatible
  M5 working tree while pinning and reporting the public M4 release commit above;
  only the M4 acceptance surface was exercised
- command: `LIBRA_RUN_LIVE_AGENT_GATE=1
  LIBRA_LIVE_M4_GATE_OWNED_CLAUDE_SESSION=<redacted>
  cargo test --features test-live-agent --test agent_live_gate_test
  live_m4_historical_import_three_provider_acceptance -- --exact --nocapture
  --test-threads=1`
- sanitized aggregate results:
  - real current-repository provider sessions validated: 3 (one per provider)
  - provider replay checks with zero new checkpoints: 3
  - real cross-repository sources rejected: 1 (`LBR-AGENT-015`)
  - gate-owned sessions erased and restored: 1
  - erased-session replay attempts blocked before restore: 1 (`LBR-AGENT-019`)
  - audited restore checkpoints written: 1; a fail-closed object-index repair
    boundary (`LBR-AGENT-018`) was closed by the required replay, whose final
    checkpoint delta was 0
- stable result: success (1 passed, 0 failed; provider/source absence and any
  unclosed partial result are failures, not skips)
