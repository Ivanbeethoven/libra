# libra hydrate

`libra hydrate <path>...` materializes working-tree content **on demand**
(lore.md 3.3). It is the honest, platform-portable v1 of Lore's "hydrating
VFS": an EXPLICIT command, not a transparent FUSE-on-access filesystem (that
remains a `worktree-fuse` follow-up). Whole-object only — no FastCDC range.

## Compatibility

- Level: `intentionally-different` (Git has no hydrate/VFS surface).

## Design

For each requested path (and, by default, its transitive forward dependencies
via the 3.1 dependency graph), hydrate resolves the blob local → alternate
(2.3) → remote durable tier and writes it into the working tree. The read
policy is honored for free (`--offline`/`--local` refuse a remote fetch).

### Failure-recovery contract

Each blob is OID-verified on a borrowed/remote hit (and, with `--verify`,
re-hashed on the local path — healing from the durable tier on a mismatch),
then published via an atomic temp-file + rename. A hydration that fails for
ANY reason — object missing everywhere, remote unreachable, transport error,
verify mismatch, interruption — leaves the pre-existing worktree file
UNTOUCHED and never a truncated or half-written file.

### Sparse gating

An active sparse view (2.2) gates the FULL hydration set — both the requested
roots AND their pulled-in dependencies — so a dependency edge can never bypass
a view set to avoid materializing large out-of-view assets. `--ignore-sparse`
overrides.

## Examples

```bash
libra hydrate scene.usd                 # materialize scene.usd + its deps
libra hydrate scene.usd --no-deps       # just this file
libra hydrate assets/ --depth-limit 2   # bound the dependency closure
libra hydrate big.bin --verify          # re-hash the payload before landing
libra hydrate a b --dry-run             # report what would hydrate
libra hydrate x --ignore-sparse         # hydrate an out-of-view path
```

## Deferred (not v1)

LFS-pointer blobs (their download path is not yet atomic — skipped cleanly),
symlink/gitlink entries, transparent FUSE on-access hydration, FastCDC
byte-range hydration, and cross-machine dependency expansion (needs 3.2 to
wire `refs/notes/deps` into fetch).
