use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
};

use git_internal::internal::index::Index;

use super::{
    calc_file_blob_hash,
    status::{Changes, StatusError, UntrackedFiles},
    status_untracked_paths::{
        TrackedPaths, collapse_untracked_directories, directory_marker, is_top_level_path,
        sort_paths,
    },
};
use crate::utils::{path, util};

pub(crate) struct StatusWorktreeChanges {
    pub(crate) unstaged: Changes,
    pub(crate) ignored_files: Vec<PathBuf>,
    pub(crate) index: Index,
    /// §B.3.3 accumulator protocol: paths the scan could not inspect
    /// (workdir-relative). Text formats fail closed on any entry; JSON
    /// reports the partial result plus `data.io_blocked[]`.
    pub(crate) io_blocked: Vec<crate::command::status_probe::IoBlockedEvent>,
}

struct WorkdirScan {
    untracked: Vec<PathBuf>,
    ignored: Vec<PathBuf>,
    io_blocked: Vec<crate::command::status_probe::IoBlockedEvent>,
}

fn io_blocked_event(
    path: &Path,
    error: &io::Error,
) -> crate::command::status_probe::IoBlockedEvent {
    use crate::command::status_probe::{IoBlockedEvent, IoBlockedReason};
    IoBlockedEvent {
        path: path.to_path_buf(),
        reason: if error.kind() == io::ErrorKind::PermissionDenied {
            IoBlockedReason::PermissionDenied
        } else {
            IoBlockedReason::IoError
        },
    }
}

pub(crate) fn collect_status_worktree_changes(
    untracked_mode: UntrackedFiles,
    include_ignored: bool,
    ignore_case: bool,
) -> Result<StatusWorktreeChanges, StatusError> {
    let workdir = util::try_working_dir().map_err(|source| StatusError::Workdir { source })?;
    let index_path = path::try_index().map_err(|source| StatusError::Workdir { source })?;
    let index = Index::load(&index_path).map_err(|source| StatusError::IndexLoad {
        path: index_path.clone(),
        source,
    })?;
    let tracked = TrackedPaths::from_index(&index, ignore_case);
    let mut io_blocked: Vec<crate::command::status_probe::IoBlockedEvent> = Vec::new();
    let mut unstaged =
        collect_tracked_worktree_changes(&workdir, &index, tracked.files(), &mut io_blocked)?;
    let mut ignored_files = Vec::new();

    if !matches!(untracked_mode, UntrackedFiles::No) {
        let mut scan = scan_workdir(&workdir, &index, &tracked, untracked_mode, include_ignored)?;
        io_blocked.append(&mut scan.io_blocked);
        unstaged.new = if matches!(untracked_mode, UntrackedFiles::Normal) {
            collapse_untracked_directories(scan.untracked, &tracked)
        } else {
            sort_paths(scan.untracked)
        };
        ignored_files = if matches!(untracked_mode, UntrackedFiles::Normal) {
            collapse_untracked_directories(scan.ignored, &tracked)
        } else {
            sort_paths(scan.ignored)
        };
    }

    io_blocked.sort_by(|a, b| a.path.cmp(&b.path));
    io_blocked.dedup_by(|a, b| a.path == b.path);
    Ok(StatusWorktreeChanges {
        unstaged,
        ignored_files,
        index,
        io_blocked,
    })
}

