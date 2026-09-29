//! Recursive virtual-ancestor construction: base folding, depth/width
//! guards and synthesized content materialization.
#![allow(unused_imports)]
use super::*;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use clap::{Parser, ValueEnum};
use git_internal::{
    hash::ObjectHash,
    internal::{
        index::Index,
        object::{
            blob::Blob,
            commit::Commit,
            signature::{Signature, SignatureType},
            tree::{Tree, TreeItemMode},
        },
    },
};
use serde::{Deserialize, Serialize};

use super::{
    get_target_commit, load_object, load_object_raw, rename_detect, reset,
    restore::{self, RestoreArgs},
    save_object, status, switch,
};
use crate::{
    command::{
        commit::{CleanupMode, cleanup_commit_message, parse_cleanup_mode},
        editor,
    },
    common_utils::{format_commit_msg, parse_commit_msg},
    info_println,
    internal::{
        branch::{Branch, BranchStoreError},
        config::ConfigKv,
        db::get_db_conn_instance,
        head::Head,
        merge_base,
        reflog::{ReflogAction, ReflogContext, with_reflog},
        repo_hooks::{
            RepoHook, replay_repo_hook_output, run_advisory_repo_hook, run_repo_hook_with_io,
        },
        tree_plumbing,
    },
    utils::{
        attributes::{self, AttributeState},
        error::{CliError, CliResult, StableErrorCode},
        object_ext::TreeExt,
        output::{OutputConfig, emit_json_data},
        path, util, worktree,
    },
};


pub(crate) use conflict::*;
pub(crate) use tree_merge::*;
pub(crate) use content::*;
pub(crate) use workdir::*;

// Preserve the existing command::merge type path for downstream callers.
#[allow(unused_imports)]
pub(crate) use autostash::StoppedMerge;
pub(crate) use autostash::{
    MergeAutostash, conclude_merge_after_commit, conclude_stopped_merge, snapshot_stopped_merge,
};
use autostash::{
    preflight_held_autostash, prepare_merge_autostash, resolve_pending_autostash,
    resolve_pending_autostash_with, store_pending_autostash, verify_autostash_ownership,
};
pub(crate) use state::{
    MergeState, merge_in_progress, merge_state_for_pseudo_refs, merge_state_gc_oids,
};

pub(crate) fn ensure_virtual_ancestor_depth(depth: usize) -> Result<(), PullMergeError> {
    if depth > MAX_VIRTUAL_ANCESTOR_DEPTH {
        return Err(PullMergeError::VirtualAncestorTooDeep);
    }
    Ok(())
}

pub(crate) fn ensure_virtual_ancestor_width(bases: usize) -> Result<(), PullMergeError> {
    if bases > MAX_VIRTUAL_ANCESTOR_BASES {
        return Err(PullMergeError::VirtualAncestorTooWide { bases });
    }
    Ok(())
}

pub(crate) fn virtual_base_fold_order(bases: &[ObjectHash]) -> Vec<ObjectHash> {
    let mut ordered = bases.to_vec();
    ordered.sort_by_key(|id| id.to_string());
    ordered.dedup();
    ordered
}

pub(crate) fn merge_bases_of_folded(
    folded: &[ObjectHash],
    next: &ObjectHash,
) -> Result<Vec<ObjectHash>, PullMergeError> {
    merge_bases_of_folded_with(
        folded,
        next,
        |base, tip| {
            merge_base::merge_bases(base, tip)
                .map_err(|error| PullMergeError::History(error.to_string()))
        },
        |ancestor, descendant| {
            merge_base::is_ancestor(ancestor, descendant)
                .map_err(|error| PullMergeError::History(error.to_string()))
        },
    )
}

