# `libra fork`

Fork the current ScorpioFS-backed worktree into a new independent worktree.

## Synopsis

```text
libra fork <PATH>
```

## Description

`fork` requires the current worktree to be backed by ScorpioFS. It registers the
new Libra linked worktree without materializing the committed tree, asks
ScorpioFS to materialize the source upper layer into the new mount, binds the
same base revision, and writes the new worktree pointer and private index through
the mounted filesystem.

The fork uses the `materialize` strategy. The parent and child receive separate
upper layers; later writes in one worktree are not visible in the other.

The daemon endpoint is selected with `LIBRA_SCORPIOFS_ENDPOINT` (default
`http://127.0.0.1:2725/antares`). The Mega path is selected with
`LIBRA_SCORPIOFS_REPO_PATH` (default `/project`).

## Argument

### `<PATH>`

Empty or nonexistent target path for the new linked worktree.

```bash
libra fork ../experiment
```

## Ownership

ScorpioFS owns mount, lower projection, and writable upper layers. Libra owns the
linked-worktree registry, HEAD/index scope, objects, commit, refs, and transport.
The fork command does not copy Git objects or create a second object store.

## Current limitations

- only ScorpioFS-backed source worktrees are supported;
- `materialize` is the implemented mode; frozen-layer `chain` is a later optimization;
- the source must already have a bound `base_revision` and a persisted ScorpioFS
  mount id;
- a detached source is not a valid sync target until it is attached to a branch.
