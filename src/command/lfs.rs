//! LFS subcommands for authentication, batch negotiation, lock management, and integrating media storage with standard workflows.

use std::{
    fs::{File, OpenOptions},
    io,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::Path,
};

use clap::Subcommand;
use git_internal::internal::index::Index;
use reqwest::StatusCode;

use crate::{
    command::{
        lfs_schema::{LfsFileOutput, LfsOutput},
        status,
    },
    internal::{
        config::ConfigKv,
        head::Head,
        protocol::lfs_client::{LFSClient, LockListError},
    },
    lfs_structs::{Lock, LockListQuery, Ref, VerifiableLockRequest},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        lfs,
        output::{OutputConfig, emit_json_data},
        path,
        path_ext::PathExt,
        util,
    },
};

/// `--help` examples shown in `libra lfs --help` output (attached in
/// `src/cli.rs` via `after_help` on the `Lfs` subcommand).
///
/// `lfs` exposes six sub-commands: `track` (read/add attributes patterns),
/// `untrack`, `ls-files`, and the three lock-server flows (`locks`,
/// `lock`, `unlock`). The banner pins the canonical invocation per
/// sub-command plus a JSON variant so users can map intent to invocation
/// without reading the design doc. Cross-cutting `--help` EXAMPLES
/// rollout per `docs/development/commands/_general.md` item B.
pub const LFS_EXAMPLES: &str = "\
EXAMPLES:
    libra lfs track                       List currently tracked LFS attribute patterns
    libra lfs track '*.bin' '*.psd'       Add LFS patterns to .libra_attributes
    libra lfs untrack '*.bin'             Remove an LFS pattern
    libra lfs ls-files                    List LFS-tracked files in the working tree
    libra lfs ls-files --long --size      Show full OIDs and sizes
    libra lfs locks                       List remote locks for the current branch
    libra lfs lock build/output.bin       Acquire a remote lock on a file
    libra lfs unlock build/output.bin     Release a lock you own
    libra lfs unlock --force --id <id>    Force-release a lock owned by someone else
    libra lfs --json ls-files             Structured JSON output for agents";

/// [Docs](https://github.com/git-lfs/git-lfs/tree/main/docs/man)
#[derive(Subcommand, Debug)]
pub enum LfsCmds {
    /// View or add LFS paths to Libra Attributes (root)
    Track {
        /// One or more glob patterns to mark as LFS-tracked (e.g. `*.bin`). Omit to list current patterns
        pattern: Option<Vec<String>>,
    },
    /// Remove LFS paths from Libra Attributes
    Untrack {
        /// One or more glob patterns to remove from `.libra_attributes`
        path: Vec<String>,
    },
    /// Lists currently locked files from the Libra LFS server. (Current Branch)
    Locks {
        /// Filter to a single lock id
        #[clap(long, short, value_name = "ID")]
        id: Option<String>,
        /// Filter locks to a specific repository-relative path
        #[clap(long, short, value_name = "PATH")]
        path: Option<String>,
        /// Maximum number of locks to return
        #[clap(long, short, value_name = "N")]
        limit: Option<u64>,
    },
    /// Set a file as "locked" on the Libra LFS server
    Lock {
        /// String path name of the locked file. This should be relative to the root of the repository working directory
        path: String,
    },
    /// Remove "locked" setting for a file on the Libra LFS server
    Unlock {
        /// Repository-relative path of the file to unlock
        path: String,
        /// Force-release a lock you do not own (requires server-side permission)
        #[clap(long, short)]
        force: bool,
        /// Unlock by lock id instead of by path
        #[clap(long, short, value_name = "ID")]
        id: Option<String>,
    },
    /// Show information about Libra LFS files in the index and working tree (current branch)
    LsFiles {
        /// Show the entire 64 character OID, instead of just first 10.
        #[clap(long, short)]
        long: bool,
        /// Show the size of the LFS object between parenthesis at the end of a line.
        #[clap(long, short)]
        size: bool,
        /// Show only the lfs tracked file names.
        #[clap(long, short)]
        name_only: bool,
    },
}

