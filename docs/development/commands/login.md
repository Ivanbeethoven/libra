# `libra login`

**Compatibility:** `intentionally-different` — Libra host-scoped HTTP auth extension, not a Git command.

## Summary

`libra login` is part of Libra's host-scoped HTTP session-token surface (used
by cloud/publish endpoints). It is a Libra-only extension with no Git
equivalent.

Authenticates to a host and stores a session token. Flags: `--host <host>` (default host), `--no-browser` (device/manual flow instead of opening a browser). Times out after 15 minutes.

## Examples

```bash
libra login --host libra.tools
```
