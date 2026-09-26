//! Compact stage+commit+push+finalize workflow for a ScorpioFS-mounted worktree.
//!
//! The command deliberately delegates each step to Libra's existing command
//! implementations. ScorpioFS supplies the POSIX worktree and the effective-diff
//! report; it never writes Libra objects, the index, refs, or remote state.
//!
//! Against a Worktree-v2 daemon the flow is a four-step transaction
//! (docs/scorpiofs-libra-complete-spec-v1.md §7.3):
//!
//! 1. `GET /worktrees/{mount}/state` — effective diff + optimistic `generation`;
//! 2. `add -A` and one commit;
//! 3. push to the trunk;
//! 4. `POST /worktrees/{mount}/commit-finalize` — the daemon pins its lower
//!    projection to the pushed revision and removes exactly the committed upper
//!    entries. Only a `ready` finalize means the worktree is truly synchronized;
//!    a push without a finalize leaves the stale overlay in place, shadowing the
//!    lower until a retry succeeds.
//!
//! `push` success alone therefore is never reported as a completed sync.

use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::{
    command::{add, checkout, commit, push},
    utils::{
        error::{CliError, CliResult},
        output::OutputConfig,
        util,
    },
};

pub const SYNC_EXAMPLES: &str = r#"Examples:
  libra sync -m "Update generated sources"
  libra sync

`sync` stages the whole current worktree, creates one commit, pushes using this
branch's configured upstream, and (on a ScorpioFS-backed worktree) finalizes the
commit so the mounted projection moves onto the pushed revision."#;

#[derive(Parser, Debug, Default)]
#[command(after_help = SYNC_EXAMPLES)]
pub struct SyncArgs {
    /// Commit message. When omitted, generate a stable message from the current
    /// worktree instead of opening an interactive editor.
    #[arg(short, long)]
    pub message: Option<String>,
}

/// Daemon-side view of a ScorpioFS worktree (`GET /worktrees/{mount}/state`).
#[derive(Debug, Deserialize)]
pub(crate) struct ScorpioState {
    #[serde(default)]
    pub(crate) base_revision: Option<String>,
    pub(crate) generation: u64,
    #[serde(default)]
    pub(crate) dirty: bool,
    #[serde(default)]
    pub(crate) changes: Vec<ScorpioChange>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ScorpioChange {
    pub(crate) path: String,
    /// `added` | `modified` | `deleted` (Worktree v2), or v1 `modified`/`deleted`.
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) content_hash: Option<String>,
}

#[derive(Serialize)]
struct FinalizeRequest<'a> {
    expected_base_revision: Option<&'a str>,
    expected_generation: u64,
    new_base_revision: Option<&'static str>,
    committed_paths: Vec<FinalizePath<'a>>,
}

#[derive(Serialize)]
struct FinalizePath<'a> {
    path: &'a str,
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_hash: Option<&'a str>,
}

#[derive(Deserialize)]
struct FinalizeResponse {
    state: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    detail: Option<String>,
}

pub(crate) fn scorpio_endpoint() -> String {
    std::env::var("LIBRA_SCORPIOFS_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:2725/antares".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// The ScorpioFS mount id backing this worktree, if any.
pub(crate) fn current_mount_id() -> Option<String> {
    util::try_get_worktree_gitdir(None)
        .ok()?
        .join("scorpiofs_mount_id")
        .read_to_string_if_exists()
}

trait ReadToStringIfExists {
    fn read_to_string_if_exists(&self) -> Option<String>;
}

impl ReadToStringIfExists for std::path::Path {
    fn read_to_string_if_exists(&self) -> Option<String> {
        std::fs::read_to_string(self)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

pub(crate) fn http() -> reqwest::Client {
    reqwest::Client::new()
}

pub async fn execute_safe(args: SyncArgs, output: &OutputConfig) -> CliResult<()> {
    let mount_id = current_mount_id();
    let endpoint = scorpio_endpoint();

    // Step 1: read the daemon's effective diff as the optimistic-lock baseline.
    let state = match &mount_id {
        Some(id) => {
            let url = format!("{endpoint}/worktrees/{id}/state");
            match http().get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    Some(response.json::<ScorpioState>().await.map_err(|e| {
                        CliError::fatal(format!("invalid ScorpioFS worktree state: {e}"))
                    })?)
                }
                // Legacy daemon without Worktree v2: still sync, but say plainly
                // that the mounted projection will stay behind the pushed commit.
                _ => {
                    eprintln!(
                        "warning: ScorpioFS daemon does not serve Worktree v2 state; \
                         the mounted projection will not be finalized"
                    );
                    None
                }
            }
        }
        None => None,
    };

    if let Some(state) = &state {
        if !state.dirty {
            println!("worktree already synchronized with the mounted projection");
            return Ok(());
        }
    }

    // Step 2+3: stage everything, commit once, push to the trunk.
    add::execute_safe(add::AddArgs::all_worktree(), output).await?;

    let message = args
        .message
        .unwrap_or_else(|| "Sync ScorpioFS worktree".to_string());
    commit::execute_safe(
        commit::CommitArgs {
            message: Some(message),
            ..Default::default()
        },
        output,
    )
    .await?;

    let branch = checkout::get_current_branch().await.ok_or_else(|| {
        CliError::fatal(
            "sync requires an attached branch; use `libra switch -c <branch>` before syncing",
        )
    })?;
    let refspec = format!("refs/heads/{branch}:refs/heads/main");
    push::execute_safe(
        push::PushArgs::for_refspecs("origin".to_string(), vec![refspec]),
        output,
    )
    .await?;

    // Step 4: finalize — pin the daemon's lower to the pushed revision and clear
    // the committed upper entries. This is what makes "sync succeeded" true.
    if let (Some(id), Some(state)) = (&mount_id, &state) {
        if let Err(error) = finalize_mount(&endpoint, id, state).await {
            return Err(CliError::fatal(format!(
                "pushed the commit but failed to finalize the mounted worktree: {error}\n\
                 the mount still serves the previous revision; retry `libra sync`"
            )));
        }
    }

    Ok(())
}

async fn finalize_mount(
    endpoint: &str,
    mount_id: &str,
    state: &ScorpioState,
) -> Result<(), String> {
    // The committed set is exactly the effective diff read before staging. A path
    // edited since then makes the finalize fail its per-path hash check — which is
    // the optimistic lock working as designed.
    let committed: Vec<FinalizePath> = state
        .changes
        .iter()
        .map(|change| FinalizePath {
            path: &change.path,
            kind: &change.kind,
            content_hash: change.content_hash.as_deref(),
        })
        .collect();

    let request = FinalizeRequest {
        expected_base_revision: state.base_revision.as_deref(),
        expected_generation: state.generation,
        new_base_revision: None, // the daemon resolves the just-pushed revision
        committed_paths: committed,
    };

    let url = format!("{endpoint}/worktrees/{mount_id}/commit-finalize");
    let response = http()
        .post(&url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("finalize request failed: {e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("finalize returned HTTP {status}: {body}"));
    }
    let parsed: FinalizeResponse = response
        .json()
        .await
        .map_err(|e| format!("finalize response parse failed: {e}"))?;
    if parsed.state != "ready" {
        return Err(format!(
            "finalize did not complete ({}){}",
            parsed.state,
            parsed.detail.map(|d| format!(": {d}")).unwrap_or_default()
        ));
    }
    Ok(())
}
