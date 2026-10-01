# `libra mega`

## Goal

Provide a small, scriptable Mega forge client with a command shape familiar to
users of `gh`. This is a Libra extension rather than a Git compatibility
surface. User-facing usage is documented in
[`docs/commands/mega.md`](../../commands/mega.md).

## Current Surface

- `status`
- `auth login|status|logout`
- `issue list|view|create|close|reopen`
- `cl list|view|close|reopen|merge`

Mega change lists are created by the push and Buck workflows, so the CLI does
not expose a synthetic `cl create` operation.

## Design

- The default API origin is `https://git.gitmega.com`; `--host` and
  `MEGA_HOST` override it.
- Remote hosts require HTTPS. Loopback HTTP is permitted for local development
  and deterministic tests.
- `MEGA_TOKEN` is an ephemeral override. Interactive login stores tokens in
  Libra's encrypted, host-scoped auth store.
- Redirects are disabled so a token cannot be forwarded to another origin.
- List requests cap `--limit` at 100 and support JSON/machine output through
  the global output contract.

The Mega API must accept access tokens on the authenticated issue, change-list,
and current-user routes. Browser session cookies remain supported; an explicit
invalid `Authorization` header fails closed instead of falling back to cookies.

## Verification

Focused coverage lives in `tests/command/mega_test.rs` and the unit tests in
`src/command/mega.rs`. The HTTP contract tests pin request paths, Bearer auth,
list pagination payloads, JSON output, and pre-network validation failures.

## Remaining Work

- Add commands for comments, labels, assignees, and merge-queue operations when
  those workflows are needed from automation.
- Add a live-service smoke test behind an explicit network feature once CI has
  a revocable Mega test token.
