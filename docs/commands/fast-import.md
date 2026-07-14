# `libra fast-import`

Import a Git fast-import stream into a Libra repository. It is the natural
counterpart to [`fast-export`](fast-export.md).

## Synopsis

```text
libra fast-import [--input <file>] [--max-count <n>] [--quiet]
```

## Description

Supported directives include:

- `blob` with marks and counted or delimited `data`;
- `commit <ref>` with `mark`, `author`, `committer`, message `data`, `from`,
  `merge`, `M`, `D`, `C`, `R`, `N`, and `deleteall`;
- inline blob data in `M ... inline` and `N inline`;
- annotated `tag` records (including optional marks and commit/tree/blob/tag
  targets);
- `reset <ref>` with `from`, or ref deletion when `from` is omitted;
- `checkpoint`, `done`, and lenient `feature`/`option`/`progress` preambles.

Paths accept Git C-style quoting and are rejected if they are absolute, empty,
traversing, or not valid UTF-8 for Libra's tree model. Commit/tag messages must
also be UTF-8 and fail rather than being lossily changed. `M` validates that the
file mode matches the referenced object type, preventing corrupt tree entries.

Branches and tags are persisted in their normal ref stores. `refs/notes/*`
commits are translated to Libra's notes table from either `N` records or Git's
tree-shaped notes commits. Other ref namespaces fail closed.

### Transaction model

Objects are written while parsing. Branch, tag, and notes changes are buffered
and published together in one SQLite transaction at `checkpoint`, `done`, or a
clean EOF. A failure before publication changes no refs or notes; any already
written objects are unreachable and can be reclaimed with `libra fsck` followed
by `libra gc`. A `checkpoint` intentionally makes the preceding batch durable.

### Safety and resource limits

- Total input defaults to 1 GiB and is controlled by
  `fastimport.maxInputSize`; unreadable, invalid, or zero values fail closed.
- A command/header line is limited to 1 MiB before allocation grows further.
- Top-level blob, commit, and tag objects default to a 1,000,000-object limit;
  `--max-count` changes it. Derived trees are not counted separately.
- Literal object IDs must match the repository's SHA-1/SHA-256 format, marks
  must be unique, and referenced objects must exist with the required type.

## Options

| Option | Description |
|---|---|
| `--input <file>` | Read from a file instead of stdin. |
| `--max-count <n>` | Set the top-level imported-object ceiling. |
| `--quiet` | Suppress the final human summary. |

The import protocol remains raw even when global JSON/machine flags are set;
use `--quiet` when another program consumes stdout.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | The stream completed and its final batch was published. |
| `128` | Malformed/unsupported input, invalid config/ref/path/object, a limit, repository, transaction, or IO failure. |

## Examples

```bash
libra fast-export --all | libra fast-import --quiet
libra fast-import < repository.fi
libra fast-import --input repository.fi --max-count 2000000
```

## Comparison with Git

| Task | Libra | Git |
|---|---|---|
| Import a stream | `libra fast-import` | `git fast-import` |

Deferred protocol commands include `cat-blob`, `ls`, and `get-mark`; marks-file
import/export and truly streaming multi-GiB object payloads are also deferred.
