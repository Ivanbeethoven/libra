# Libra link/subtree Composition RFC

## Status

Draft (Proposed). This document is an RFC gate, not an implementation. Nothing described here ships until this RFC is accepted and its `§3.0.1` gate cells are all filled and reviewed. Until then any partial work may only land as a feature-gated experiment (`lore.md` §3.0.1 rule).

## Date

2026-07-04

## Owning gap item

`docs/development/gap/lore.md` §3.4 — "link/subtree composition RFC". Declared dependencies: `metadata` (1.5 / 1.10), `sparse` (2.2), `auth` (1.6 / 2.7).

**Relationship to D1 (careful framing).** Declination **D1** (`docs/development/commands/_compatibility.md:66`, "submodule 子命令族 — 状态：拒绝") reopens only "if a multi-repo dependency scenario that CANNOT be solved by monorepo or object storage arises, **PLUS a clear RFC**". This RFC does **not** claim that trigger — it argues the *opposite*: the underlying user need (compose another project's directory, updatably) **IS** solvable inside the monorepo/object-storage model, via ordinary versioned tree content with provenance. So this RFC does **not** reopen D1; **D1 stays declined**, the submodule/`160000` shape is never introduced, and this document instead specifies the in-boundary alternative that makes the multi-repo pointer unnecessary. (If a future scenario genuinely cannot be solved this way, *that* would be the D1-reopen RFC — this is not it.)

## Background / Motivation

Lore ships two composition primitives side by side: `link` (versioned, recorded into the revision) and `layer` (local, overlaid at materialization time). Libra has already landed the local half:

- **`layer` (2.4, `src/internal/layer/mod.rs:58`)** is a named, purely-LOCAL overlay. Its single load-bearing invariant is **never-enters-commit**, enforced at two independent chokepoints (an un-negatable highest-precedence ignore exclusion at `src/utils/ignore.rs:257` / `src/utils/util.rs:1200`, plus a hard `add`-staging guard at `src/command/add.rs:150` that closes the `add --force` hole). `layer`'s module doc explicitly records that its **VERSIONED sibling `link` is deferred to this §3.4 RFC** (`src/internal/layer/mod.rs:3-7`).

The versioned half does not exist yet. Users who want to vendor a subdirectory of another project into a monorepo, keep it updatable, and later contribute changes back today have only two unappealing options in Git-land: (a) Git **submodules** (a `160000` gitlink pointing at a commit in *another* repository), or (b) manual copy-paste with no provenance. Libra has deliberately declined (a) at the product-boundary level (D1, D4 `clone --recurse-submodules`), and today actively rejects `160000` gitlink tree entries across `rebase`/`merge` (`src/command/rebase.rs:4038`, `src/command/merge.rs:2370`) and skips them in `archive`/`rev-list`/`bundle` (`src/command/rev_list.rs:568`) — `fast-export` is the exception, preserving/emitting `M 160000 <id> <path>` records (`src/command/fast_export.rs:132`) for round-trip fidelity — because **Libra stores no foreign-repo commit objects**, so a `160000` entry is structurally unresolvable in its object model. `push` still only warns "submodule is not supported yet" (`src/command/push.rs:2878`).

The motivation for this RFC is to give users the *capability* Lore's `link` and Git's submodules provide — compose content from another source, keep it identifiable and updatable — **without** importing the submodule shape that D1 declines and that Libra's object model cannot represent.

## The D1 boundary argument (why this satisfies the reopen gate)

The crux of D1 is not "no composition" — it is "no **multi-repo live pointer**". A Git submodule is a `160000` gitlink: an in-tree pointer to a commit that lives in a *different* object graph, resolved by recursively cloning that other repository. That is a multi-repo dependency, and it is exactly what breaks in Libra: there is no foreign object graph to point into.

The alternative this RFC proposes — **subtree composition with recorded provenance** — is categorically different:

1. **The composed content is ordinary, in-repo, versioned tree content.** A subtree is materialized as regular `100644`/`100755` blob entries and `040000` subtree entries under a prefix directory (`TreeItemMode::Blob | BlobExecutable | Tree`, `src/command/ls_tree.rs:671`). It round-trips through standard Git tree objects with zero private fields. There is **no** `160000` gitlink, no external live pointer, no second object graph. To `status`, `diff`, `checkout`, `merge`, and every downstream consumer, the composed subtree is just files that are in the tree — because they are.
2. **Provenance is metadata, not a pointer.** "Where did `vendor/libfoo` come from, at which source commit" is recorded as commit trailers (which travel with the pack) plus an optional local note — never as a tree entry that must be resolved to make the working tree complete. If the source repository vanishes, the composed content is unaffected; only a future `subtree pull` would report an actionable "source unreachable" error.
3. **It is monorepo-native.** The whole point of D1's "single-repo/trunk-based" boundary is that everything you need is in one object graph. Subtree composition *keeps* everything in one object graph. That is *why* it is the composition model D1 can accept: it resolves the "I want another project's code in my tree, updatably" need **inside** the monorepo/object-storage model D1 says the boundary must be solvable within, rather than by reintroducing the multi-repo pointer D1 rejects.

So this RFC does not "reopen submodules". It closes the actual user need behind D1 with a design that stays on the correct side of the boundary. Submodules/gitlinks remain declined (see Alternatives Considered #1).

## Contrast table: submodule vs layer (2.4) vs subtree (this RFC)

| Property | Git submodule (`160000` gitlink) — DECLINED (D1) | `layer` 2.4 (landed) | `link`/`subtree` (this RFC) |
|---|---|---|---|
| Enters a commit? | Yes, as a `160000` pointer | **Never** (two chokepoints) | **Yes**, as ordinary `100644`/`100755`/`040000` content |
| What is stored in the tree | A commit OID from another repo | Nothing (local overlay only) | Real blobs/subtrees under a prefix |
| Object graph | Multi-repo (foreign objects) | Single-repo | **Single-repo** (all objects in-repo) |
| Provenance | Implicit in `.gitmodules` + gitlink | N/A (not versioned) | Explicit trailers + optional note |
| Git-format compatible | Only if the consumer resolves foreign repos | N/A (never serialized) | **Fully** (standard tree round-trip) |
| Updatable from source | `git submodule update` (fetch foreign repo) | N/A | `libra subtree pull` (three-way merge into prefix) |
| Materialization | Recursive clone of another repo | Explicit `layer apply` to worktree | Fully materialized committed content (atomic write; large blobs use `hydrate`'s write discipline) — **not** lazy-on-access |

`layer` and `subtree` are the two halves of Lore's composition pair, exactly as `src/internal/layer/mod.rs:3-7` frames it: `layer` is **local, never-committed**; `subtree`/`link` is **versioned, committed**. This RFC must not blur them — in particular it must **not** relax `layer`'s never-enters-commit invariant to make it versioned (Alternatives Considered #2).

## Goals

- Let a user compose another source's directory into a prefix of this repo as **ordinary versioned tree content**, with recorded provenance sufficient to update it later.
- Keep the composed result **100% Git-disk-format compatible**: only `100644`/`100755`/`040000` entries, no `160000` gitlink, no private/unparseable field in any object/index/pack/ref/LFS pointer.
- **Reuse** existing single-owner subsystems rather than inventing storage: the notes store and trailer parser for provenance, `sparse` for scoping a composed subtree view, `hydrate` for on-demand atomic materialization, `deps` + the 3.2 `--notes` side-channel as the travel template, and the transport/auth stack for source access.
- Stay **opt-in and explicit** (a `libra subtree` command family), never a default auto-compose model (`lore.md` §3.5 red line).
- Deliver an **honest v1 slice** (copy-a-source-subtree-with-provenance) and explicitly defer richer lazy/live "link" semantics to later, separately-gated phases.

## Non-goals

- **Not** Git submodules, `160000` gitlinks, `.gitmodules`, or `clone --recurse-submodules` (D1/D4 stay declined). This RFC does not reintroduce them and must not emit a `160000` entry anywhere.
- **Not** a default/automatic composition model. There is no auto-follow, no lazy-fetch-by-default (`lore.md` §3.5:256). Composition happens only on an explicit `libra subtree` command.
- **Not** a new per-kind SQLite table for provenance (`lore.md` §3.6:268 red line). Provenance rides existing stores.
- **Not** a change to the default semantics of any existing Git-compatible command (`clone`, `commit`, `merge`, `checkout` behave identically when `libra subtree` is never invoked).
- **Not** a live external dependency: the source repository is a *provenance reference*, never required to be present to make the working tree complete.
- **Not** transparent lazy "link" (auto-materialize-on-access, byte-range fetch, live follow of a moving source ref) — those are deferred (see Phased rollout).
- **Not** cross-network / foreign-Git provenance-note travel in v1 (inherits the D17 deferral pattern from deps 3.2).

## Design overview

The v1 slice is **`libra subtree`**, a net-new top-level command family (no such command exists today — confirmed net-new). It has three storage-relevant facts:

1. **Content = ordinary tree entries.** `subtree add`/`pull` write blobs/subtrees under a prefix and produce a normal commit. Materialization reuses the `hydrate` atomic-write discipline (`src/command/hydrate.rs:310` `land()` → `atomic_write::write_atomic` + exec-bit publish), so a failed materialization never leaves a truncated/half-written/wrong-mode file. Whole-object only (no FastCDC byte-range), matching `hydrate` v1.
2. **Provenance = authoritative travel-safe trailers + a derived local snapshot note.** Two layers with a clear, non-overlapping division of labor (this is the load-bearing correction over a naive "notes override trailers" model):
   - **Per-operation TRAILERS (source of truth, travel-free).** Each `subtree add`/`pull`/`remove` bakes an immutable trailer block describing *that one operation* into the commit it creates. Trailers are ordinary commit-message text, so they ride the pack for free and survive `clone` with no side-channel. Because they are per-commit and immutable, they are an append-only **event log**: the trailers on the whole ancestor chain (add → pull → … → remove events) reconstruct the active subtree set at any commit.
   - **Per-commit NOTE (derived cache/index, local).** The optional `refs/notes/subtree` note at commit `C` holds the **full reconciled snapshot** of the active subtree set as of `C` — NOT a per-operation delta. Each subtree operation carries the prior HEAD snapshot forward with its one change and writes the updated full set as the note on the new commit (exactly how `deps` stores the full adjacency doc per commit, not deltas). So `subtree list` is an O(1) read of HEAD's note.
   - **Reconciliation across a DAG (precise).** The note is a *cache* of the trailer event log; when it is absent (a fresh clone — notes do not auto-travel, D17 pattern; or a hand-deleted note), `subtree list` reconstructs the active set. Because HEAD ancestry is a DAG (merges), the reconstruction is defined precisely, not as a vague "walk history":
     1. **Every subtree-set-changing commit writes the full snapshot note.** Each `subtree add`/`pull`/`remove` writes the complete active-set snapshot as the `refs/notes/subtree` note on the commit it creates. A merge that changes the active set (i.e. integrates a subtree op from a side branch) likewise writes a **reconciled** snapshot note plus a summary trailer. So whenever the note is present it is authoritative and unambiguous.
     2. **Fallback (note absent) = first-parent, newest-wins.** Walk **first-parent** ancestry from HEAD (deterministic mainline order) and fold the `Libra-Subtree-*` trailer events newest-to-oldest: the newest event per `prefix` wins — an `add`/`pull` sets/updates that prefix, a `remove` tombstones it. First-parent-only deliberately avoids the ambiguity of merging conflicting events from divergent branches (which traversal order would otherwise decide arbitrarily).
     3. **Merge discipline (documented constraint, not a silent gap).** Since the fallback is first-parent, a subtree op performed on a *side* branch becomes authoritative on the mainline only once a merge (or the carried-forward note) records it on first-parent history — exactly the snapshot carry-forward rule in (1). Subtree ops are expected on the mainline; a merge integrating a side-branch subtree op MUST (re)write the snapshot note so `list` stays exact. There is no "notes win over trailers" precedence conflict: trailers are authoritative, and the note is a derived snapshot that must equal the first-parent trailer reconciliation.
3. **No new table.** Provenance notes ride the existing `notes` store (loose blob + `notes` row, migration `2026061401_notes.sql`) under a reserved ref, owned by a new single-owner module, mirroring `internal::deps::DependencyStore`. **Zero migrations in v1.**

## v1 slice: subtree composition

### CLI surface (sketch)

All new surface; stable `--json`; new `LBR-SUBTREE-*` StableErrorCodes (each needs a `docs/error-codes.md` row). Nothing here alters an existing command's defaults.

```
libra subtree add    --prefix <dir> <source> [<ref>] [--squash] [--message <msg>] [--json]
libra subtree pull   --prefix <dir> [<source>] [<ref>] [--squash] [--json]
libra subtree list   [--prefix <dir>] [--json]
libra subtree split  --prefix <dir> [--branch <name>] [--annotate <prefix>] [--json]
libra subtree remove --prefix <dir> [--json]        # (optional in v1)
```

- **`add`** — resolve `<source>` at `<ref>` (default the source's default branch), fetch its objects, take the subtree at `--source-path` (default source root), and write it under `--prefix` as ordinary tree entries; create one commit; record provenance (trailers on that commit + optional note). `--squash` (v1 default and, for v1, only mode) records only the source's tip commit OID rather than importing source history.
- **`pull`** — advance an existing subtree: fetch newer source content at `<ref>`, and perform a **three-way merge** between (old-source-subtree-tree, new-source-subtree-tree, current-prefix-content-in-HEAD), writing the merged result back under `--prefix`; update provenance (`source_commit` advances). Source and ref default to the recorded provenance.
- **`list`** — enumerate the **active** subtrees at HEAD (prefix, source, source_commit, source_path, mode). Fast path: read the `refs/notes/subtree` snapshot note on HEAD (O(1)). Fallback (note absent — e.g. a fresh clone where notes did not travel): reconcile the trailer event log (add/pull/remove) across the ancestor chain to reconstruct the active set, and optionally rebuild the note. The trailer reconstruction is authoritative; the note must equal it.
- **`split`** — synthesize a standalone history containing **only** the prefix's content, path-rewritten to the subtree root, so the user can push it back to the source or republish it. Purely constructs ordinary commits/trees; **no gitlinks**.
- **`remove`** — delete the prefix content and its provenance in a new commit (history retains the trailers, like any commit message — see Privacy).

### Provenance metadata schema

Two layers, both size-bounded and Git-compatible.

**(1) Travel-safe TRAILER layer** — emitted on the composition/pull commit and parsed directly from the commit message by the landed 1.9 commit-message trailer parser. Note: `MetadataKv::revision_get` (`src/internal/metadata.rs:586`) is **not** reusable here — it is hard-bound to `refs/notes/metadata` (it loads *that* doc and merges it with trailers), so `SubtreeStore` parses the `Libra-Subtree-*` trailer block from the commit message itself and reads its own snapshot from `refs/notes/subtree`. Trailers are ordinary commit-message text; they travel with the pack with zero side-channel and survive `clone` for free:

```
Libra-Subtree-Prefix: vendor/libfoo
Libra-Subtree-Source: https://example.com/foo.git
Libra-Subtree-Source-Commit: 3f2a…<hex OID>
Libra-Subtree-Source-Path:
Libra-Subtree-Mode: squash
```

The source-commit OID is a hex string sized by the active hash kind — it MUST go through the `cli.rs` `core.objectformat` → `set_hash_kind` preflight and must not hard-code 20/32-byte widths (`lore.md` §3.0 hash-format constraint). A `<source>` URL containing embedded credentials MUST be run through `redact::redact_url_credentials` before it is written into a trailer (trailers travel — see Security).

**(2) Local NOTE layer** — an optional per-commit versioned JSON document under a reserved notes ref, owned by a new single-owner `internal::subtree::SubtreeStore`, structurally identical to `internal::deps::DependencyStore` (`src/internal/deps/mod.rs:1-35`). Each note holds the **full reconciled snapshot** of the active subtree set as of its commit (carried forward on each operation), so it is a fast O(1) index for `subtree list` that a fresh clone rebuilds from the trailer event log when absent. It is a *derived cache* of the authoritative trailers — never a competing source of truth — plus optional local-only breadcrumbs that must not travel:

```jsonc
{
  "version": 1,                         // SUBTREE_DOC_VERSION, reject on mismatch
  "subtrees": [
    {
      "prefix": "vendor/libfoo",        // re-validated on read & import (no ../, no abs, no .libra/)
      "source": "https://example.com/foo.git",
      "source_ref": "refs/heads/main",
      "source_commit": "3f2a…",         // hex OID, hash-kind agnostic
      "source_path": "",                // subtree within source; re-validated like prefix
      "mode": "squash",
      "added_at": "2026-07-04T00:00:00Z",
      "last_pull_commit": "3f2a…"
    }
  ]
}
```

Bounds and discipline reused verbatim from deps/metadata:

- Reserved ref: `refs/notes/subtree` (a `notes_ref` string key in the existing `notes` table, **not** a reference-table ref — exactly like `refs/notes/deps`).
- Whole-doc byte bound `MAX_DOC_LEN = 1 << 20` (1 MiB), mirroring `metadata` `MAX_VALUE_LEN` (`src/internal/metadata.rs:44-46`) and `deps` `MAX_DOC_LEN` (`src/internal/deps/mod.rs:49`). Enforced on **both** write and read/import (hand-edited or fetched docs) **before** parsing; over-version or over-size is rejected (`corrupt_doc_error` pattern).
- Absence-tolerant: a missing note reads as an empty set (deps `load_doc`); an empty set **removes** the note (empty == never-written).
- **Single owner:** only `SubtreeStore` touches `refs/notes/subtree`; no command calls `internal::notes` directly for it (deps discipline). Reads/writes go through `internal::notes::add/show/remove` (`src/internal/notes.rs:183/298/334`).
- Read-modify-write lost-update guard: the note write follows the metadata/deps blob compare-and-swap re-verify on the last-entry delete. The trailer layer is immutable, so the travel-safe record can never be lost to a note race.
- **Untrusted-data re-validation:** every `prefix` and `source_path` is re-normalized and re-validated on load *and* on import via a `normalize_edge_path`-style check (`src/internal/deps/mod.rs:121`) — reject absolute paths, `..` escapes, backslashes, empty, and any `.libra/` target — because a hand-edited or fetched note must never inject a traversal path into a materialization consumer.

### Materialization (`add`) flow — on Git tree objects, no gitlinks

1. Resolve `<source>` to a transport client via the existing dispatch — `RemoteClient::from_spec_with_remote(spec, remote)` (`src/command/fetch.rs`), which uniformly applies HTTPS token auth and SSH vault keys; a local Libra source resolves credential-free through `LocalClient::from_path` / `is_libra_source` (`src/internal/protocol/local_client.rs:56`).
2. Fetch the source objects reachable from `<ref>` using the existing pack path (no wire object filtering; honest whole-pack fetch, like `clone --deps-of`). Apply the source-trust caps from Security below.
3. Read the source **commit tree** (not index), walk the subtree at `source_path`, and for each entry re-validate the path against the prefix (reject `..`/absolute/backslash escapes) before joining — this is the guard the RFC owns for the `utils/tree.rs:73` unvalidated-join gap.
4. Build the new tree: graft the source subtree's blobs/subtrees under `--prefix` as ordinary `100644`/`100755`/`040000` entries. `120000` symlink and `160000` gitlink source entries and LFS-pointer blobs are **cleanly skipped-unsupported** in v1 (hydrate precedent), never forced or mis-materialized.
5. Materialize to the working tree using the `hydrate` `land()` discipline (`atomic_write::write_atomic` + post-rename exec-bit, fail-loud on chmod). Any failure leaves the pre-existing worktree untouched.
6. Create one commit whose tree contains the grafted subtree; attach the provenance trailers; update the `refs/notes/subtree` note via `SubtreeStore`.

Because step 4 emits only ordinary entries, the resulting commit is a normal Git commit: it round-trips, merges, and rebases with no special handling, and old Libra/Git reads it as plain files.

### Update (`pull`) flow

`pull` is a three-way merge over ordinary trees, never a gitlink update:

- base = source subtree tree at the recorded `source_commit`;
- theirs = source subtree tree at the new `<ref>`;
- ours = current content under `--prefix` in HEAD.

Merge the three over ordinary tree entries, write the result under `--prefix`, commit, and advance `source_commit` in both provenance layers. Local edits under the prefix (ours) are preserved by the three-way merge; conflicts surface through the normal merge conflict path. `--squash` records only the new tip OID; a future non-squash mode (deferred) would import source history.

**Reuse honesty:** the tree-item three-way primitives already exist — `merge_tree_items` (`src/command/merge.rs:1921`) and stash's `merge_trees` (`src/command/stash.rs:1967`) — but both are **private** and operate over full trees, so `pull` cannot call them drop-in; v1 needs a small extraction of a prefix-scoped three-way tree merge (or a thin wrapper that restricts the merge to the `--prefix` subtree). This is a bounded, feasible refactor, not a new merge engine.

### Split flow

`split` reconstructs a synthetic history whose trees contain only the prefix's content, path-rewritten to the root, so it can be pushed back to the source. It builds ordinary commits/trees exclusively — there is no gitlink and no foreign object graph involved. The concrete builder is `write_tree_from_leaves` (`src/internal/tree_plumbing.rs:82`) plus normal commit construction; a squash-equivalent single synthetic commit is clearly feasible in v1, while full commit-by-commit split (replaying only the prefix's touching commits) is a later phase.

### Source access & auth (reused, no new surface)

- Remote sources reuse `RemoteClient::from_spec_with_remote` + `internal::auth::HostScope::from_request_url` / `lookup` (HTTPS host-scoped token, `src/internal/protocol/https_client.rs:87`) and `configure_ssh_client` / `try_load_vault_ssh_key_for_remote` (SSH vault key → 0600 temp key, `src/command/fetch.rs:223`). No new credential surface; the auth host-scope trust boundary is inherited unchanged.
- Local Libra sources are credential-free (`LocalClient`, no host/port/token consulted).
- Non-TTY callers never hit an interactive auth prompt (existing fail-fast-with-`libra auth login` behavior).

### Optional scoping via `sparse` (2.2)

For a large composed subtree, `sparse` (2.2, `src/internal/sparse/mod.rs`) can scope a **read-only view** of the prefix (what `ls-files`/`diff` output), exactly as `clone --deps-of` stores its closure as a `SparseViewStore` view. This is an output filter only — it never mutates the worktree, never writes skip-worktree bits, and never narrows the commit set (D10). Composed content itself is always fully materialized ordinary tree content (there is no skip-worktree bit, and `commit` builds the tree from a full index, so a narrowed worktree would drop files at commit — the same reason `deps` 3.2 keeps a commit-safe full checkout).

### Reused seams (summary)

| Need | Reused seam |
|---|---|
| Provenance (travel-safe, authoritative) | 1.9 commit-message trailer parser, parsed directly from the commit message (**not** `MetadataKv::revision_get`, `metadata.rs:586`, which is bound to `refs/notes/metadata`) |
| Provenance (local snapshot cache) | `internal::notes` store + new single-owner `SubtreeStore` mirroring `internal::deps::DependencyStore` (`src/internal/deps/mod.rs`); reserved `refs/notes/subtree`; each note = full carry-forward snapshot |
| Doc bounds / re-validation / absence-tolerance | deps/metadata `MAX_DOC_LEN`, `normalize_edge_path` (`deps/mod.rs:121`), `load_doc` pattern |
| Atomic materialization | `atomic_write::write_atomic` + post-rename exec-bit (`src/utils/atomic_write.rs`) — the discipline `hydrate::land()` uses; `land()` (`hydrate.rs:310`) is **private**, so v1 shares it via a small extracted helper or calls `write_atomic` directly |
| Three-way tree merge (`pull`) | `merge_tree_items` (`merge.rs:1921`) / `merge_trees` (`stash.rs:1967`) — both **private**; v1 extracts a prefix-scoped wrapper (bounded refactor) |
| Split builder | `write_tree_from_leaves` (`src/internal/tree_plumbing.rs:82`) + normal commit construction |
| Object fetch + verify | `ClientStorage::get` (`src/utils/client_storage.rs:614`), `verify_fetched_object` (`src/utils/storage/tiered.rs:39`) |
| Source access + auth | `RemoteClient::from_spec_with_remote`, `auth::lookup`, `configure_ssh_client`, `LocalClient` (credential-free local) |
| Optional scoping | `SparseView` / `SparseViewStore` (`src/internal/sparse/mod.rs`) |
| Provenance-note travel (deferred, extension) | Modeled on 3.2's side-channel but NOT a drop-in: `export_deps_notes`/`import_notes` (`local_client.rs:270`, `fetch.rs`) are **deps-TYPED** (enumerate `refs/notes/deps`, decode `DepsDoc`); a subtree-typed `export/import_subtree_notes` analog would be needed. v1 avoids this entirely — trailers travel for free. |
| Tree entry modes | `TreeItemMode::Blob | BlobExecutable | Tree` (`src/command/ls_tree.rs:671`); `src/internal/tree_plumbing.rs:228` — **`Commit`/`160000` intentionally never emitted** |

## §3.0.1 mandatory gate

> Per `docs/development/gap/lore.md:138`, every numbered item must fill and pass this gate before work starts; any blank cell means the feature may ship only as a feature-gated experiment. §5.1's global gate is inherited by default.

### (A) Four-face compatibility matrix (no bare N/A)

| Compatibility face | Answer | Reject-criterion status |
|---|---|---|
| **Git disk format** (objects/index/pack/refs/LFS pointer) | **No new on-disk field.** Composed content is ordinary `100644`/`100755` blobs and `040000` subtrees under a prefix — standard Git tree objects that round-trip losslessly. Provenance is (1) ordinary commit-message trailers (Git already permits arbitrary trailers) and (2) an optional loose blob + `notes` row under `refs/notes/subtree` (existing `notes` store, migration `2026061401`). New repos are readable by old Libra (composed tree = plain files; the extra notes ref and the trailers are inert to an old binary). Old repos are readable by new Libra trivially. | **PASS** — no `160000` gitlink emitted; no private/unparseable field injected into any object/index/pack/ref/LFS pointer. |
| **Git wire protocol** (smart-HTTP/SSH/git://, standard LFS) | **No new wire message; no capability negotiation.** Composed tree content travels as ordinary pack objects over existing transports. Provenance **trailers** travel baked into the commit (standard). Provenance **notes** do **not** auto-travel (`refs/notes/*` default OFF, Git parity) — old-client↔new-remote and new-client↔old-remote both see ordinary commits/trees. `subtree add` fetches source objects via the existing pack path — and because that leg pulls from an **untrusted source repository**, it is governed by the Security cell's fetch guards (OID-verify before materialization + object count/size budgets), not just "ordinary interop". Optional note-travel is deferred to a subtree-typed side-channel modeled on 3.2's `--notes` (LibraRepo↔LibraRepo local only). Not N/A: source fetch and content travel both exercise the wire, and the answer is "unchanged / ordinary transport, with source-trust guards on the fetch leg". | **PASS** — no standard Git/LFS interop broken. |
| **SQLite schema / migration** | **v1 adds NO new table or column.** Provenance notes ride the existing `notes` table (`2026061401_notes.sql`); provenance trailers ride the commit message; `SubtreeStore` is a new **owner module**, not a new table. Therefore v1 does **not** bump `schema_version`, has nothing to migrate, and needs no `_down.sql`. Not a bare N/A — this is a deliberate "no schema change" with justification. *If* a later phase adds an optional projection/index table, it must pay the full ritual: idempotent forward DDL + matching `*_down.sql`, a `builtin_migrations()` entry with strictly-increasing `YYYYMMDDNN` version, and the SIX-site update in `tests/db_migration_test.rs` (versions vec, names vec, `len()` assertion, `max_registered_version`, builtin-registry vec, both rollback-order vecs) plus the ordered lists in `tests/agent_capture_migration_test.rs`. | **PASS** — v1 has a `_down` obligation only if/when a table is added (none in v1); "old DB opens" is trivially safe (no version bump). |
| **CLI / public API** | **All net-new surface.** New `libra subtree add/pull/split/list[/remove]` with stable `--json` schemas, stable exit codes, and new `LBR-SUBTREE-*` StableErrorCodes (each with a `docs/error-codes.md` row + `compat_error_codes_doc_sync` guard). Ships with a `SUBTREE_EXAMPLES` `after_help` constant and a Command-Groups row (three help-EXAMPLES compat guards). **No existing Git-compatible command changes default semantics** — `clone`/`commit`/`merge`/`checkout` behave identically when `libra subtree` is never invoked. | **PASS** — no default semantics of an existing Git-compatible command changed. |

### (B) Named phased migration

Mandatory because subtree composition touches worktree semantics (materializes content) and persistence (provenance notes). Because v1 introduces **no schema bump and no destructive old-path removal**, the phases govern the experimental→stable rollout and the optional note-travel side-channel rather than a data migration.

| Phase | old-repo × new-binary (read/write) | new-repo × old-binary (read/write) | Rollback trigger | Recoverable state after rollback |
|---|---|---|---|---|
| **灰度 / feature-gate (default off)** | New binary reads any repo unchanged; `libra subtree` gated behind an experimental flag/feature so it is inert unless explicitly enabled. Write: only when explicitly invoked. | Old binary reads the repo fully: composed content is ordinary tree content; provenance trailers are inert commit-message text; `refs/notes/subtree` is ignored. Old binary can still **write** (commit/merge) safely — no `schema_version` bump blocks it. | Disable the feature gate. | Composed content **persists** (it is real committed tree content); provenance trailers persist in history; the local note is simply unused. No data loss, no corruption. |
| **早期过渡 / default on, old path retained** | Command enabled by default; `SubtreeStore` note index active; note-travel side-channel remains opt-in (`--notes`, default OFF). Old materialization/read paths untouched. | Unchanged from above: old binary still reads composed content as plain files; still writes safely; still ignores the notes ref/trailers. | Config/flag to disable `libra subtree`; or unset `subtree.*` note-travel opt-in. | Composed content and trailers persist; note index may be dropped (rebuildable from trailers). |
| **默认启用 / default on, old path removed** | There is **no legacy path to remove** (net-new feature, no schema swap). This phase is "stabilize": promote out of the experimental gate; optionally stabilize the note-travel side-channel. | Unchanged: old binaries keep reading composed repos as ordinary content. If — and only if — a later phase adds an index table with a `schema_version` bump, the backward-compat hard rule below activates. | Revert to 早期过渡 gating. | Fully recoverable; composed content is Git-native and independent of the feature. |

**Backward-compat hard rule (committed):** if any later phase bumps `schema_version` (e.g. an optional provenance index), an old binary opening that higher-`schema_version` repo must **read-only-pass** pure-read commands (`status`/`log`/`diff`) with a version warning and return an actionable "please upgrade libra" error (not a panic) on writes; and a new interop test "old binary opens new-schema repo" must be added. In v1 this is vacuously satisfied because no version bump occurs — old binaries read *and* write composed repos safely, since the composed content is ordinary Git tree content.

### (C) Security / Privacy / Assumptions / Risks / Alternatives (no bare N/A)

**Security.** The RFC **does change the trust surface** for one operation: `subtree add`/`pull` materialize content from an **UNTRUSTED source repository** into the working tree. This is the threat model the RFC explicitly owns:

- *Path traversal.* `utils/tree.rs:73` joins tree-entry names to a path with no `..`/absolute validation today. Subtree materialization MUST re-validate **every** source tree-entry path against the prefix before joining (reject `..`, absolute, backslash, `.libra/`, `.libraignore`/`.gitignore`, and worktree escapes), reusing the `normalize_edge_path` (`deps/mod.rs:121`) / `hydrate` `land()` destination-safety pattern. Persisted/fetched provenance `prefix`/`source_path` are re-validated on read and import (never trust stored data — deps Codex P1 precedent).
- *Resource exhaustion.* `LocalClient` fetch buffers the whole pack in memory with no object count/size cap. Subtree fetch MUST impose an explicit, **bounded** budget — proposed: config keys `subtree.maxSourceObjects` (default **1,000,000** objects) and `subtree.maxSourceBytes` (default **2 GiB** received-pack), layered on the global 0.9 resource limits (`--max-connections`, object-count/size caps) — and deny beyond budget with an actionable `LBR-SUBTREE-*` error rather than OOM. (The exact defaults are a Decisions-needed item, but v1 must ship a concrete non-infinite cap, not an open budget.)
- *Credential leakage.* A `<source>` URL may embed `user:token@host`. It MUST be passed through `redact::redact_url_credentials` before being stored in a **trailer** (trailers travel with the pack) and before any log/error/JSON. Plaintext tokens/keys never appear in provenance. Source access reuses the auth host-scope trust boundary (`internal::auth`) unchanged — a stored token attaches only to a matching normalized `host:port` over HTTPS; cross-host requests never see it.
- *Integrity.* Fetched source objects are OID-verified before materialization (`verify_fetched_object`, `tiered.rs:39`), same as `hydrate` borrowed/remote hits.

**Privacy.** Provenance **trailers travel** with the commit and are visible to any peer/server that receives the pack: they reveal the source URL, source commit OID, and subtree path. This is intentional (it is how a later `pull` locates the source) but MUST be documented — a **private-source URL becomes visible** to anyone who obtains the composed repo, so the trailer stores a **redacted** URL and users are warned. Provenance **notes are local-only by default** (they do not auto-travel), so any more-sensitive local breadcrumb stays local. Deletion/redaction: `subtree remove` drops content + provenance in a new commit, but historical trailers persist in prior commit messages exactly like any commit text — removing them from history requires a history rewrite (`filter`/rebase), which is called out; the local note is deletable immediately.

**Assumptions** (each `*invalidated if:*`):

- The source subtree is expressible as ordinary Git tree entries. *invalidated if:* it contains `160000` gitlinks, `120000` symlinks, or LFS pointers we cannot resolve — v1 cleanly skips those (hydrate precedent) rather than mis-materialize.
- Provenance trailers are sufficient to locate the source for a later `pull`. *invalidated if:* the source moves or disappears — `pull` degrades to an actionable "source unreachable" error; already-materialized content is unaffected.
- The existing `notes` store, trailer parser, and `hydrate`/`sparse` seams remain stable single-owner surfaces. *invalidated if:* metadata/notes ownership or the trailer scheme changes — then `SubtreeStore` must be re-pinned to the new owner API.
- No `schema_version` bump in v1. *invalidated if:* a phase adds an index table — then the (B) hard rule and the six-site migration ritual apply.

**Risks** (each `*mitigation:*`):

- Malicious source tree attempts path traversal. *mitigation:* re-validate every entry path on materialization and every stored `prefix`/`source_path` on read/import; reject escapes; reuse `normalize_edge_path` + `land()` guards.
- Huge source causes memory exhaustion. *mitigation:* object count/size budgets on the fetch; actionable deny beyond budget.
- Provenance drift or lost update on concurrent note writes. *mitigation:* blob compare-and-swap re-verify (metadata/deps pattern); immutable trailer layer is the travel-safe source of truth and cannot be lost to a note race.
- Users mistake subtree for a submodule (expect a live link). *mitigation:* documentation + `subtree list` shows plain in-repo content; never emit `160000`; the source is a provenance reference, not a live pointer.
- Squash `pull` three-way merge mishandles local edits under the prefix. *mitigation:* reuse the existing merge machinery over ordinary trees; conflicts surface through the normal conflict path; content is committed content so `diff`/`status` stay honest.

**Alternatives Considered** (≥2, each with a concrete rejection):

1. **Git submodules / `160000` gitlink pointer.** *Rejected on the D1 product boundary.* Libra is single-repo/trunk-based and stores no foreign-repo commit objects, so a `160000` gitlink is structurally unresolvable — it is already actively rejected by `rebase`/`merge` and skipped by `archive`/`rev-list`/`bundle` (`fast-export` preserves the records for round-trip fidelity), and `push` only warns "submodule is not supported yet". A submodule is a multi-repo **live pointer**, exactly what D1 declines; this RFC is the "clear RFC" D1 names as the reopen gate, and it reopens the *need* by offering a monorepo-native alternative, **not** by reintroducing the pointer. D4 (`clone --recurse-submodules`) stays declined in sync.
2. **Extend the local `layer` (2.4) primitive to enter commits.** *Rejected on the layer red line.* `layer`'s load-bearing invariant is **never-enters-commit**, enforced at two chokepoints; making it versioned would break that invariant and conflate a local overlay with in-history content. `layer` and `link`/`subtree` are deliberately the two halves of the composition pair (local vs versioned, `layer/mod.rs:3-7`); the versioned half must be a **separate** primitive.
3. **A new per-subtree SQLite table for provenance.** *Rejected on §3.6:268* ("no per-metadata-kind table"). A table adds a coherency window and a migration/rollback cost the design avoids; provenance instead rides the existing `notes` store + commit trailers, computed in memory from a self-contained size-bounded doc — the exact resolution `deps` and `metadata` already adopted.
4. **Materializing sparse-checkout to lazily fetch subtree content on access.** *Rejected for v1.* Materializing sparse is deferred (D10); Libra has no skip-worktree bit and `commit` builds the tree from a full index, so a narrowed worktree would silently drop the subtree at commit. Composed content must be fully materialized ordinary tree content (whole-object, like `hydrate` v1). Lazy/live "link" semantics are deferred to a separately-gated later phase.

## Phased rollout

- **Phase A (this RFC → acceptance).** Fill and review this §3.0.1 gate. No code lands until accepted; any prototype is feature-gated only.
- **Phase B (v1, experimental gate).** `subtree add` + `subtree list` + `subtree pull` (squash) + `subtree split` (squash-equivalent), behind an experimental gate. Provenance = trailers (authoritative) + `SubtreeStore` local note index. Zero migrations. Source-trust guards (path re-validation, size caps, credential redaction) mandatory. L1 tests: add/list/pull/split round-trips, `list` fast-path (HEAD note) **and** fallback (note deleted → first-parent trailer reconciliation reproduces the same set), **including a merge-commit case** (a subtree op on a side branch that a merge records via a reconciled snapshot note, then note-deleted → first-parent reconciliation still yields the correct active set), malicious-path rejection, credential redaction, skip-unsupported (gitlink/symlink/LFS), atomic-failure-leaves-worktree-intact, source-fetch budget-exceeded rejection, and — the (B) hard requirement — an **old-binary-reads-new-subtree-repo interop test** (a repo containing composed subtree content + `Libra-Subtree-*` trailers + a `refs/notes/subtree` row is read *and* committed to by a binary with no subtree support, proving the content is inert ordinary tree data).
- **Phase C (stabilize).** Promote out of the gate; optionally wire the 3.2-style `--notes` side-channel for provenance-note travel (LibraRepo↔LibraRepo local only, default OFF, Git parity; cross-network/foreign-Git/push deferred per the D17 pattern).
- **Later phases (separately gated).** Non-squash full-history subtree merge and commit-by-commit `split`; richer "link" semantics (auto-follow a moving source ref, lazy materialize-on-access, byte-range) — these require their own RFC/gate and are **explicitly out of scope** here. Any of them that touches persistence would pay the full migration ritual and the (B) backward-compat hard rule.

## Decisions needed / Open questions

- **Trailer naming & git-subtree interop.** `Libra-Subtree-*` (proposed) vs also emitting stock `git-subtree-dir`/`git-subtree-split` trailers so `git subtree` can interoperate. Interop is attractive but couples us to git-subtree's format.
- **Squash-only v1 vs full-history in v1.** v1 proposes squash-only (record tip OID). Full-history import (non-squash) is heavier and deferred — confirm this scoping.
- **Provenance-note travel in v1 or Phase C.** Trailers travel for free; the note side-channel (3.2 pattern) is proposed for Phase C. Confirm v1 relies solely on trailers for cross-machine `pull`.
- **`SubtreeStore` note as authoritative vs index-only.** Proposed: trailer authoritative, note is a rebuildable local index. Confirm we never make the note the source of truth (avoids a coherency window).
- **Source fetch strategy.** Reuse the full clone/fetch pack path vs a scoped fetch of only the source subtree's reachable objects. Full pack is simpler and honest (like `clone --deps-of`); scoped fetch needs care to stay commit-safe.
- **`subtree remove` in v1.** Include now, or defer? Historical-trailer persistence (Privacy) may argue for a documented rewrite path instead.
- **New `LBR-SUBTREE-*` error codes.** Enumerate the set (unreachable source, path-traversal-rejected, unsupported-entry-skipped-summary, budget-exceeded, prefix-conflict) and register each in `docs/error-codes.md`.
