# Two O(index) linear scans on large worktrees — design notes

Measured on a 124k-entry worktree (libra 0.19.63). Both defects are independent of
any particular storage backend: they reproduce on a plain local repository, and only
get worse when the worktree is remote or FUSE-backed.

| command | measured | depends on changes? |
|---|---|---|
| `ls-tree -r HEAD` (walks the same 124k entries) | **0.39 s** | — |
| `status` | **0.55 s** | — |
| **`ls-files`** | **> 900 s** | no |
| **`commit`** (`validate_index_objects`) | **~84 s** | no |

The first two rows matter as a baseline: walking 124k entries takes well under a
second, so the minutes spent by the other two are implementation cost, not tree size.

---

## 1. `ls-files` computes worktree state unconditionally

`src/command/ls_files.rs`: for **every** index entry the cached block computed

```rust
let exists = fs::symlink_metadata(&worktree_path).is_ok();
let is_modified = exists && entry_modified(...)?;
```

and only afterwards filtered on `--deleted` / `--modified`. `entry_modified` reads
the file and hashes it, so a plain `libra ls-files` — a pure index dump that Git
answers without ever touching the worktree — read and hashed every tracked file.

### Fix

Worktree state is only needed to **filter** (`-d`/`-m`), to **label** (`-t`), or to
fill the `status` field that the `--json` shape always carries:

```rust
let needs_worktree_state = _args.deleted || _args.modified || _args.tag || json;
```

`json` is threaded in from `execute_safe` (the CLI entry point), because
`FileEntry` derives `Serialize` and its `status` is part of the JSON output — without
it, `--json` would silently report every entry as `cached`. `OutputConfig::default()`
has no JSON format, so the non-CLI `execute` path still skips the work.

### Correctness

`entry_modified` is a pure predicate (it reads and hashes, and does not mutate the
index), so skipping it changes nothing except the `status` field — which is only
consumed by `-t` and by JSON, both now covered by the guard. Stage 1/2/3 entries
report `unmerged` regardless of worktree state.

Verified: `ls-files`, `-s`, `-t`, `-d`, `-m`, `-o`, `-c`, `-o -i --exclude-standard`
and `--json` are **byte-identical** before/after, and `-t` still reports real state
(`H`/`C`/`R`). On a 124k worktree `ls-files` went from >900 s to **0.25 s**, while
`-t` stays slow by design.

---

## 2. `commit` validates object types by reading every object

`src/internal/tree_plumbing.rs::validate_index_objects_with` asked
`storage.get_object_type(&entry.hash)` for **every** stage-0 index entry. That goes
through `ClientStorage::get_object_type` →

```rust
self.block_on_storage(async move { storage.get(&hash).await.map(|(_, t)| t) })
```

— i.e. it **materialises the whole object to keep a single type tag**. On a 124k
index that is 124k full object reads: ~84 s, and the dominant cost of `commit`.

### Fix

The repository already has the right call: `ClientStorage::get_object_types_bounded_many`,
whose local backend answers from `object_type_from_loose_header` / the pack entry
header and never decodes a payload (and whose remote fallback is a verified bounded
read). Use it once, up front:

```rust
let probed = storage.get_object_types_bounded_many(&hashes).ok();
```

then treat a probed hit as final, and let anything the probe could not answer fall
through to the existing per-object read.

### Correctness

The probe's contract is "missing IDs are absent from the returned map; all other
failures retain their failing OID", so:

- probed **and** matching the expected type → nothing left to check (the common case:
  a healthy index resolves entirely here);
- probed with a **different** type → reported as `WrongObjectType`, exactly as before;
- **not** in the map (absent object), or the probe failed for any reason → the
  original per-object read runs, so absence keeps its `ObjectNotFound`/`missing_ok`
  behaviour and every error message is unchanged.

Nothing is weakened: the check is still made for every entry, it is simply cheap.
This deliberately avoids the tempting shortcut of validating only the changed paths —
that would stop catching a dangling reference on an untouched entry.

Verified: `cargo check` + `command_test` green; on a 124k worktree
`commit --allow-empty` went from ~84 s to **~7.5 s**, HEAD advances correctly and
`fsck` reports clean.
