# AI Handoff: v9fs Git Index Timestamp Fix

## Problem

Running `libra` on a v9fs/9p-backed filesystem can panic inside `git-internal`:

```text
thread 'libra-cli' panicked at .../git-internal-0.8.1/src/internal/index.rs:154:58:
called `Result::unwrap()` on an `Err` value: Error { kind: Unsupported, message: "creation time is not available for the filesystem" }
fatal: CLI thread panicked
```

The failing path is worktree indexing. The old code used `fs::Metadata::created().unwrap()` for the index `ctime` field. On Linux/v9fs, creation/birth time may be unsupported, so `created()` returns `Err(Unsupported)`.

## Fix Summary

The fix lives in the sibling `git-internal` checkout:

- `git-internal/src/internal/index.rs`
  - Added `index_ctime()` and `index_mtime()` helpers.
  - On Unix, `ctime` now comes from `MetadataExt::ctime()` / `ctime_nsec()`.
  - On Unix, `mtime` now comes from `MetadataExt::mtime()` / `mtime_nsec()`.
  - On non-Unix, timestamp helpers fall back deterministically instead of unwrapping.
  - `IndexEntry::new`, `Index::refresh`, and `Index::is_modified` all use the same helpers.
- `git-internal/Cargo.toml`
  - Version aligned to `0.8.1` so it satisfies `libra`'s dependency requirement.
- `libra/Cargo.toml`
  - Added a local patch:

```toml
[patch.crates-io]
git-internal = { path = "../git-internal" }
```

This requires `libra` and `git-internal` to remain sibling directories during local validation.

## Why This Is Correct

Git index `ctime` means inode change time, not filesystem creation/birth time. Using `created()` was both semantically wrong for Git metadata and fragile on filesystems that do not expose birth time. Unix `MetadataExt::ctime()` matches the Git index meaning and is available on v9fs where `created()` is not.

## Validation Commands

From the `git-internal` repository:

```bash
cargo test internal::index --lib
```

From the `libra` repository:

```bash
LIBRA_SKIP_WEB_BUILD=1 cargo check --bin libra
```

`LIBRA_SKIP_WEB_BUILD=1` is needed if `pnpm` is not installed. Without it, `libra/build.rs` tries to run `pnpm install` for web assets before Rust checking completes.

## WSL/v9fs Verification Notes

Use a worktree located on the v9fs/9p-mounted path that previously triggered the panic. Then run the same `libra` command that touches the index, for example the original command or an add/status flow.

Expected result after the fix:

- No panic about `creation time is not available for the filesystem`.
- Cargo output should show `git-internal v0.8.1 (.../git-internal)` when checking/building `libra`, confirming the local patch is active.

If the panic still references a crates.io path like:

```text
.../.cargo/registry/src/.../git-internal-0.8.1/...
```

then `libra` is not using the local patched crate. Confirm the sibling layout and rerun from the patched `libra` checkout.
