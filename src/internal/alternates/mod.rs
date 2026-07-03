//! Object alternates (lore.md 2.3) — borrow objects from a shared/parent
//! object store instead of copying them.
//!
//! This module is the SOLE reader/writer of two git-standard on-disk files
//! under a repo's `objects/info/` dir (§3.6 single-owner; a plain file, so no
//! lazily-created SQLite table, and portable to plain `git` and old Libra
//! binaries):
//!
//! - `alternates` — newline-separated OBJECT-DIRECTORY paths this store borrows
//!   FROM (absolute, or relative to this `objects/` dir; `#` comments / blanks
//!   skipped). The read-resolver consults these on a local miss.
//! - `borrowers` — newline-separated object-dir paths that borrow FROM this
//!   store (a Libra extension git does not have). This is the KEYSTONE of
//!   deletion safety: while this file names any live borrower, `gc` /
//!   `cache evict` REFUSE to prune loose objects, so a shared base can never
//!   delete an object a borrower still needs (the row's 绝不删 requirement,
//!   airtight — see [`has_live_borrowers`]).

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

/// Transitive-alternates recursion backstop.
const MAX_DEPTH: usize = 5;

fn alternates_file(objects_dir: &Path) -> PathBuf {
    objects_dir.join("info").join("alternates")
}

fn borrowers_file(objects_dir: &Path) -> PathBuf {
    objects_dir.join("info").join("borrowers")
}

/// Parse one on-disk list file into resolved absolute object-dir paths (a
/// relative entry is joined to `objects_dir`). Missing file → empty. Comment
/// (`#`) and blank lines are skipped.
fn read_list(path: &Path, objects_dir: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let p = PathBuf::from(line);
            if p.is_absolute() {
                p
            } else {
                objects_dir.join(p)
            }
        })
        .collect()
}

fn write_list(path: &Path, entries: &[PathBuf]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = String::new();
    for entry in entries {
        body.push_str(&entry.to_string_lossy());
        body.push('\n');
    }
    crate::utils::atomic_write::write_atomic(path, body.as_bytes(), true)
        .map_err(std::io::Error::other)
}

/// A bounded-retry `O_EXCL` lockfile guard serializing the read-modify-write of
/// `alternates` / `borrowers` (Codex P1: concurrent adds must not drop an
/// entry). Released on drop.
struct FileLock(PathBuf);

impl FileLock {
    fn acquire(target: &Path) -> std::io::Result<Self> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock = target.with_extension("lock");
        for _ in 0..200 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock)
            {
                Ok(_) => return Ok(FileLock(lock)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "timed out acquiring the alternates lock",
        ))
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Serialized read-modify-write: acquire the lock on `list_file`, re-read it,
/// apply `mutate`, and write it back atomically.
fn update_list(
    list_file: &Path,
    base_dir: &Path,
    mutate: impl FnOnce(&mut Vec<PathBuf>),
) -> std::io::Result<bool> {
    let _lock = FileLock::acquire(list_file)?;
    let mut entries = read_list(list_file, base_dir);
    let before = entries.len();
    let before_snapshot = entries.clone();
    mutate(&mut entries);
    if entries == before_snapshot {
        return Ok(false);
    }
    let _ = before;
    write_list(list_file, &entries)?;
    Ok(true)
}

/// The alternate object dirs this store borrows FROM (direct, unresolved).
pub fn list(objects_dir: &Path) -> Vec<PathBuf> {
    read_list(&alternates_file(objects_dir), objects_dir)
}

/// The FLATTENED, transitive alternate chain for `objects_dir` (git alternates
/// are transitive). Cycle-safe (canonicalized visited set, with a raw-path
/// fallback when a dir cannot be canonicalized) and depth-capped. Non-existent
/// alternate dirs are skipped (a dangling alternate is a warning, surfaced by
/// fsck — never a hard read failure here).
pub fn resolve_chain(objects_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    visited.insert(canon(objects_dir));

    let mut frontier: Vec<(PathBuf, usize)> =
        list(objects_dir).into_iter().map(|p| (p, 1usize)).collect();
    while let Some((dir, depth)) = frontier.pop() {
        if depth > MAX_DEPTH {
            tracing::warn!(dir = %dir.display(), "alternates chain exceeds max depth; truncating");
            continue;
        }
        let key = canon(&dir);
        if !visited.insert(key) {
            continue; // cycle / already seen
        }
        if !dir.is_dir() {
            tracing::warn!(dir = %dir.display(), "alternate object dir does not exist; skipping");
            continue;
        }
        out.push(dir.clone());
        for next in list(&dir) {
            frontier.push((next, depth + 1));
        }
    }
    out
}

/// Register `alternate_objects_dir` as an alternate of `objects_dir` (append if
/// absent) AND register `objects_dir` as a BORROWER of the alternate (so the
/// base's gc/evict can protect the borrowed objects). Idempotent.
pub fn add(objects_dir: &Path, alternate_objects_dir: &Path) -> std::io::Result<()> {
    let alternate = std::fs::canonicalize(alternate_objects_dir)
        .unwrap_or_else(|_| alternate_objects_dir.to_path_buf());
    let me = std::fs::canonicalize(objects_dir).unwrap_or_else(|_| objects_dir.to_path_buf());
    // 1) register THIS store as a BORROWER of the alternate FIRST (Codex P1):
    // if step 2 then fails, an extra borrower pin (base over-protected) is
    // safer than an unprotected borrow (base could prune what we read).
    update_list(&borrowers_file(&alternate), &alternate, |borrowers| {
        if !borrowers
            .iter()
            .any(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == me)
        {
            borrowers.push(me.clone());
        }
    })?;
    // 2) record the alternate in this store.
    update_list(&alternates_file(objects_dir), objects_dir, |alts| {
        if !alts
            .iter()
            .any(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == alternate)
        {
            alts.push(alternate.clone());
        }
    })?;
    Ok(())
}

