# `libra logout`

**Compatibility:** `intentionally-different` — Libra host-scoped HTTP auth extension, not a Git command.

## Summary

`libra logout` is part of Libra's host-scoped HTTP session-token surface (used
by cloud/publish endpoints). It is a Libra-only extension with no Git
equivalent.

Clears stored session tokens. Flags: `--host <host>`, `--all` (every host), `--local-only` (drop the local token without notifying the host).

## Examples

```bash
libra logout --host libra.tools
```
