# Agent Command Development

`libra agent` is an intentionally different external-agent capture extension,
not a Git-compatible command.

The active development contract, backlog, and compatibility guardrails live in
[`../tracing/agent.md`](../tracing/agent.md). Keep this file as the command
development index entry so `docs/development/commands/README.md` can list every
public CLI command without duplicating the Agent planning document.

## Deferred / Non-goal parity

The following external-agent parity surfaces are decided **non-goals** for the
current wave. Each is recorded — with its handling and restart condition — in
the 「还未实现的功能」 table of [`../tracing/agent.md`](../tracing/agent.md)
(the canonical Agent contract); they are surfaced to users in
[`docs/commands/agent.md`](../../commands/agent.md) and in the `agent` row of
[`COMPATIBILITY.md`](../../../COMPATIBILITY.md):

1. **`agent add`/`remove` `--local-dev` / `--force`** — unpublished; canonical
   `status` / `enable` / `disable` (+ `add` / `remove` aliases) only. If
   implemented, each must hang on both the canonical verb and its alias.
2. **Provider-specific transcript compaction/reassemble trait** — deferred parity
   on top of the landed manifest-relative chunking (no provider-specific
   compactor yet).
3. **Optional capability traits** (`ProtectedFilesProvider`, `TranscriptCompactor`,
   `HookResponseWriter`, `RestoredSessionPathResolver`, …) beyond the landed
   `DeclaredAgentCaps` set — no public behavior yet.
4. **External-RPC method family beyond the v2 `info`/capability gate** —
   undeclared capabilities stay fail-closed.
5. **Non-first-batch supported roster** — `gemini` / `cursor` / `copilot` /
   `factory-ai` stay `supported=false` (unsupported, not hook-installable, not
   launchable) and are omitted from `agent list` entirely; the first batch is
   `claude-code` / `codex` / `opencode`. The omission is pinned by
   `tests/command/agent_roster_test.rs::agent_roster_surface`; the unsupported
   registry classification stays pinned by
   `tests/compat/agent_capability_matrix_pin.rs`.

## Historical import contract

The active DR-05/M4 contract is implemented by `libra agent import` and is
specified canonically in [`../tracing/agent.md`](../tracing/agent.md): explicit
consent before content access/export, provider-root descriptor authorization,
typed redaction before persistence, current-repository ownership, coverage
claim + import identity fencing, and local erase tombstones. The default
`agent list --json` remains schema v1; callers opt into the method matrix with
`--schema-version 2`. Batch limits charge bytes actually read from the held
source even when a candidate later fails validation, and the absolute deadline
begins before discovery, bounds reservation/object/CAS work, and releases every
owned uncommitted import lease on expiry. Transaction commit awaits are not
cancelled: the deadline is checked immediately before commit and the resulting
success is authoritative even if observation finishes after the deadline. A
failed abandonment is chained into the surfaced error with a doctor-repair hint.
Repository ownership is the canonical shared Libra
storage identity, so sibling linked worktrees are accepted while cross-repo
sources remain rejected. Import attempt markers are created in the reservation
transaction; live, export, and subagent writers use the same fail-closed
pre-object registration. The effective per-source read cap is
`min(agent.max_transcript_read_bytes, 16 MiB)`; explicit larger settings emit
the actual effective value. Discovery traversal/open and held-descriptor file
reads run in private, kill-on-timeout helper processes wrapped by the command's
absolute deadline. Provider roots are
opened component-by-component; Claude sources and each nested Codex date
directory are opened relative to pinned no-follow descriptors before consent. Each new OID is added
as a durable provisional preclaim before its loose-object write, but becomes
deletion-eligible only after this writer wins publication and records it in
`created_oids`; a crash between those steps leaks safely instead of claiming a
concurrent writer's object. The object is compressed to a unique file in the
shared private `objects/info/libra-tmp` directory and promoted without overwrite,
with any existing final object fully validated before reuse. Fsync is conditional
on `--sync-data`/`LIBRA_SYNC_DATA`. A 64-entry bounded, 24-hour scavenger removes
only exact `.<40-or-64-lowercase-hex-oid>.tmp-<decimal-pid>-<uuid>` regular files and retains
unrelated entries. Expired construction attempts and `cleanup_pending` jobs are
retired by explicit `agent doctor --repair`/GC maintenance, not append or erase;
cleanup-pending ownership ignores the ordinary writer TTL, blocks same-session
erasure until retired, and is immediately repairable by doctor. Malformed
markers are surfaced as manual-required rather than silently skipped.
Each marker has a random writer generation, and every ownership mutation,
final ref CAS, and clear operation compares it exactly so same-checkpoint
takeover cannot be confused with the expired writer.
Rejected-object diagnostic reachability covers loose, packed, and alternate
objects under a 64 MiB per-object load-cost cap, full OID verification, a
250,000-object traversal cap, and a 30-second per-read/total deadline. Its roots
include refs, reflogs, registered worktree indexes, and sequencer state; roots
are snapshotted outside the writer transaction and revalidated before ownership retirement.
Index snapshotting runs in a killable helper under the same aggregate deadline;
no-follow/nonblocking regular-file opens, held-descriptor `limit + 1` reads, and
checksum/parsing of those exact bytes close special-file, growth, and path-reopen races.
Index enumeration is capped at 256 files/64 MiB aggregate. Any refusal preserves
durable cleanup ownership and fails closed. Inline recovery never unlinks a
shared loose object or deletes its object-index row; physical reclamation is
delegated to repository GC because index writers do not share the SQLite lock.
Rejected-writer ownership registration is O(1), durable, and blocks empty-catalog erasure until
the ownership job is safely retired. Persisted provisional ownership,
not process-local "created" state, controls zero-progress session reaping.
Retention GC physically deletes terminal ownerless import identities once no
coverage claim remains, including zero-checkpoint rows. Dry-run obtains the
same identity count by simulating coverage removal in a rolled-back transaction.
