# `libra whoami`

**Compatibility:** `intentionally-different` — Libra host-scoped HTTP auth extension, not a Git command.

## Summary

`libra whoami` is part of Libra's host-scoped HTTP session-token surface (used
by cloud/publish endpoints). It is a Libra-only extension with no Git
equivalent.

Reports the identity associated with the stored token for a host. Flags: `--host <host>`, `--refresh` (re-validate/refresh the token before reporting).

## Examples

```bash
libra whoami --host libra.tools
```
