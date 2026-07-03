//! Path builders for repository storage: index, objects, database, hooks, and attributes locations relative to the working directory.

use std::{io, path::PathBuf};

use crate::utils::util;

pub fn index() -> PathBuf {
    // lore.md 2.1: the index is PER-WORKTREE — it lives in the local gitdir,
    // not the shared/common storage (db/objects stay shared).
    util::worktree_gitdir().join("index")
}

pub fn try_index() -> io::Result<PathBuf> {
    Ok(util::try_get_worktree_gitdir(None)?.join("index"))
}

pub fn objects() -> PathBuf {
    util::storage_path().join("objects")
}

pub fn try_objects() -> io::Result<PathBuf> {
    Ok(util::try_get_storage_path(None)?.join("objects"))
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