pub(crate) fn merge_bases_of_folded_with(
    folded: &[ObjectHash],
    next: &ObjectHash,
    mut merge_bases: impl FnMut(&ObjectHash, &ObjectHash) -> Result<Vec<ObjectHash>, PullMergeError>,
    mut is_ancestor: impl FnMut(&ObjectHash, &ObjectHash) -> Result<bool, PullMergeError>,
) -> Result<Vec<ObjectHash>, PullMergeError> {
    ensure_virtual_ancestor_width(folded.len())?;
    let [single] = folded else {
        // Candidates tagged with the part (folded base) that produced them.
        let mut candidates: Vec<(usize, ObjectHash)> = Vec::new();
        for (part, base) in folded.iter().enumerate() {
            for candidate in merge_bases(base, next)? {
                if !candidates.iter().any(|(_, known)| *known == candidate) {
                    candidates.push((part, candidate));
                    ensure_virtual_ancestor_width(candidates.len())?;
                }
            }
        }
        let mut maximal = Vec::new();
        for (part, candidate) in &candidates {
            let mut dominated = false;
            for (other_part, other) in &candidates {
                if other_part == part {
                    continue;
                }
                if is_ancestor(candidate, other)? {
                    dominated = true;
                    break;
                }
            }
            if !dominated {
                maximal.push(*candidate);
            }
        }
        return Ok(virtual_base_fold_order(&maximal));
    };
    merge_bases(single, next)
}

pub(crate) fn virtual_merge_base(
    bases: &[ObjectHash],
    gitlinks: &GitlinkEntries,
    fold: VirtualFold<'_>,
) -> Result<VirtualAncestor, PullMergeError> {
    let mut blobs = VirtualBlobs::new();
    let items = fold_merge_bases(bases, gitlinks, 1, &mut blobs, fold)?;
    Ok(VirtualAncestor { items, blobs })
}

pub(crate) fn fold_merge_bases(
    bases: &[ObjectHash],
    gitlinks: &GitlinkEntries,
    depth: usize,
    blobs: &mut VirtualBlobs,
    fold: VirtualFold<'_>,
) -> Result<HashMap<PathBuf, MergeTreeEntry>, PullMergeError> {
    ensure_virtual_ancestor_depth(depth)?;
    let ordered = virtual_base_fold_order(bases);
    ensure_virtual_ancestor_width(ordered.len())?;
    let Some((first, rest)) = ordered.split_first() else {
        // No common ancestor at this level: the virtual ancestor is the empty
        // tree, exactly as an unrelated-history merge uses one.
        return Ok(HashMap::new());
    };
    let first_commit = load_merge_commit(first)?;
    let mut folded_ids = vec![*first];
    let mut timestamp = first_commit.committer.timestamp;
    let mut items = commit_tree_split_for_merge(&first_commit)?.0;
    for next in rest {
        let next_commit = load_merge_commit(next)?;
        let next_items = commit_tree_split_for_merge(&next_commit)?.0;
        let sub_bases = merge_bases_of_folded(&folded_ids, next)?;
        let sub_items = fold_merge_bases(&sub_bases, gitlinks, depth + 1, blobs, fold.clone())?;
        items = merge_virtual_items(&sub_items, &items, &next_items, depth, blobs, fold.clone())?;
        folded_ids.push(*next);
        timestamp = timestamp.max(next_commit.committer.timestamp);
        if fold.persist {
            materialize_virtual_ancestor(&items, gitlinks, &folded_ids, timestamp)?;
        }
    }
    Ok(items)
}

