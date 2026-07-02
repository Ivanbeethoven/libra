# `libra revision`

Revision ordinal index (a Libra extension, lore.md §1.16, porting Lore's
`revision find number`). Git has no equivalent surface.

## Synopsis

```
libra revision find --number <N> [--ref <branch>]
libra revision number <commitish> [--ref <branch>]
libra revision index [--ref <branch>] [--rebuild]
```

## Description

Each branch's **first-parent chain** gets a monotonic 1-based numbering
(1 = root, N = tip), stored in a rebuildable SQLite side table. The numbering
is a pure function of the tip (deterministic across rebuilds and machines).
Commits reachable only through merged-in side branches have **no** ordinal —
the reverse lookup says so explicitly rather than inventing a number.

Freshness is re-validated on **every** read, in the same transaction as the
lookup: fast-forwards append (existing ordinals never change); history
rewrites (rebase/amend/reset) and `refs/replace` changes trigger a full
deterministic rebuild. A stale index never answers. The first query on a
long branch walks its whole chain once (O(chain) object loads, possibly
remote under tiered storage); later queries are index hits.

| Subcommand | Purpose |
|---|---|
| `find --number <N>` | Print the OID of revision #N (Lore's `revision find number`). Out of range → exit 1 naming the chain length; `N < 1` → 129. |
| `number <commitish>` | Reverse lookup: the ordinal of a commit on the ref's chain. Not on the chain → exit 1 with the coverage note. |
| `index [--rebuild]` | Freshness report (tip, count, built-at); `--rebuild` forces a deterministic rebuild and prunes index rows for deleted branches. |

`--ref <branch>` targets any local branch; the default is the current branch
(detached HEAD → an error suggesting `--ref`). `--json` emits structured
output. `find --metadata` (search by 1.10 revision metadata) is a documented
follow-up — the ordinal index provides the deterministic iteration order such
a scan would walk.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success. |
| `1` | Miss (ordinal out of range; commit not on the chain). |
| `128` | Fatal (not a repository; detached HEAD without `--ref`). |
| `129` | Usage (`--number` < 1). |

## Examples

```bash
libra revision find -n 1                 # the root revision
libra revision number HEAD               # how long is the mainline?
libra revision find -n 42 --ref main     # the 42nd revision of main
libra revision index --rebuild           # deterministic rebuild + prune
libra --json revision number HEAD        # structured output
```

## Comparison with Git

Nearest Git analogues: `git rev-list --first-parent --count <oid>` (reverse
direction) and `<tip>~<k>` suffix arithmetic (forward). Classified
`intentionally-different` in [`COMPATIBILITY.md`](../../COMPATIBILITY.md).
