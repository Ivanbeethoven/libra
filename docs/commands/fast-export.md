# `libra fast-export`

Emit selected history as a Git fast-import stream. The command is read-only: it
never changes objects, refs, the index, or the worktree.

## Synopsis

```text
libra fast-export [--all] [<rev>...]
```

## Description

`fast-export` writes commits in parent-before-child order with one shared mark
table. With no arguments it exports `HEAD`. A branch or tag keeps its real ref
name; a raw revision uses a synthetic `refs/heads/exported-N` ref.

Supported selection and fidelity:

- multiple revisions in one stream;
- `A..B` and `^A` exclusions for incremental streams (excluded parents are
  emitted as literal prerequisite object IDs);
- `--all` for all local branches and tags plus Libra notes mappings;
- lightweight and annotated commit tags, including the annotated tag object;
- notes represented as valid fast-import `N` records;
- Git C-style quoting for spaces, controls, quotes, backslashes, and UTF-8 path
  bytes;
- shared blob/commit/tag marks and a final `done` directive.

Each commit is encoded as `deleteall` plus its complete `M` file list. This is
larger than Git's parent-diff stream, but reconstructs the same tree. Commit
signing headers are omitted because fast-import commit records cannot represent
them; annotated-tag messages remain intact.

`--all` fails closed if a stored note's target cannot receive a stream mark
(for example, it is outside the selected history). Export an appropriate ref
set rather than accepting a stream that would silently lose the note.

## Options

| Option | Description |
|---|---|
| `<rev>...` | Revisions, `A..B` ranges, or `^A` exclusions. Defaults to `HEAD`. |
| `--all` | Export all local branches and tags, plus notes whose targets are in that graph. |

The output is always the raw stream on stdout. Global JSON/machine flags do not
wrap protocol bytes.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | The complete stream was written. |
| `128` | Repository, revision, object, note-closure, or output failure. |

## Examples

```bash
# Save every branch, tag, and reachable note
libra fast-export --all > repository.fi

# Export two refs with shared marks
libra fast-export main topic > selected.fi

# Create an incremental stream whose base must already exist at import time
libra fast-export v1..main > since-v1.fi

# Import into another repository
libra fast-export --all | libra fast-import --quiet
```

## Comparison with Git

| Task | Libra | Git |
|---|---|---|
| Export selected refs | `libra fast-export main topic` | `git fast-export main topic` |
| Export local refs | `libra fast-export --all` | `git fast-export --all` |

Deferred surfaces include symmetric `A...B`, marks files
(`--import-marks`/`--export-marks`), `--anonymize`, path/blob filtering, and
annotated tags whose final target is not a commit.
