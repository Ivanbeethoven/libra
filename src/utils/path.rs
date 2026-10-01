//! Path builders for repository storage: index, objects, database, hooks, and attributes locations relative to the working directory.

use std::{future::Future, io, path::PathBuf};

use crate::utils::util;

tokio::task_local! {
    static INDEX_OVERRIDE: PathBuf;
}

/// Run `future` with a task-local index path used by all nested index readers.
///
/// This is intentionally crate-private and scoped to one async task, so dry-run
/// consumers can use an isolated index without environment variables or
/// cross-command/global state.
pub(crate) async fn with_index_override<T>(
    index_path: PathBuf,
    future: impl Future<Output = T>,
) -> T {
    INDEX_OVERRIDE.scope(index_path, future).await
}

pub fn index() -> PathBuf {
    if let Ok(index_path) = INDEX_OVERRIDE.try_with(Clone::clone) {
        return index_path;
    }
    // lore.md 2.1: the index is PER-WORKTREE. For a LINKED worktree it lives in the
    // COMMON storage's `worktrees/<id>/` — the same split git uses
    // ($GIT_COMMON_DIR/worktrees/<id>/index) — which matters most on a ScorpioFS
    // worktree, where the worktree's own `.libra` sits INSIDE the FUSE mount: the
    // index is ~19 MB at 174k entries, measured at 290 ms per read and several
    // hundred ms per write through the mount.
    linked_index_for(util::worktree_gitdir())
}

pub fn try_index() -> io::Result<PathBuf> {
    if let Ok(index_path) = INDEX_OVERRIDE.try_with(Clone::clone) {
        return Ok(index_path);
    }
    Ok(linked_index_for(util::try_get_worktree_gitdir(None)?))
}

/// Resolve the per-worktree index path (see [`index`]). Infallible and idempotent:
/// a linked worktree whose index still sits in its own gitdir (created before this
/// split) is migrated once, on first touch.
fn linked_index_for(gitdir: PathBuf) -> PathBuf {
    let Ok(common) = std::fs::read_to_string(gitdir.join("commondir")) else {
        return gitdir.join("index");
    };
    let common = common.trim();
    if common.is_empty() {
        return gitdir.join("index");
    }
    let common = if std::path::Path::new(common).is_absolute() {
        PathBuf::from(common)
    } else {
        // git writes this relative to the gitdir
        gitdir.join(common)
    };
    let Ok(id) = std::fs::read_to_string(gitdir.join("worktree_id")) else {
        return gitdir.join("index");
    };
    let id = id.trim();
    if id.is_empty() {
        return gitdir.join("index");
    }
    let dir = common.join("worktrees").join(id);
    let new_index = dir.join("index");
    if !new_index.exists() {
        let old_index = gitdir.join("index");
        if old_index.exists() {
            // One-time migration for worktrees created before the split; a rename
            // through the mount is still cheaper than every later read paying it.
            if std::fs::create_dir_all(&dir).is_ok()
                && std::fs::rename(&old_index, &new_index).is_err()
            {
                let _ = std::fs::copy(&old_index, &new_index);
            }
        }
    }
    let _ = std::fs::create_dir_all(&dir);
    new_index
}

pub fn objects() -> PathBuf {
    util::storage_path().join("objects")
}

pub fn try_objects() -> io::Result<PathBuf> {
    Ok(util::try_get_storage_path(None)?.join("objects"))
}

/// Shared/common repository storage that owns the commit-preview quota.
pub(crate) fn try_preview_scratch_storage() -> io::Result<PathBuf> {
    util::try_get_storage_path(None)
}

/// FastCDC media chunk store root (lore.md §6): a physical SIBLING of
/// `objects/`, wholly outside the Git object graph. Content-addressed chunk
/// files live under `media/chunks/<ab>/<chunk_hash>`; it is NEVER walked as a
/// loose-object store. Gated behind the `fastcdc` feature at the call sites.
#[cfg(feature = "fastcdc")]
pub fn media_chunks() -> PathBuf {
    util::storage_path().join("media").join("chunks")
}

/// FastCDC media manifest store root (lore.md §6): content-addressed manifest
/// JSON files under `media/manifests/<media_oid>.json`. Sibling of `objects/`,
/// outside the Git object graph.
#[cfg(feature = "fastcdc")]
pub fn media_manifests() -> PathBuf {
    util::storage_path().join("media").join("manifests")
}

pub fn database() -> PathBuf {
    util::storage_path().join(util::DATABASE)
}

pub fn hooks() -> PathBuf {
    util::storage_path().join("hooks")
}

pub fn attributes() -> PathBuf {
    util::working_dir().join(util::ATTRIBUTES)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn preview_scratch_uses_common_storage_for_linked_worktree() {
        let root = tempfile::tempdir().expect("create linked-worktree fixture");
        let common = root.path().join("main/.libra");
        let linked = root.path().join("linked");
        let linked_gitdir = linked.join(".libra");
        fs::create_dir_all(common.join("objects")).expect("create shared object store");
        fs::create_dir_all(&linked_gitdir).expect("create linked worktree gitdir");
        fs::write(
            linked_gitdir.join("commondir"),
            common.to_string_lossy().as_bytes(),
        )
        .expect("write commondir");
        fs::write(linked_gitdir.join("worktree_id"), b"linked-test\n").expect("write worktree id");
        let _cwd = crate::utils::test::ChangeDirGuard::new(&linked);

        assert_eq!(
            try_preview_scratch_storage().expect("resolve shared preview scratch"),
            common.canonicalize().expect("canonicalize shared storage")
        );
    }
}