pub async fn execute(cmd: LfsCmds) -> CliResult<()> {
    execute_safe(cmd, &OutputConfig::default()).await
}

pub async fn execute_safe(cmd: LfsCmds, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let result = run_lfs(cmd).await?;
    render_lfs_output(&result, output)
}

async fn run_lfs(cmd: LfsCmds) -> CliResult<LfsOutput> {
    // TODO: attributes file should be created in current dir, NOT root dir
    let attr_path = path::attributes().to_string_or_panic();
    match cmd {
        LfsCmds::Track { pattern } => {
            match pattern {
                Some(pattern) => {
                    let pattern = convert_patterns_to_workdir(pattern); //
                    let patterns = add_lfs_patterns(&attr_path, pattern).map_err(|e| {
                        CliError::io(format!("failed to update '{attr_path}': {e}"))
                    })?;
                    Ok(LfsOutput {
                        action: "track".to_string(),
                        patterns,
                        ..LfsOutput::default()
                    })
                }
                None => {
                    let lfs_patterns = lfs::extract_lfs_patterns(&attr_path)
                        .map_err(|e| CliError::io(format!("failed to read '{attr_path}': {e}")))?;
                    Ok(LfsOutput {
                        action: "track-list".to_string(),
                        patterns: lfs_patterns,
                        ..LfsOutput::default()
                    })
                }
            }
        }
        LfsCmds::Untrack { path } => {
            // only remove totally same pattern with path ?
            let path = convert_patterns_to_workdir(path); //
            let patterns = untrack_lfs_patterns(&attr_path, path)
                .map_err(|e| CliError::io(format!("failed to update '{attr_path}': {e}")))?;
            Ok(LfsOutput {
                action: "untrack".to_string(),
                patterns,
                ..LfsOutput::default()
            })
        }
        LfsCmds::Locks { id, path, limit } => {
            let refspec = current_refspec_or_err().await?;
            tracing::debug!("refspec: {}", refspec);
            let query = LockListQuery {
                id: id.unwrap_or_default(),
                path: path.unwrap_or_default(),
                limit: limit.map(|l| l.to_string()).unwrap_or_default(),
                cursor: "".to_string(),
                refspec: refspec.clone(),
            };
            let locks = LFSClient::get()
                .await
                .map_err(|e| {
                    CliError::fatal(e.to_string())
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                })?
                .get_locks(query)
                .await
                .map_err(map_lock_list_error)?
                .locks;
            Ok(LfsOutput {
                action: "locks".to_string(),
                locks,
                refspec: Some(refspec),
                ..LfsOutput::default()
            })
        }
        LfsCmds::Lock { path } => {
            // Only check existence
            if !Path::new(&path).exists() {
                return Err(
                    CliError::fatal(format!("pathspec '{path}' did not match any files"))
                        .with_stable_code(StableErrorCode::CliInvalidTarget),
                );
            }

            let refspec = current_refspec_or_err().await?;
            let code = LFSClient::get()
                .await
                .map_err(|e| {
                    CliError::fatal(e.to_string())
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                })?
                .lock(path.clone(), refspec.clone())
                .await
                .map_err(|e| {
                    CliError::network(format!("LFS lock request failed: {e}"))
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                })?;
            if code == StatusCode::FORBIDDEN {
                return Err(
                    CliError::fatal("You must have push access to create a lock")
                        .with_stable_code(StableErrorCode::AuthPermissionDenied),
                );
            } else if code == StatusCode::CONFLICT {
                return Err(CliError::conflict("lock already exists")
                    .with_stable_code(StableErrorCode::ConflictOperationBlocked));
            } else if !code.is_success() {
                return Err(CliError::network(format!(
                    "LFS lock failed with status {}",
                    code.as_u16()
                ))
                .with_detail("status", code.as_u16()));
            }
            Ok(LfsOutput {
                action: "lock".to_string(),
                path: Some(path),
                refspec: Some(refspec),
                ..LfsOutput::default()
            })
        }
        LfsCmds::Unlock { path, force, id } => {
            // When `--id` is provided the lock is looked up by id on the
            // server (see the `Some(id) => id` branch below); `path` is
            // only kept as a label for the audit output. Skipping the
            // path-existence and clean-tree checks in that case avoids
            // friction when unlocking a file that has been deleted
            // locally but still holds a server-side lock.
            if !force && id.is_none() {
                if !Path::new(&path).exists() {
                    return Err(CliError::fatal(format!(
                        "pathspec '{path}' did not match any files"
                    ))
                    .with_stable_code(StableErrorCode::CliInvalidTarget));
                }
                if !status::is_clean().await {
                    return Err(CliError::conflict("working tree not clean")
                        .with_stable_code(StableErrorCode::ConflictOperationBlocked));
                }
            }
            let refspec = current_refspec_or_err().await?;
            let id = match id {
                None => {
                    // get id by path
                    let locks = LFSClient::get()
                        .await
                        .map_err(|e| {
                            CliError::fatal(e.to_string())
                                .with_stable_code(StableErrorCode::NetworkUnavailable)
                        })?
                        .get_locks(LockListQuery {
                            refspec: refspec.clone(),
                            path: path.clone(),
                            id: "".to_string(),
                            cursor: "".to_string(),
                            limit: "".to_string(),
                        })
                        .await
                        .map_err(map_lock_list_error)?
                        .locks;
                    if locks.is_empty() {
                        return Err(CliError::fatal(format!("no lock found for path '{path}'"))
                            .with_stable_code(StableErrorCode::RepoStateInvalid));
                    }
                    locks[0].id.clone()
                }
                Some(id) => id,
            };
            let code = LFSClient::get()
                .await
                .map_err(|e| {
                    CliError::fatal(e.to_string())
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                })?
                .unlock(id.clone(), refspec.clone(), force)
                .await
                .map_err(|e| {
                    CliError::network(format!("LFS unlock request failed: {e}"))
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                })?;
            if code == StatusCode::FORBIDDEN {
                return Err(CliError::fatal("You must have push access to unlock")
                    .with_stable_code(StableErrorCode::AuthPermissionDenied));
            } else if !code.is_success() {
                return Err(CliError::network(format!(
                    "LFS unlock failed with status {}",
                    code.as_u16()
                ))
                .with_detail("status", code.as_u16()));
            }
            Ok(LfsOutput {
                action: "unlock".to_string(),
                path: Some(path),
                id: Some(id),
                refspec: Some(refspec),
                ..LfsOutput::default()
            })
        }
        LfsCmds::LsFiles {
            long,
            size,
            name_only,
        } => {
            let idx_file = path::index();
            let index = Index::load(&idx_file)
                .map_err(|e| CliError::io(format!("failed to load index: {e}")))?;
            let entries = index.tracked_entries(0);
            let storage = util::objects_storage();
            let mut files = Vec::new();
            for entry in entries {
                let path_abs = util::workdir_to_absolute(&entry.name);
                if lfs::is_lfs_tracked(&path_abs) {
                    let data = storage.get(&entry.hash).map_err(|e| {
                        CliError::io(format!("failed to read blob {}: {e}", entry.hash))
                    })?;
                    if let Some((oid, lfs_size)) = lfs::parse_pointer_data(&data) {
                        let is_pointer = lfs::parse_pointer_file(&path_abs).is_ok();
                        // An asterisk (*) after the OID indicates a full object, a minus (-) indicates an LFS pointer.
                        // or not exists (-)
                        let _type = if is_pointer || !path_abs.exists() {
                            "-"
                        } else {
                            "*"
                        };
                        let full_oid = oid.clone();
                        let oid = if long { oid } else { oid[..10].to_owned() };
                        let (size_value, display_size) = if size {
                            let display = util::auto_unit_bytes(lfs_size);
                            (Some(lfs_size), Some(format!(" ({display:.2})")))
                        } else {
                            (None, None)
                        };
                        files.push(LfsFileOutput {
                            path: entry.name.clone(),
                            oid,
                            full_oid,
                            marker: _type.to_string(),
                            size: size_value,
                            display_size,
                        });
                    }
                }
            }
            Ok(LfsOutput {
                action: "ls-files".to_string(),
                files,
                name_only,
                show_size: size,
                ..LfsOutput::default()
            })
        }
    }
}

