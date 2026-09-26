# `libra sync`

Stage, commit, and push the current worktree in one operation.

## Synopsis

```text
libra sync [-m <MESSAGE>]
```

## Description

`sync` is Libra's compact workflow for a mounted worktree, including a
ScorpioFS-backed worktree:

1. stage the complete worktree (`add -A`),
2. create one commit, and
3. push the current branch back to the repository's `origin` remote.

The command does not implement filesystem semantics or Git object storage itself;
it delegates staging, commit construction, and transport to Libra's existing
commands. A worktree must have an attached branch. For Mega2's trunk workflow,
the current branch is pushed to `refs/heads/main`, because Mega2 rejects public
branch updates outside `main`.

When `-m` is omitted, Libra uses `Sync ScorpioFS worktree` as the generated commit
message. This makes the command non-interactive and suitable for an agent run.

## Options

### `-m, --message <MESSAGE>`

Commit message for the generated commit.

```bash
libra sync -m "Update generated sources"
libra sync
```

## ScorpioFS integration

For a ScorpioFS mount, Libra discovers the linked worktree through the mounted
`.libra/commondir`, `.libra/worktree_id`, and private `.libra/index` files. The
ScorpioFS daemon supplies the POSIX view and reports upper-layer changes; Libra
remains the owner of the index, objects, refs, commit, and push.

The mount must advertise `whiteout.oci.v1` before a sync that may delete files.
Otherwise the caller must fail closed rather than silently omit remote deletions.

## Failure modes

- a detached HEAD is rejected; attach a branch with `libra switch -c <branch>`;
- missing author identity is reported by `commit`;
- a remote rejection is returned by `push` without suppressing the original error;
- no changes are handled by the underlying commit command rather than creating
  an empty commit.

## Git comparison

Git has no single `sync` command with this exact contract. Use `git add -A`,
`git commit`, and `git push` separately when working with a Git repository.
