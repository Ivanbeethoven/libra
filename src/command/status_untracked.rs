use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use git_internal::internal::index::Index;

use super::{
    calc_file_blob_hash,
    status::{Changes, StatusError, UntrackedFiles},
};
use crate::utils::{path, util};

pub(crate) struct StatusWorktreeChanges {
    pub(crate) unstaged: Changes,
    pub(crate) ignored_files: Vec<PathBuf>,
    pub(crate) index: Index,
}

struct WorkdirScan {
    untracked: Vec<PathBuf>,
    ignored: Vec<PathBuf>,
}

pub(crate) fn collect_status_worktree_changes(
    untracked_mode: UntrackedFiles,
    include_ignored: bool,
) -> Result<StatusWorktreeChanges, StatusError> {
    let workdir = util::try_working_dir().map_err(|source| StatusError::Workdir { source })?;
    let index_path = path::try_index().map_err(|source| StatusError::Workdir { source })?;
    let index = Index::load(&index_path).map_err(|source| StatusError::IndexLoad {
        path: index_path.clone(),
        source,
    })?;
    let tracked_files = index.tracked_files();
    let mut unstaged = collect_tracked_worktree_changes(&workdir, &index, &tracked_files)?;
    let mut ignored_files = Vec::new();

    if !matches!(untracked_mode, UntrackedFiles::No) {
        let scan = scan_workdir(
            &workdir,
            &index,
            &tracked_files,
            untracked_mode,
            include_ignored,
        )?;
        unstaged.new = if matches!(untracked_mode, UntrackedFiles::Normal) && include_ignored {
            collapse_untracked_directories(scan.untracked, &index)
        } else {
            scan.untracked
        };
        ignored_files = if matches!(untracked_mode, UntrackedFiles::Normal) && include_ignored {
            collapse_untracked_directories(scan.ignored, &index)
        } else {
            scan.ignored
        };
    }

    Ok(StatusWorktreeChanges {
        unstaged,
        ignored_files,
        index,
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
) -> Result<Changes, StatusError> {
    let mut changes = Changes::default();
    for file in tracked_files {
        let file_str = file
            .to_str()
            .ok_or_else(|| StatusError::InvalidPathEncoding { path: file.clone() })?;
        let file_abs = workdir.join(file);
        if !file_abs.exists() {
            changes.deleted.push(file.clone());
        } else if index.is_modified(file_str, 0, workdir) {
            let file_hash =
                calc_file_blob_hash(&file_abs).map_err(|source| StatusError::FileHash {
                    path: file_abs.clone(),
                    source,
                })?;
            if !index.verify_hash(file_str, 0, &file_hash) {
                changes.modified.push(file.clone());
            }
        }
    }
    Ok(changes)
}

fn scan_workdir(
    workdir: &Path,
    index: &Index,
    tracked_files: &[PathBuf],
    untracked_mode: UntrackedFiles,
    include_ignored: bool,
) -> Result<WorkdirScan, StatusError> {
    let mut scan = WorkdirScan {
        untracked: Vec::new(),
        ignored: Vec::new(),
    };
    let mut pending_dirs = vec![workdir.to_path_buf()];

    while let Some(dir) = pending_dirs.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|source| StatusError::ListWorkdirFiles {
            path: workdir.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| StatusError::ListWorkdirFiles {
                path: workdir.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            let name = entry.file_name();
            if name == OsStr::new(util::ROOT_DIR) || name == OsStr::new(util::GIT_DIR) {
                continue;
            }

            let file_type = entry
                .file_type()
                .map_err(|source| StatusError::ListWorkdirFiles {
                    path: workdir.to_path_buf(),
                    source,
                })?;
            let relative = path
                .strip_prefix(workdir)
                .map_err(|err| StatusError::ListWorkdirFiles {
                    path: workdir.to_path_buf(),
                    source: std::io::Error::other(err.to_string()),
                })?
                .to_path_buf();
            if file_type.is_dir() {
                if util::check_gitignore(&workdir.to_path_buf(), &path) {
                    if include_ignored {
                        scan.ignored.push(relative);
                    }
                    continue;
                }
                if matches!(untracked_mode, UntrackedFiles::Normal)
                    && !include_ignored
                    && is_top_level_path(&relative)
                    && !has_tracked_descendant(&relative, tracked_files)
                {
                    scan.untracked.push(directory_marker(&relative));
                    continue;
                }
                pending_dirs.push(path);
            } else if file_type.is_file() {
                scan_file(&mut scan, workdir, index, &path, &relative, include_ignored)?;
            }
        }
    }

    Ok(scan)
}

fn scan_file(
    scan: &mut WorkdirScan,
    workdir: &Path,
    index: &Index,
    path: &Path,
    relative: &Path,
    include_ignored: bool,
) -> Result<(), StatusError> {
    let file_str = relative
        .to_str()
        .ok_or_else(|| StatusError::InvalidPathEncoding {
            path: relative.to_path_buf(),
        })?;
    let tracked = index.tracked(file_str, 0);
    if util::check_gitignore(&workdir.to_path_buf(), &path.to_path_buf()) {
        if include_ignored && !tracked {
            scan.ignored.push(relative.to_path_buf());
        }
    } else if !tracked {
        scan.untracked.push(relative.to_path_buf());
    }
    Ok(())
}

fn collapse_untracked_directories(untracked_files: Vec<PathBuf>, index: &Index) -> Vec<PathBuf> {
    use std::collections::{BTreeSet, HashMap};

    if untracked_files.is_empty() {
        return untracked_files;
    }

    let mut dir_files: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut root_files: Vec<PathBuf> = Vec::new();

    for file in &untracked_files {
        let components: Vec<_> = file.components().collect();
        if components.len() > 1 {
            let top_dir = PathBuf::from(components[0].as_os_str());
            dir_files.entry(top_dir).or_default().push(file.clone());
        } else {
            root_files.push(file.clone());
        }
    }

    let mut result: BTreeSet<PathBuf> = BTreeSet::new();
    result.extend(root_files);

    for (dir, files) in dir_files {
        if has_tracked_descendant(&dir, &index.tracked_files()) {
            result.extend(files);
        } else {
            result.insert(directory_marker(&dir));
        }
    }

    result.into_iter().collect()
}

fn has_tracked_descendant(dir: &Path, tracked_files: &[PathBuf]) -> bool {
    tracked_files.iter().any(|file| file.starts_with(dir))
}

fn is_top_level_path(path: &Path) -> bool {
    path.components().count() == 1
}

fn directory_marker(path: &Path) -> PathBuf {
    let mut display = path.display().to_string();
    if !display.ends_with('/') {
        display.push('/');
    }
    PathBuf::from(display)
}