/// ScorpioFS heuristic fast path: the mount's upper layer is the only writable
/// surface, so the daemon's changed-path set IS the complete candidate set for
/// unstaged changes — nothing outside it can differ from the index baseline.
/// Querying it replaces the whole-tree walk and the per-file stat storm with a
/// single bounded HTTP call plus hash comparisons (O(touch-set), not O(tree)).
///
/// Returns `None` (caller falls back to the full scan) when not on a ScorpioFS
/// mount, when the daemon is unreachable or pre-Worktree-v2, and for
/// `--untracked-files=all` (kept byte-identical with the full-scan semantics).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn scorpiofs_worktree_changes(
    untracked_mode: UntrackedFiles,
    include_ignored: bool,
    ignore_case: bool,
) -> Option<StatusWorktreeChanges> {
    use crate::command::sync::{ScorpioState, current_mount_id, http, scorpio_endpoint};
    use std::time::Duration;

    if matches!(untracked_mode, UntrackedFiles::All) {
        return None;
    }
    let mount_id = current_mount_id()?;
    let workdir = util::try_working_dir().ok()?;
    let index_path = path::try_index().ok()?;
    let index = Index::load(&index_path).ok()?;

    let url = format!("{}/worktrees/{}/state", scorpio_endpoint(), mount_id);
    // Bounded wait: status must stay fast even when the daemon is gone.
    let response = http()
        .get(&url)
        .timeout(Duration::from_millis(800))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let state = response.json::<ScorpioState>().await.ok()?;

    let tracked = TrackedPaths::from_index(&index, ignore_case);
    let mut unstaged = Changes::default();
    let mut ignored_files = Vec::new();
    for change in &state.changes {
        let p = change.path.trim_start_matches("./");
        // The `.libra` pointer is VCS metadata living in the upper layer; it
        // never participates in status.
        if p.is_empty() || p == ".libra" || p.starts_with(".libra/") {
            continue;
        }
        let rel = PathBuf::from(p);
        let is_tracked = index.tracked(p, 0);
        match change.kind.as_str() {
            "deleted" => {
                if is_tracked {
                    unstaged.deleted.push(rel);
                }
            }
            "added" | "modified" => {
                if is_tracked {
                    // Ignore rules never apply to tracked files (Git semantics):
                    // an edited `app.log` stays `modified` even when `*.log` is
                    // ignored. Test the hash first, so a matching ignore rule
                    // cannot silently swallow a real edit.
                    // The daemon hashes upper content in the same git-blob-OID
                    // domain as the index; a touch or reverted write hashes
                    // back to the index value, which means clean.
                    let clean = match (&change.content_hash, index.get(p, 0)) {
                        (Some(h), Some(entry)) => *h == entry.hash.to_string(),
                        _ => false,
                    };
                    if !clean {
                        unstaged.modified.push(rel);
                    }
                } else {
                    let absolute = workdir.join(&rel);
                    if util::check_gitignore(&workdir, &absolute) {
                        if include_ignored {
                            ignored_files.push(rel);
                        }
                    } else if !matches!(untracked_mode, UntrackedFiles::No) {
                        unstaged.new.push(rel);
                    }
                }
            }
            _ => {}
        }
    }
    if matches!(untracked_mode, UntrackedFiles::Normal) {
        unstaged.new = collapse_untracked_directories(unstaged.new, &tracked);
    }
    Some(StatusWorktreeChanges {
        unstaged,
        ignored_files,
        index,
        io_blocked: Vec::new(),
    })
}

pub(crate) fn changes_to_current_directory(mut changes: Changes) -> Changes {
    changes.new = changes
        .new
        .into_iter()
        .map(path_to_current_preserving_directory_marker)
        .collect();
    changes.modified = changes
        .modified
        .into_iter()
        .map(util::workdir_to_current)
        .collect();
    changes.deleted = changes
        .deleted
        .into_iter()
        .map(util::workdir_to_current)
        .collect();
    changes.renamed = changes
        .renamed
        .into_iter()
        .map(|(old, new)| (util::workdir_to_current(old), util::workdir_to_current(new)))
        .collect();
    changes
}

fn path_to_current_preserving_directory_marker(path: PathBuf) -> PathBuf {
    if !path.to_string_lossy().ends_with('/') {
        return util::workdir_to_current(path);
    }

    let relative = util::workdir_to_current(&path);
    directory_marker(&relative)
}

fn collect_tracked_worktree_changes(
    workdir: &Path,
    index: &Index,
    tracked_files: &[PathBuf],
    io_blocked: &mut Vec<crate::command::status_probe::IoBlockedEvent>,
) -> Result<Changes, StatusError> {
    let mut changes = Changes::default();
    for file in tracked_files {
        let file_str = file
            .to_str()
            .ok_or_else(|| StatusError::InvalidPathEncoding { path: file.clone() })?;
        let file_abs = workdir.join(file);
        // A tracked path that is genuinely gone is a deletion; but a real I/O
        // error (permission denied, I/O failure) only means "can't tell" — it
        // must NOT be reported as a deletion NOR fabricated as clean
        // (§B.6.0.1/§B.3.3): record the event, keep everything already
        // collected, and continue with the remaining tracked files. Text
        // formats fail closed at render time; JSON reports the partial
        // result with `data.io_blocked[]`.
        match file_abs.symlink_metadata() {
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                changes.deleted.push(file.clone());
                continue;
            }
            Err(source) => {
                io_blocked.push(io_blocked_event(file, &source));
                continue;
            }
            Ok(_) => {}
        }
        if index.is_modified(file_str, 0, workdir) {
            match calc_file_blob_hash(&file_abs) {
                Ok(file_hash) => {
                    if !index.verify_hash(file_str, 0, &file_hash) {
                        changes.modified.push(file.clone());
                    }
                }
                Err(source) => {
                    io_blocked.push(io_blocked_event(file, &source));
                    continue;
                }
            }
        }
    }
    Ok(changes)
}

