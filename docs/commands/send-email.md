# `send-email` policy

## Status

`libra send-email` is intentionally unavailable. Libra does not implement SMTP
delivery, does not read Git's `sendemail.*` configuration, does not manage SMTP
credentials, and does not contact a mail server. Invoking the absent command
fails with `LBR-CLI-001` (exit 129) before repository or network work.

This is the P2-04 / D19 compatibility policy. A transport-shaped
`--dry-run`/`--validate-only` stub would imply support for Git's recipient,
alias, configuration, credential, TLS, and SMTP semantics when those semantics
do not exist in Libra.

## Safe workflow

Libra owns patch generation; a dedicated mail tool owns validation and
delivery:

1. Generate Git-consumable messages with `libra format-patch`.
2. Review the generated files locally.
3. Use stock `git send-email --dry-run` to validate the external transport
   configuration and resolved recipients.
4. Remove `--dry-run` only after reviewing the exact recipient list and mailer
   configuration.

All SMTP credentials, aliases, recipient expansion, TLS, retries, and delivery
logs belong to the external mailer. Libra never receives those secrets on this
path.

## Examples

Generate one patch, validate it with stock Git, then send it explicitly:

```bash
libra format-patch -1 HEAD
git send-email --dry-run 0001-*.patch
git send-email 0001-*.patch
```

Generate a reviewed series into a dedicated directory:

```bash
libra format-patch --cover-letter -o outgoing origin/main..HEAD
git send-email --dry-run outgoing/*.patch
```

If `git send-email` is not installed, use another mailer that accepts RFC 2822
message files. Do not rename a custom wrapper to `libra send-email`; scripts
should keep the transport boundary visible.

## Compatibility

| Surface | Libra | Stock Git |
|---------|-------|-----------|
| Patch mail generation | `libra format-patch` | `git format-patch` |
| SMTP delivery | Unsupported | `git send-email` |
| Transport dry run | Unsupported | `git send-email --dry-run` |
| `sendemail.*` config / aliases / credentials | Not read | Read by `git send-email` |

See [`format-patch.md`](format-patch.md) for the supported generation surface
and [`COMPATIBILITY.md`](../../COMPATIBILITY.md) for the command-level tier.