pub(crate) fn merge_virtual_items(
    base_items: &HashMap<PathBuf, MergeTreeEntry>,
    our_items: &HashMap<PathBuf, MergeTreeEntry>,
    their_items: &HashMap<PathBuf, MergeTreeEntry>,
    depth: usize,
    blobs: &mut VirtualBlobs,
    fold: VirtualFold<'_>,
) -> Result<HashMap<PathBuf, MergeTreeEntry>, PullMergeError> {
    // FIX-MG05-02: the fold is a merge, so it detects renames like any other.
    // Without this the virtual ancestor keeps the OLD path while the sides
    // carry the new one, and the outer merge then compares each side against a
    // base that has nothing at the renamed path — which silently resurrects
    // content one side had reverted. Measured on git 2.50.1 with bases
    // `A` (renames `old` to `new`) and `B` (edits line 2), ours merging both
    // and reverting B's edit, theirs merging both and editing line 7: Git keeps
    // the revert, and this fold used to restore `B edit`. Git runs the same
    // detection at `call_depth > 0`. The notices are dropped: Git announces
    // nothing inside a virtual merge, and the user never chose these inputs.
    let mut base_items = base_items.clone();
    let mut our_items = our_items.clone();
    let mut their_items = their_items.clone();
    // MG-06: the fold raises the same path-level rename shapes the user's own
    // merge does. It needs no `forced` conflicts, though: a virtual ancestor
    // can never fail, so every shape settles as CONTENT here — a 1to2 leaves
    // the one merged blob at both destinations and drops the source, a
    // collision leaves the rename's merge to be resolved against the occupant,
    // and a rename/delete reuses the base version — which is exactly what the
    // path-by-path resolution below produces from the maps this rewrites.
    detect_and_apply_renames(
        &mut base_items,
        &mut our_items,
        &mut their_items,
        fold.rename_config,
        fold.conflict_style,
        (VIRTUAL_OURS_LABEL, VIRTUAL_THEIRS_LABEL),
        &mut TreeMergeContext::nested_with_external(
            fold.persist,
            depth,
            fold.conflict_style,
            fold.default_driver,
            fold.external_merge_runtime.clone(),
            fold.input_normalization,
            blobs,
        ),
    )?;
    let (base_items, our_items, their_items) = (&base_items, &our_items, &their_items);

    let mut all_paths: BTreeSet<PathBuf> = base_items.keys().cloned().collect();
    all_paths.extend(our_items.keys().cloned());
    all_paths.extend(their_items.keys().cloned());

    let mut merged = HashMap::new();
    for path in all_paths {
        let [base, ours, theirs] = sides_without_empty_dir_beside_file(
            base_items.get(&path),
            our_items.get(&path),
            their_items.get(&path),
        );
        let resolution = {
            let mut context = TreeMergeContext {
                persist_merged_blobs: fold.persist,
                favor: None,
                conflict_style: fold.conflict_style,
                depth,
                default_driver: fold.default_driver.map(str::to_owned),
                external_merge_runtime: fold.external_merge_runtime.clone(),
                input_normalization: fold.input_normalization,
                ancestor_label: "merged common ancestors".to_string(),
                ours_label: VIRTUAL_OURS_LABEL.to_string(),
                theirs_label: VIRTUAL_THEIRS_LABEL.to_string(),
                virtual_blobs: blobs,
            };
            resolve_three_way(&path, base, ours, theirs, &mut context)?
        };
        let entry = match resolution {
            MergeResolution::Use(entry) => Some(entry),
            MergeResolution::Delete => None,
            MergeResolution::Conflict(kind) => {
                if let ConflictKind::BothChanged {
                    rendered: Some(entry),
                    ..
                } = kind
                {
                    merged.insert(path, entry);
                    continue;
                }
                let driver = match kind {
                    ConflictKind::BothChanged { driver, .. } => driver,
                    _ => BuiltinMergeDriver::Text,
                };
                virtual_conflict_resolution(
                    &path,
                    base,
                    ours,
                    theirs,
                    blobs,
                    VirtualConflictContext {
                        driver,
                        depth,
                        fold: fold.clone(),
                    },
                )?
            }
        };
        if let Some(entry) = entry {
            merged.insert(path, entry);
        }
    }
    // MG-04 inside the fold (Git at `call_depth > 0`): a file whose path is
    // also a directory in the result cannot go into one tree; Git moves it to
    // `unique_path(path, "Temporary merge branch N")` and keeps folding — no
    // user is asked. Without the move the ancestor tree would carry a blob and
    // a subtree under one name.
    relocate_virtual_df_files(&mut merged, base_items, our_items, their_items);
    // Empty-directory markers stay in the ancestor: the outer merge must see
    // that the base HAD a directory there (Codex R3), and
    // `create_tree_from_items_map` writes such an entry verbatim.
    Ok(merged)
}