fn render_lfs_output(result: &LfsOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("lfs", result, output);
    }
    if output.quiet {
        return Ok(());
    }

    match result.action.as_str() {
        "track" => {
            // Same silent-empty UX class as the `track-list` and `locks`
            // fixes (v0.17.1065 / v0.17.1066): if every requested pattern
            // was already tracked, `add_lfs_patterns` returns an empty
            // `added` Vec and the user previously saw zero output. Emit
            // a confirmed-already-tracked notice so the command never
            // looks like a hang.
            if result.patterns.is_empty() {
                println!("No new patterns added (already tracked)");
            } else {
                for pattern in &result.patterns {
                    println!("Tracking \"{pattern}\"");
                }
            }
        }
        "track-list" => {
            // Always print the header so `libra lfs track` (list mode) is
            // never silent — pre-v0.17.1065 an empty pattern list rendered
            // nothing at all and the user couldn't tell whether the command
            // ran or hung. Matches `git lfs track`'s behavior on an empty
            // repo (header + no rows).
            println!("Listing tracked patterns");
            for pattern in &result.patterns {
                println!("    {} ({})", pattern, util::ATTRIBUTES);
            }
        }
        "untrack" => {
            // Same silent-empty fix: if the file had no matching LFS
            // patterns for the user-supplied args, we previously
            // printed nothing. Emit a confirmed-no-op notice.
            if result.patterns.is_empty() {
                println!("No matching LFS patterns to untrack");
            } else {
                for pattern in &result.patterns {
                    println!("Untracking \"{pattern}\"");
                }
            }
        }
        "locks" => {
            // Same UX class as the `track-list` empty fix in v0.17.1065:
            // an empty lock list previously printed nothing, leaving the
            // user unable to distinguish "no locks held" from "command
            // hung" or "wrong subcommand". Emit a confirmed-empty notice
            // so the success signal is always visible.
            if result.locks.is_empty() {
                println!("No locks on the current branch");
            } else {
                let max_path_len = result
                    .locks
                    .iter()
                    .map(|lock| lock.path.len())
                    .max()
                    .unwrap_or(0);
                for lock in &result.locks {
                    println!(
                        "{:<path_width$}\tID:{}",
                        lock.path,
                        lock.id,
                        path_width = max_path_len
                    );
                }
            }
        }
        "lock" => {
            if let Some(path) = &result.path {
                println!("Locked {path}");
            }
        }
        "unlock" => {
            if let Some(path) = &result.path {
                println!("Unlocked {path}");
            }
        }
        "ls-files" => {
            // Same silent-empty fix: a repo with no LFS-tracked files
            // previously rendered zero stdout. `--name-only` consumers
            // (e.g. shell pipelines) intentionally expect bare output,
            // so the notice is gated on the not-name-only path.
            if result.files.is_empty() {
                if !result.name_only {
                    println!("No LFS files in the working tree");
                }
            } else {
                for file in &result.files {
                    let tail = file.display_size.as_deref().unwrap_or("");
                    if result.name_only {
                        println!("{}{}", file.path, tail);
                    } else {
                        println!("{} {} {}{}", file.oid, file.marker, file.path, tail);
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
}

/// `lfs.lockEnforce` policy (lore.md 2.8): an opt-in gate on `add`/`commit`
/// against locks held by OTHERS on the LFS server. Never a lock manager —
/// the server stays the single source of truth (`POST locks/verify`, the
/// same ours/theirs split push already consumes) and the push-time check
/// remains the authoritative backstop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockEnforcePolicy {
    Off,
    Warn,
    Block,
}

impl LockEnforcePolicy {
    /// Case-insensitive; `off` is accepted explicitly so a repo can
    /// override a broader setting. Anything else is a hard usage error —
    /// a typo must not silently disable enforcement.
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "off" => Ok(Self::Off),
            "warn" => Ok(Self::Warn),
            "block" => Ok(Self::Block),
            other => Err(format!(
                "invalid lfs.lockEnforce value '{other}' (expected off, warn, or block)"
            )),
        }
    }
}

async fn load_lock_enforce_policy() -> CliResult<LockEnforcePolicy> {
    // ConfigKv is what `libra config set` writes (keys stored VERBATIM) —
    // the case-insensitive lookup accepts lfs.lockEnforce / lfs.lockenforce.
    let entry = ConfigKv::get_var_case_insensitive("lfs.", "lockEnforce")
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to read lfs.lockEnforce: {error}"))
                .with_stable_code(StableErrorCode::RepoStateInvalid)
        })?;
    match entry {
        None => Ok(LockEnforcePolicy::Off),
        Some(entry) => LockEnforcePolicy::parse(&entry.value).map_err(|message| {
            CliError::command_usage(message)
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("set it with: libra config lfs.lockEnforce off|warn|block")
        }),
    }
}

