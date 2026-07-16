# Live agent gate evidence — M4 (pre-review)

> plan-20260713「本机 live agent 执行验证门」固定字段记录。sanitized-only；
> provider session ids、home/source paths、transcript text and digests are omitted.

- release/tag: `pre-review`
- commit: working tree over `2950e65ce32018bb61d9a6fc3babd28d75e90d48`
- UTC time: 2026-07-15T07:43:07Z
- providers: real local `claude-code` 2.1.210, `codex` 0.144.4, and
  `opencode` 1.17.18
- scope: M4 = DR-05a/b/c historical import, explicit consent/current-repository
  ownership, per-turn coverage arbitration, local erase tombstone, audited restore,
  and versioned `agent list` v1/v2 capability surface
- command: `LIBRA_RUN_LIVE_AGENT_GATE=1 cargo test --features test-live-agent
  --test agent_live_gate_test -- --nocapture --test-threads=1`
- procedure and sanitized results:
  - real Claude and Codex by-id discovery: passed
  - real Claude/Codex/OpenCode historical import into the current repository:
    passed for all three delivered source paths
  - immediate replay of every imported source: passed with exactly zero new
    checkpoints, proving per-turn idempotency through the public CLI
  - a real Claude source belonging to another repository: rejected with
    `LBR-AGENT-015`; no partial session was created
  - a gate-owned real Claude capture was erased, replay was rejected with
    `LBR-AGENT-019`, then `--restore-erased` succeeded and wrote the append-only
    restore audit; the next replay again wrote zero checkpoints
  - real OpenCode export under the Required offline bwrap profile: passed,
    8,627 authorized bytes normalized to one coverage-v1 turn
- identity finding: the newest Codex file was a forked review-subagent rollout
  carrying both its own and its parent's `session_meta`. Production correctly
  rejected that ambiguous identity. The live selector was tightened to choose a
  real root rollout with exactly one matching provider identity; production
  validation was not weakened.
- source continuity: Claude authentication expired before this final run, so a
  previously restored, gate-owned small real session was selected again only
  after its append-only restore audit proved gate ownership. The reset removed
  that capture identity and tombstone before the run; no transcript content or
  identifier is recorded here. The 16 MiB effective cap and 120 second import
  deadline were not relaxed.
- new-binary → old-binary gate: a preserved 0.18.88 binary opened a fresh
  2026071403 repository and failed closed at exit 128 (`newer schema`, latest
  supported 2026071401). Post-checks remained tombstone=1, matching session=0,
  max schema=2026071403.
- stable result: success (4 passed, 0 failed in 175.12 seconds; three real
  provider import sources, exact-zero replay checkpoints, one cross-repository
  rejection, one erase/replay rejection/restore cycle, and one real OpenCode
  turn; gated M4 provider absence is a failure, not a skip)