fn scan_workdir(
    workdir: &Path,
    index: &Index,
    tracked: &TrackedPaths,
    untracked_mode: UntrackedFiles,
    include_ignored: bool,
) -> Result<WorkdirScan, StatusError> {
    let mut scan = WorkdirScan {
        untracked: Vec::new(),
        ignored: Vec::new(),
        io_blocked: Vec::new(),
    };
    let mut pending_dirs = vec![workdir.to_path_buf()];

    while let Some(dir) = pending_dirs.pop() {
        let dir_rel = dir.strip_prefix(workdir).unwrap_or(&dir).to_path_buf();
        let reader = match std::fs::read_dir(&dir) {
            Ok(reader) => reader,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                // §B.3.3: record and continue with the remaining pending
                // directories — never drop what earlier siblings collected.
                scan.io_blocked.push(io_blocked_event(&dir_rel, &source));
                continue;
            }
        };
        for entry in reader {
            let entry = match entry {
                Ok(entry) => entry,
                Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => {
                    scan.io_blocked.push(io_blocked_event(&dir_rel, &source));
                    continue;
                }
            };
            let path = entry.path();
            let name = entry.file_name();
            if name == OsStr::new(util::ROOT_DIR) || name == OsStr::new(util::GIT_DIR) {
                continue;
            }

            let entry_rel = path.strip_prefix(workdir).unwrap_or(&path).to_path_buf();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => {
                    scan.io_blocked.push(io_blocked_event(&entry_rel, &source));
                    continue;
                }
            };
            let relative = path
                .strip_prefix(workdir)
                .map_err(|err| list_error(&dir, io::Error::other(err.to_string())))?
                .to_path_buf();
            if file_type.is_dir() {
                if util::check_gitignore(workdir, &path) {
                    if include_ignored {
                        scan.ignored.push(relative);
                    }
                    continue;
                }
                if matches!(untracked_mode, UntrackedFiles::Normal)
                    && !include_ignored
                    && is_top_level_path(&relative)
                    && !tracked.has_descendant(&relative)
                {
                    // Git only reports an untracked directory when it holds
                    // at least one visible untracked file. A directory whose
                    // entire contents are skip-listed (`.libra`/`.git`) or
                    // ignored must stay invisible — e.g. test harnesses'
                    // `.libra-test-home/` holding only a nested `.libra`.
                    // The probe stops at the first qualifying file, so the
                    // no-descend perf win survives for real content; an
                    // unreadable directory is conservatively reported (we
                    // cannot verify it is empty, and git reports it too).
                    if untracked_dir_has_visible_file(workdir, &path) {
                        scan.untracked.push(directory_marker(&relative));
                    }
                    continue;
                }
                pending_dirs.push(path);
            } else if file_type.is_file() || file_type.is_symlink() {
                scan_file(
                    &mut scan,
                    workdir,
                    index,
                    tracked,
                    &path,
                    &relative,
                    include_ignored,
                )?;
            }
        }
    }

    Ok(scan)
}

fn scan_file(
    scan: &mut WorkdirScan,
    workdir: &Path,
    index: &Index,
    tracked_paths: &TrackedPaths,
    path: &Path,
    relative: &Path,
    include_ignored: bool,
) -> Result<(), StatusError> {
    // §B.6.1: a non-UTF-8 name can never be an index entry (index paths are
    // UTF-8 strings), so it is untracked by construction — base `??`/ignored
    // classification proceeds on the raw path and the whole status must NOT
    // fail (rename candidacy is skipped separately with a warning).
    let tracked = match relative.to_str() {
        Some(file_str) => {
            index.tracked(file_str, 0) || tracked_paths.same_file_case_alias(workdir, relative)
        }
        None => false,
    };
    if util::check_gitignore(workdir, path) {
        if include_ignored && !tracked {
            scan.ignored.push(relative.to_path_buf());
        }
    } else if !tracked {
        scan.untracked.push(relative.to_path_buf());
    }
    Ok(())
}

/// Whether an untracked top-level directory contains at least one visible
/// (non-skip-listed, non-ignored) file — the git precondition for showing
/// the collapsed `dir/` marker. Walks lazily and returns at the first hit;
/// any read error makes the answer `true` (report rather than silently
/// hide something we could not inspect).
fn untracked_dir_has_visible_file(workdir: &Path, dir: &Path) -> bool {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(_) => return true,
        };
        for entry in entries {
            let Ok(entry) = entry else { return true };
            let name = entry.file_name();
            if name == OsStr::new(util::ROOT_DIR) || name == OsStr::new(util::GIT_DIR) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                return true;
            };
            let path = entry.path();
            if util::check_gitignore(workdir, &path) {
                continue;
            }
            if file_type.is_file() || file_type.is_symlink() {
                return true;
            }
            if file_type.is_dir() {
                pending.push(path);
            }
        }
    }
    false
}

fn list_error(path: &Path, source: io::Error) -> StatusError {
    StatusError::ListWorkdirFiles {
        path: path.to_path_buf(),
        source,
    }
}