/// Gate `candidates` (repo-relative slash paths of the operation's staged
/// new/modified/DELETED set — deletions matter: they never reach the
/// push-time OID check) against server locks. Behavior matrix is pinned in
/// COMPATIBILITY.md; the notable calls: explicit offline intent skips with a
/// recorded warning in BOTH modes (deletion residual documented); an
/// unreachable server FAILS CLOSED under `block` (an opted-in hard
/// guarantee must not silently degrade on a flaky network — the
/// LIBRA_READ_POLICY discipline) and warns-and-proceeds under `warn`.
/// Repo-root-relative, forward-slash form (the git-index / LFS-lock-path
/// convention). Drops `.` and any leading components; joins `Normal` parts
/// with `/` so a Windows `sub\\file.bin` candidate matches the server's
/// `sub/file.bin` lock path.
fn normalize_lock_path(path: &str) -> String {
    use std::path::{Component, Path};
    Path::new(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) async fn enforce_lock_policy(candidates: &[String]) -> CliResult<()> {
    if candidates.is_empty() {
        return Ok(());
    }
    let workdir = util::working_dir();
    // Normalize to the git-index / LFS-lock convention: repo-root-relative,
    // forward-slash (candidates arrive repo-root-relative from `status`, but
    // may carry platform separators — a byte-for-byte compare against
    // slash-normalized server lock paths would else miss a lock on Windows).
    let normalized: Vec<String> = candidates
        .iter()
        .map(|path| normalize_lock_path(path))
        .collect();
    let lfs_candidates: Vec<&String> = normalized
        .iter()
        .filter(|path| lfs::is_lfs_tracked(workdir.join(path.as_str())))
        .collect();
    if lfs_candidates.is_empty() {
        return Ok(()); // zero overhead for non-LFS work
    }
    let policy = load_lock_enforce_policy().await?;
    if policy == LockEnforcePolicy::Off {
        return Ok(());
    }
    // Explicit offline intent: skip with a recorded warning (the operator
    // asked for no network; push-time verify still guards uploads).
    if crate::utils::read_policy::read_policy() == crate::utils::read_policy::ReadPolicy::LocalOnly
    {
        eprintln!(
            "warning: lfs.lockEnforce skipped (offline read policy); locks were NOT verified"
        );
        crate::utils::output::record_warning();
        return Ok(());
    }
    // Detached HEAD: refspec-scoped verification is undefined; push refuses
    // detached LFS pushes, so the backstop holds.
    let Some(refspec) = current_refspec().await else {
        eprintln!("warning: lfs.lockEnforce skipped (detached HEAD); locks were NOT verified");
        crate::utils::output::record_warning();
        return Ok(());
    };
    // Remote resolution. A fresh `switch -c` branch has no upstream (Ok(None))
    // — fall back to `remote.origin.url` so enforcement is NOT silently
    // skipped in a repo with a configured remote. But a real config/storage
    // ERROR must NOT be masked by the origin fallback (that would verify
    // against the wrong remote and proceed unverified) — surface it per
    // policy.
    let remote_url = match ConfigKv::get_current_remote_url().await {
        Ok(Some(url)) => Some(url),
        Ok(None) => ConfigKv::get_remote_url("origin").await.ok(),
        Err(error) => {
            return match policy {
                LockEnforcePolicy::Block => Err(CliError::fatal(format!(
                    "lfs.lockEnforce=block: cannot resolve the remote for lock verification: \
                     {error}"
                ))
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("use --offline to skip deliberately, or set lfs.lockEnforce warn")),
                _ => {
                    eprintln!("warning: lfs.lockEnforce: cannot resolve the remote: {error}");
                    crate::utils::output::record_warning();
                    Ok(())
                }
            };
        }
    };
    let Some(remote_url) = remote_url else {
        // Purely local repository: structural no-op.
        tracing::debug!("lfs.lockEnforce: no remote configured; nothing to verify");
        return Ok(());
    };
    let client = match LFSClient::from_remote_url(&remote_url) {
        Ok(client) => client,
        Err(error) => {
            return match policy {
                LockEnforcePolicy::Block => Err(CliError::fatal(format!(
                    "lfs.lockEnforce=block: cannot reach the LFS server: {error}"
                ))
                .with_stable_code(StableErrorCode::NetworkUnavailable)
                .with_hint("use --offline to skip deliberately, or set lfs.lockEnforce warn")),
                _ => {
                    eprintln!("warning: lfs.lockEnforce: cannot build the LFS client: {error}");
                    crate::utils::output::record_warning();
                    Ok(())
                }
            };
        }
    };
    let request = VerifiableLockRequest {
        refs: Ref { name: refspec },
        cursor: None,
        limit: None,
    };
    let (code, list) = match client.verify_locks(request).await {
        Ok(result) => result,
        Err(error) => {
            // Transport failure.
            return match policy {
                LockEnforcePolicy::Block => Err(CliError::network(format!(
                    "lfs.lockEnforce=block: lock verification failed: {error}"
                ))
                .with_hint("use --offline to skip deliberately, or set lfs.lockEnforce warn")),
                _ => {
                    eprintln!("warning: lfs.lockEnforce: lock verification failed: {error}");
                    crate::utils::output::record_warning();
                    Ok(())
                }
            };
        }
    };
    if code == StatusCode::NOT_FOUND {
        return Ok(()); // server has no locking API — mirror the push path
    }
    if code == StatusCode::FORBIDDEN {
        return match policy {
            LockEnforcePolicy::Block => Err(CliError::fatal(
                "lfs.lockEnforce=block: you must have push access to verify locks",
            )
            .with_stable_code(StableErrorCode::AuthPermissionDenied)),
            _ => {
                eprintln!("warning: lfs.lockEnforce: no push access to verify locks");
                crate::utils::output::record_warning();
                Ok(())
            }
        };
    }
    if !code.is_success() {
        // verify_locks already printed the server detail and returned an
        // EMPTY list — an empty list on a 5xx is an unverified state, not a
        // clean bill: apply the unreachable-server policy.
        return match policy {
            LockEnforcePolicy::Block => Err(CliError::network(format!(
                "lfs.lockEnforce=block: lock verification returned HTTP {code}"
            ))
            .with_hint("use --offline to skip deliberately, or set lfs.lockEnforce warn")),
            _ => {
                crate::utils::output::record_warning();
                Ok(())
            }
        };
    }
    // Only locks held by OTHERS gate the operation (ours = permission).
    let offending: Vec<&Lock> = list
        .theirs
        .iter()
        .filter(|lock| {
            lfs_candidates
                .iter()
                .any(|candidate| candidate.as_str() == lock.path)
        })
        .collect();
    if offending.is_empty() {
        return Ok(());
    }
    let describe = |lock: &Lock| {
        let owner = lock
            .owner
            .as_ref()
            .map(|user| user.name.clone())
            .unwrap_or_else(|| "(unknown)".to_string());
        format!(
            "'{}' is locked by {} (lock id {})",
            lock.path, owner, lock.id
        )
    };
    match policy {
        LockEnforcePolicy::Warn => {
            for lock in &offending {
                eprintln!("warning: {}", describe(lock));
            }
            crate::utils::output::record_warning();
            Ok(())
        }
        LockEnforcePolicy::Block => {
            let listing = offending
                .iter()
                .map(|lock| describe(lock))
                .collect::<Vec<_>>()
                .join("; ");
            Err(CliError::fatal(format!(
                "lfs.lockEnforce=block: {listing}"
            ))
            .with_stable_code(StableErrorCode::ConflictOperationBlocked)
            .with_hint("inspect with: libra lfs locks")
            .with_hint(
                "ask the owner to unlock, or set lfs.lockEnforce warn to proceed with a warning",
            ))
        }
        LockEnforcePolicy::Off => Ok(()),
    }
}

pub(crate) async fn current_refspec() -> Option<String> {
    match Head::current().await {
        Head::Branch(name) => Some(format!("refs/heads/{name}")),
        // Return None silently — every caller wraps the None branch in
        // a typed error (`current_refspec_or_err` → CliError, the
        // `lfs_client.rs` `push_objects` site → LfsPushError). Pre-fix
        // we also `emit_legacy_stderr("fatal: HEAD is detached")` here,
        // which doubled the error envelope on stderr (legacy line +
        // typed-error envelope from the caller), confusing `--json` /
        // `--machine` consumers.
        Head::Detached(_) => None,
    }
}

async fn current_refspec_or_err() -> CliResult<String> {
    current_refspec().await.ok_or_else(|| {
        CliError::fatal("HEAD is detached").with_stable_code(StableErrorCode::RepoStateInvalid)
    })
}

fn map_lock_list_error(error: LockListError) -> CliError {
    match error {
        LockListError::Request(detail) => {
            CliError::network(format!("failed to query LFS locks: {detail}"))
        }
        LockListError::Http { status, message } => {
            if status == StatusCode::FORBIDDEN {
                CliError::fatal("You must have push access to list locks")
                    .with_stable_code(StableErrorCode::AuthPermissionDenied)
            } else {
                CliError::network(format!(
                    "LFS get locks failed with status {}",
                    status.as_u16()
                ))
                .with_stable_code(StableErrorCode::NetworkProtocol)
                .with_detail("status", status.as_u16())
                .with_detail("body", message)
            }
        }
        LockListError::Decode(detail) => {
            CliError::network(format!("failed to decode LFS locks response: {detail}"))
                .with_stable_code(StableErrorCode::NetworkProtocol)
        }
    }
}

/// temp
fn convert_patterns_to_workdir(patterns: Vec<String>) -> Vec<String> {
    patterns
        .into_iter()
        .map(|p| util::to_workdir_path(&p).to_string_or_panic())
        .collect()
}

fn add_lfs_patterns(file_path: &str, patterns: Vec<String>) -> io::Result<Vec<String>> {
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(file_path)?;

    if file.metadata()?.len() > 0 {
        file.seek(SeekFrom::End(-1))?;

        let mut last_byte = [0; 1];
        file.read_exact(&mut last_byte)?;

        // ensure the last byte is '\n'
        if last_byte[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }

    let lfs_patterns = lfs::extract_lfs_patterns(file_path)?;
    let mut added: Vec<String> = Vec::new();
    for pattern in patterns {
        if lfs_patterns.contains(&pattern) || added.contains(&pattern) {
            continue;
        }
        added.push(pattern.clone());
        let pattern = format!(
            "{} filter=lfs diff=lfs merge=lfs -text\n",
            pattern.replace(" ", r"\ ")
        );
        file.write_all(pattern.as_bytes())?;
    }

    Ok(added)
}

fn untrack_lfs_patterns(file_path: &str, patterns: Vec<String>) -> io::Result<Vec<String>> {
    if !Path::new(file_path).exists() {
        return Ok(Vec::new());
    }
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);

    let mut lines: Vec<String> = Vec::new();
    let mut removed = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let mut matched_pattern = None;
        // delete the specified lfs patterns. We compare against the
        // on-disk (escaped-space) form, but record the *original* input
        // pattern in `removed` so the return value is symmetric with
        // `add_lfs_patterns` (both surface the un-escaped user-facing
        // form).
        for pattern in &patterns {
            let escaped = pattern.replace(" ", r"\ ");
            if line.trim_start().starts_with(&escaped) && line.contains("filter=lfs") {
                matched_pattern = Some(pattern.clone());
                break;
            }
        }
        match matched_pattern {
            Some(pattern) => removed.push(pattern),
            None => lines.push(line),
        }
    }

    // clear the file
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(file_path)?;

    for line in lines {
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_lock_list_error_forbidden_maps_to_auth_permission_denied() {
        let err = map_lock_list_error(LockListError::Http {
            status: StatusCode::FORBIDDEN,
            message: "forbidden".to_string(),
        });
        assert_eq!(err.stable_code(), StableErrorCode::AuthPermissionDenied);
        assert!(err.message().contains("push access"));
    }

    #[test]
    fn map_lock_list_error_decode_maps_to_network_protocol() {
        let err = map_lock_list_error(LockListError::Decode("invalid json".to_string()));
        assert_eq!(err.stable_code(), StableErrorCode::NetworkProtocol);
        assert!(err.message().contains("decode"));
    }

    #[test]
    fn map_lock_list_error_http_maps_status_and_body_detail() {
        let err = map_lock_list_error(LockListError::Http {
            status: StatusCode::BAD_GATEWAY,
            message: "upstream unavailable".to_string(),
        });
        assert_eq!(err.stable_code(), StableErrorCode::NetworkProtocol);
        assert_eq!(err.details().get("status"), Some(&serde_json::json!(502)));
        assert_eq!(
            err.details().get("body"),
            Some(&serde_json::json!("upstream unavailable"))
        );
    }

    #[test]
    fn add_lfs_patterns_deduplicates_within_a_single_call() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().to_string_lossy().into_owned();
        let added = add_lfs_patterns(
            &path,
            vec![
                "*.png".to_string(),
                "*.png".to_string(),
                "*.jpg".to_string(),
            ],
        )
        .expect("add_lfs_patterns");
        assert_eq!(added, vec!["*.png".to_string(), "*.jpg".to_string()]);

        let on_disk = lfs::extract_lfs_patterns(&path).expect("extract");
        assert_eq!(on_disk, vec!["*.png".to_string(), "*.jpg".to_string()]);
    }

    #[test]
    fn untrack_lfs_patterns_returns_unescaped_form_symmetric_with_add() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().to_string_lossy().into_owned();

        // Track a pattern with an internal space; on-disk form is escaped.
        let added =
            add_lfs_patterns(&path, vec!["my dir/*.png".to_string()]).expect("add_lfs_patterns");
        assert_eq!(added, vec!["my dir/*.png".to_string()]);

        // Untrack with the un-escaped user-facing form. The return value
        // must match the input, not the on-disk escaped form, so that
        // `LfsOutput.patterns` from track and untrack is symmetric.
        let removed = untrack_lfs_patterns(&path, vec!["my dir/*.png".to_string()])
            .expect("untrack_lfs_patterns");
        assert_eq!(removed, vec!["my dir/*.png".to_string()]);

        let on_disk = lfs::extract_lfs_patterns(&path).expect("extract");
        assert!(on_disk.is_empty(), "expected empty, got {on_disk:?}");
    }
}
