# Live agent gate evidence — M6 (pre-review)

> plan-20260713「本机 live agent 执行验证门」固定字段记录。sanitized-only；
> provider session ids、home/source paths、transcript text、digests and raw command output are omitted.

- release/tag: `pre-review`
- commit: working tree over `7eaf3925b0b383446752406f8d0b6ef01f115ffd`
- UTC time: 2026-07-21T16:13:27Z
- providers: real local `claude-code` 2.1.216, `codex` 0.144.6, and
  `opencode` 1.17.18 captures already imported into the current repository
- scope: M6 = DR-07 capture-only `libra agent graph` TUI/JSON projection,
  legacy compatibility, explicit subagent link state, privacy whitelist,
  non-TTY refusal, and zero-write behavior
- commands:
  - `LIBRA_RUN_LIVE_AGENT_GATE=1 cargo test --features test-live-agent
    --test agent_live_gate_test live_m6_agent_graph_real_capture_is_private_and_readonly
    -- --exact --nocapture --test-threads=1`
  - interactive `libra agent graph <session>` in a real PTY, followed by
    `libra --json agent graph <session>` for one captured session from each
    delivered provider
- sanitized aggregate results:
  - real captured sessions available: Claude Code 7, Codex 14, OpenCode 1
  - committed logical turns / revisions available: 105 / 105
  - real provider projections checked: 3 present sessions, with non-empty
    turn arrays for Claude Code, Codex, and OpenCode
  - real subagent links checked: 1 unresolved node with no fabricated boundary
  - TUI: two-pane session → turn → revision tree rendered successfully in a
    PTY and exited cleanly with `q`
  - JSON: schema v1 and whitelisted structural fields only; forbidden raw
    metadata/path/blob/digest column names and the current repository path were
    absent
  - non-TTY without `--json`/`--machine`: rejected before TUI initialization
    with `LBR-CLI-002`
  - row-for-row snapshots of all 10 capture/import/export catalog tables were
    identical before and after the public graph and refusal paths
  - no live erased tombstone existed; erased display/non-resurrection remains
    pinned by the deterministic L1 tombstone fixture, avoiding destructive
    erasure of an operator-owned live capture solely for this read-only gate
- stable result: success (1 gated M6 test passed, 0 failed; interactive TUI and
  three-provider JSON projections manually checked; zero catalog mutations)