pub(crate) fn relocate_virtual_df_files(
    merged: &mut HashMap<PathBuf, MergeTreeEntry>,
    base_items: &HashMap<PathBuf, MergeTreeEntry>,
    our_items: &HashMap<PathBuf, MergeTreeEntry>,
    their_items: &HashMap<PathBuf, MergeTreeEntry>,
) {
    let file_at = |items: &HashMap<PathBuf, MergeTreeEntry>, path: &PathBuf| {
        items
            .get(path)
            .copied()
            .filter(|entry| entry.mode != TreeItemMode::Tree)
    };
    let mut entries: Vec<(PathBuf, MergeTreeEntry)> = merged
        .iter()
        .map(|(path, entry)| (path.clone(), *entry))
        .collect();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    let mut base_paths: Vec<&PathBuf> = base_items.keys().collect();
    base_paths.sort();
    let mut no_subtrees = |_: &ObjectHash| Ok(false);
    let mut moves: Vec<(PathBuf, &'static str)> = Vec::new();
    let mut drops: Vec<PathBuf> = Vec::new();
    for (path, _) in &entries {
        let label = match (file_at(our_items, path), file_at(their_items, path)) {
            (Some(_), None) => VIRTUAL_OURS_LABEL,
            (None, Some(_)) => VIRTUAL_THEIRS_LABEL,
            _ => continue,
        };
        let at = base_paths.partition_point(|candidate| candidate.as_path() < path.as_path());
        let base_present = base_paths
            .get(at)
            .is_some_and(|candidate| candidate.starts_with(path));
        // The fold's maps hold leaves and empty-directory markers only, so no
        // subtree ever needs reading here.
        let file_survives = merged
            .get(path)
            .is_some_and(|entry| entry.mode != TreeItemMode::Tree);
        if !file_survives {
            continue;
        }
        if directory_is_in_the_way(path, &entries, base_present, &mut no_subtrees).unwrap_or(false)
        {
            moves.push((path.clone(), label));
        } else {
            // Empty-only entries beneath a surviving file would put a blob and
            // a subtree under one name in the ancestor tree.
            drops.push(path.clone());
        }
    }
    for path in drops {
        merged.retain(|other, _| other == &path || !other.starts_with(&path));
    }
    if moves.is_empty() {
        return;
    }
    // The same occupancy as the outer merge: every input path (a name only
    // the base had, deleted by both folded sides, still counts), every result
    // path, and their ancestors.
    let mut taken = df_occupied_names(&[base_items, our_items, their_items], merged, &[]);
    for (path, label) in moves {
        let Some(entry) = merged.remove(&path) else {
            continue;
        };
        let target = unique_df_path(&path, label, &taken);
        taken.insert(target.clone());
        merged.insert(target, entry);
    }
}

pub(crate) fn virtual_conflict_resolution(
    path: &Path,
    base: Option<&MergeTreeEntry>,
    ours: Option<&MergeTreeEntry>,
    theirs: Option<&MergeTreeEntry>,
    blobs: &mut VirtualBlobs,
    context: VirtualConflictContext<'_>,
) -> Result<Option<MergeTreeEntry>, PullMergeError> {
    let VirtualConflictContext {
        driver,
        depth,
        fold,
    } = context;
    let (Some(ours), Some(theirs)) = (ours, theirs) else {
        return Ok(base.copied());
    };
    if tree_item_kind(ours.mode) != tree_item_kind(theirs.mode) || !is_regular_file_mode(ours.mode)
    {
        return Ok(base.copied());
    }
    // Git treats an original of a DIFFERENT type as no original at all and
    // merges two-way (`merge-ort.c`'s `two_way`). The MODE rule below still
    // sees the real original, exactly as Git's does.
    let base_content = base.filter(|entry| is_regular_file_mode(entry.mode));
    let base_bytes = match base_content {
        Some(entry) => Some(load_merge_blob(entry.hash, blobs)?.data),
        None => None,
    };
    let ours_blob = load_merge_blob(ours.hash, blobs)?;
    let theirs_blob = load_merge_blob(theirs.hash, blobs)?;
    let mode = virtual_merged_mode(base, ours, theirs);
    let base_bytes = base_bytes.as_deref().unwrap_or(&[]);

    let mut record = |blob: &Blob| {
        TreeMergeContext {
            persist_merged_blobs: fold.persist,
            favor: None,
            conflict_style: fold.conflict_style,
            depth,
            default_driver: None,
            external_merge_runtime: fold.external_merge_runtime.clone(),
            input_normalization: fold.input_normalization,
            ancestor_label: "merged common ancestors".to_string(),
            ours_label: VIRTUAL_OURS_LABEL.to_string(),
            theirs_label: VIRTUAL_THEIRS_LABEL.to_string(),
            virtual_blobs: blobs,
        }
        .record_merged_blob(blob)
    };

    if driver == BuiltinMergeDriver::Binary
        || merge_input_is_binary(base_bytes)
        || merge_input_is_binary(&ours_blob.data)
        || merge_input_is_binary(&theirs_blob.data)
    {
        let Some(entry) = base_content else {
            // No original: Git's empty buffer, materialized as the empty blob
            // so the ancestor still HAS the path (an absent one would turn the
            // outer merge's add/add into a one-sided add).
            let empty = Blob::from_content_bytes(Vec::new());
            record(&empty)?;
            return Ok(Some(MergeTreeEntry {
                hash: empty.id,
                mode,
            }));
        };
        return Ok(Some(MergeTreeEntry {
            hash: entry.hash,
            mode,
        }));
    }

    let content = merge_virtual_content(
        driver,
        path,
        base_bytes,
        &ours_blob.data,
        &theirs_blob.data,
        depth,
        &fold,
    )
    .map_err(PullMergeError::TreeCreate)?;
    let blob = Blob::from_content_bytes(content);
    record(&blob)?;
    Ok(Some(MergeTreeEntry {
        hash: blob.id,
        mode,
    }))
}

pub(crate) fn merge_input_is_binary(content: &[u8]) -> bool {
    merge_input_exceeds_xdiff_size(content.len())
        || content.iter().take(8000).any(|&byte| byte == 0)
}

pub(crate) fn merge_input_exceeds_xdiff_size(len: usize) -> bool {
    len > MAX_XDIFF_SIZE
}

pub(crate) fn tree_item_kind(mode: TreeItemMode) -> u8 {
    match mode {
        TreeItemMode::Blob | TreeItemMode::BlobExecutable => 0,
        TreeItemMode::Link => 1,
        TreeItemMode::Tree => 2,
        TreeItemMode::Commit => 3,
    }
}

pub(crate) fn is_regular_file_mode(mode: TreeItemMode) -> bool {
    matches!(mode, TreeItemMode::Blob | TreeItemMode::BlobExecutable)
}

pub(crate) fn virtual_merged_mode(
    base: Option<&MergeTreeEntry>,
    ours: &MergeTreeEntry,
    theirs: &MergeTreeEntry,
) -> TreeItemMode {
    if ours.mode == theirs.mode || base.is_some_and(|base| base.mode == ours.mode) {
        theirs.mode
    } else {
        ours.mode
    }
}

pub(crate) fn merge_virtual_content(
    driver: BuiltinMergeDriver,
    path: &Path,
    base: &[u8],
    ours: &[u8],
    theirs: &[u8],
    depth: usize,
    fold: &VirtualFold<'_>,
) -> Result<Vec<u8>, String> {
    let marker_len = conflict_marker_length_at_depth(&[base, ours, theirs], depth);
    match merge_bytes_with_input_normalization(
        driver,
        path,
        base,
        ours,
        theirs,
        MergeContentOptions {
            favor: None,
            conflict_style: fold.conflict_style,
            extra_marker_size: 2 * depth,
            normalization: fold.input_normalization,
        },
    )? {
        BuiltinMergeOutcome::Clean(merged) => Ok(merged),
        BuiltinMergeOutcome::Conflict(conflicted) => Ok(relabel_conflict_markers(
            conflicted,
            marker_len,
            VIRTUAL_OURS_LABEL,
            VIRTUAL_THEIRS_LABEL,
            "base",
        )),
    }
}

pub(crate) fn materialize_virtual_ancestor(
    items: &HashMap<PathBuf, MergeTreeEntry>,
    gitlinks: &GitlinkEntries,
    parents: &[ObjectHash],
    timestamp: usize,
) -> Result<ObjectHash, PullMergeError> {
    let mut tree_items = items.clone();
    for (path, gitlink) in gitlinks {
        tree_items.insert(
            path.clone(),
            MergeTreeEntry {
                hash: *gitlink,
                mode: TreeItemMode::Commit,
            },
        );
    }
    let tree_id = create_tree_from_items_map(&tree_items).map_err(PullMergeError::TreeCreate)?;
    let signature = |signature_type| Signature {
        signature_type,
        name: "Libra".to_string(),
        email: "virtual-merge-base@libra.invalid".to_string(),
        timestamp,
        timezone: "+0000".to_string(),
    };
    let commit = Commit::new(
        signature(SignatureType::Author),
        signature(SignatureType::Committer),
        tree_id,
        parents.to_vec(),
        VIRTUAL_ANCESTOR_MESSAGE,
    );
    save_object(&commit, &commit.id)
        .map_err(|error| PullMergeError::CommitSave(error.to_string()))?;
    Ok(commit.id)
}