/// Remove `alternate_objects_dir` from `objects_dir`'s alternates and
/// unregister `objects_dir` as a borrower of it. Returns whether a link existed.
pub fn remove(objects_dir: &Path, alternate_objects_dir: &Path) -> std::io::Result<bool> {
    let alternate = std::fs::canonicalize(alternate_objects_dir)
        .unwrap_or_else(|_| alternate_objects_dir.to_path_buf());
    let me = std::fs::canonicalize(objects_dir).unwrap_or_else(|_| objects_dir.to_path_buf());
    let removed = update_list(&alternates_file(objects_dir), objects_dir, |alts| {
        alts.retain(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) != alternate);
    })?;
    // Unregister the borrower on the base (locked).
    update_list(&borrowers_file(&alternate), &alternate, |borrowers| {
        borrowers.retain(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) != me);
    })?;
    Ok(removed)
}

/// The LIVE borrowers of `objects_dir` — object dirs that borrow FROM it and
/// still exist. Dead borrower entries (a borrower repo that was deleted) are
/// PRUNED from the file (self-healing), so a stale registration never pins a
/// base forever.
pub fn live_borrowers(objects_dir: &Path) -> Vec<PathBuf> {
    let bfile = borrowers_file(objects_dir);
    let live: Vec<PathBuf> = read_list(&bfile, objects_dir)
        .into_iter()
        .filter(|p| p.is_dir())
        .collect();
    // Self-heal dead entries under the lock (best-effort — a read must never
    // fail just because the prune could not acquire the lock).
    let _ = update_list(&bfile, objects_dir, |all| {
        all.retain(|p| p.is_dir());
    });
    live
}

/// Whether `objects_dir` is a SHARED BASE that some live borrower depends on.
/// gc / cache-evict consult this and refuse to prune loose objects when true.
pub fn has_live_borrowers(objects_dir: &Path) -> bool {
    !live_borrowers(objects_dir).is_empty()
}

/// Append a raw comment/line to a store's info dir (used by tests / diagnostics).
#[cfg(test)]
pub(crate) fn append_alternate_line(objects_dir: &Path, line: &str) -> std::io::Result<()> {
    let path = alternates_file(objects_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn objdir(root: &Path, name: &str) -> PathBuf {
        let d = root.join(name).join("objects");
        std::fs::create_dir_all(d.join("info")).unwrap();
        d
    }

    #[test]
    fn add_list_remove_round_trip_and_borrower_registration() {
        let tmp = tempfile::tempdir().unwrap();
        let a = objdir(tmp.path(), "A"); // base
        let b = objdir(tmp.path(), "B"); // borrower

        assert!(list(&b).is_empty());
        assert!(!has_live_borrowers(&a));

        add(&b, &a).unwrap();
        // B lists A as an alternate; A now has B as a live borrower.
        let alts = list(&b);
        assert_eq!(alts.len(), 1);
        assert!(has_live_borrowers(&a), "A is a shared base");
        assert_eq!(live_borrowers(&a).len(), 1);

        // add is idempotent (no duplicate borrower / alternate).
        add(&b, &a).unwrap();
        assert_eq!(list(&b).len(), 1);
        assert_eq!(live_borrowers(&a).len(), 1);

        // remove unregisters both directions.
        assert!(remove(&b, &a).unwrap());
        assert!(list(&b).is_empty());
        assert!(
            !has_live_borrowers(&a),
            "borrower gone -> A no longer shared"
        );
        assert!(
            !remove(&b, &a).unwrap(),
            "removing a non-alternate returns false"
        );
    }

    #[test]
    fn resolve_chain_is_transitive_cycle_safe_and_skips_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let a = objdir(tmp.path(), "A");
        let b = objdir(tmp.path(), "B");
        let c = objdir(tmp.path(), "C");
        // C -> B -> A (transitive).
        add(&b, &a).unwrap();
        add(&c, &b).unwrap();
        let chain = resolve_chain(&c);
        let canon: HashSet<PathBuf> = chain
            .iter()
            .map(|p| std::fs::canonicalize(p).unwrap())
            .collect();
        assert!(canon.contains(&std::fs::canonicalize(&b).unwrap()));
        assert!(
            canon.contains(&std::fs::canonicalize(&a).unwrap()),
            "transitive A reached"
        );

        // A cycle A -> A is broken by the visited set (no infinite loop).
        add(&a, &a).unwrap_or(()); // self-ref may be added by the raw writer
        append_alternate_line(&a, &a.to_string_lossy()).unwrap();
        let _ = resolve_chain(&a); // must terminate

        // A dangling alternate is skipped (not in the chain).
        let missing = tmp.path().join("gone").join("objects");
        append_alternate_line(&c, &missing.to_string_lossy()).unwrap();
        let chain2 = resolve_chain(&c);
        assert!(
            !chain2.iter().any(|p| p == &missing),
            "dangling alternate is skipped"
        );
    }
}
