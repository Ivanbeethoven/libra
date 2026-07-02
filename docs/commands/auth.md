# `libra auth`

Host-scoped HTTP token auth (a Libra extension, lore.md §1.6). Token-only
v1: the full lifecycle — write, read, expiry detection, revoke — in one
surface.

## Synopsis

```
libra auth login --host <host[:port]> [--username <u>] [--with-token] [--expires-at <RFC3339> | --expires-in <N>d|h|m|s]
libra auth status [--host <host>]
libra auth logout [--host <host> | --all]
libra auth clear
```

## Description

`auth login` stores a token for a host. **There is deliberately no
`--token <value>` flag** — argv lands in shell history and `/proc`; the
token arrives via a hidden prompt (TTY) or `--with-token` stdin:

```bash
printf '%s' "$TOKEN" | libra auth login --host git.example.com --with-token
```

Tokens are AES-256-GCM-encrypted with the global vault key
(`~/.libra/vault-unseal-key`, 0600) and stored as ciphertext in the global
config DB — `libra config get/list/unset` can neither dump nor forge nor
delete `auth.token.*` entries (the auth surface is the only door). One token
per host:port scope; re-login overwrites. An OS-keyring backend is the 2.7
follow-up behind the same module boundary.

**Attach rules (the trust boundary, stored tokens)**: a stored token is sent
only on requests whose normalized host:port matches, over **https** (http
only for loopback hosts — note a token stored without an explicit port
normalizes to 443, so log in with the explicit port for non-443 loopback
remotes). Cross-host requests never see it. Redirects that would downgrade
https→http are refused outright (reqwest only strips credentials on
host/port changes, not scheme changes). The interactive 401 prompt remains
the process-wide fallback and takes precedence. The `credential fill` helper
also consults the store (https only, silent misses, username pinning
honored); `credential store/erase` never manage auth tokens.

`auth status` never prints the token: per host it reports the username,
expiry, and `valid` / `expired` / `undecryptable` (key changed — log in
again). With `--host` it is scriptable: exit 0 iff a valid token exists.
Expired tokens are warned about at use time with an `auth login` hint.

`auth logout --host <h>` revokes one host; `--all` / `auth clear` (Lore's
verb) revoke everything — revocation works even after key rotation (no
decryption needed) and is idempotent.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success (`status --host`: a valid token exists). |
| `1` | `status --host` with no valid token. |
| `128` | Storage failures. |
| `129` | Usage (bad host/expiry/username; non-TTY without `--with-token`; empty token). |

## Examples

```bash
libra auth login --host git.example.com              # hidden prompt
printf '%s' "$TOKEN" | libra auth login --host git.example.com --with-token
libra auth login --host git.example.com:8443 --expires-in 30d
libra auth status                                    # all hosts, no secrets
libra auth status --host git.example.com && echo ok  # scriptable
libra auth logout --host git.example.com
libra auth clear
```

## Comparison with Git

Git delegates this to credential helpers (`git credential-store` writes
PLAINTEXT to `~/.git-credentials`; managers are external programs). Libra
ships encrypted-at-rest host tokens natively; the repo-scoped
`libra credential` helper protocol remains for Git-compatible flows.
Classified `intentionally-different` in
[`COMPATIBILITY.md`](../../COMPATIBILITY.md).
