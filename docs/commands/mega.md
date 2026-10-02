# `libra mega`

Use Mega's forge API from the terminal. The command mirrors the high-value
parts of GitHub CLI (`gh`): authentication, issue workflows, and pull-request
style review workflows. Mega calls pull requests **change lists (CLs)**.

## Synopsis

```text
libra mega [--host <URL>] status
libra mega [--host <URL>] auth <login|status|logout>
libra mega [--host <URL>] issue <list|view|create|close|reopen>
libra mega [--host <URL>] cl <list|view|close|reopen|merge>
```

The default API origin is `https://git.gitmega.com`. Set `MEGA_HOST` or pass
`--host` for self-hosted installations. Non-TLS hosts are refused except for
explicit loopback URLs used during local development.

## Authentication

Generate an access token from the Mega web UI, then store it without placing it
in argv or shell history:

```bash
libra mega auth login
printf '%s' "$MEGA_TOKEN" | libra mega auth login --with-token
libra mega auth status
```

Interactive input is hidden. Stored tokens use the same host-scoped encrypted
vault or OS keyring as `libra auth`; `auth logout` removes the token. For CI,
`MEGA_TOKEN` supplies an ephemeral token and takes precedence over stored auth.
Tokens are attached only to the exact normalized API host and redirects are not
followed.

Issue and CL lists are public when the server allows anonymous reads. Viewing
details and every mutation require authentication.

## Issue Commands

```bash
libra mega issue list --state open --author alice --limit 20
libra mega issue view ISSUE_LINK
libra mega issue create --title "Bug report" --body "Steps to reproduce..."
libra mega issue close ISSUE_LINK
libra mega issue reopen ISSUE_LINK
```

List filters map directly to Mega's API: `--state`, `--author`, repeatable
`--assignee`, repeatable numeric `--label`, `--sort`, `--asc`, `--page`, and
`--limit` (1-100).

## Change Lists

```bash
libra mega cl list --state open
libra mega cl view CL_LINK
libra mega cl close CL_LINK
libra mega cl reopen CL_LINK
libra mega cl merge CL_LINK
```

CL creation is intentionally absent: Mega creates change lists from its push or
Buck upload workflow, rather than a standalone REST create operation.

## Structured Output

All subcommands support Libra's global `--json` and `--machine` flags:

```bash
libra --json mega issue list --state open
libra --machine mega cl view CL_LINK
```

List output contains `page`, `limit`, `total`, and the server's item records.
View output preserves the typed Mega detail response. Authentication output
never contains a token.

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Request succeeded. |
| `128` | Network, authentication, server, or response-contract failure. |
| `129` | Invalid host, pagination, state, title, or token input. |

## Examples

```bash
libra mega status
libra mega auth status
libra mega issue list --state open --assignee alice
libra --json mega issue view ISSUE_LINK
libra mega cl list --state draft --author build-bot
libra mega cl merge CL_LINK
```

## Comparison With `gh`

`issue list/view/create/close/reopen` follows the corresponding `gh issue`
mental model. `mega cl` is analogous to `gh pr`; names and payloads follow the
Mega API, and creating a CL remains part of the push workflow. This is a Libra
extension and has no Git command equivalent.
