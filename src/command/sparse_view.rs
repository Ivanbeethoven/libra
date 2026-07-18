//! `libra sparse-view` — manage the READ-ONLY sparse view filter (lore.md 2.2).
//!
//! A Libra extension, deliberately NOT named `sparse-checkout` (which stays
//! declined, D10): it NEVER touches the working tree. It stores an allowlist of
//! gitignore-syntax include patterns that scope what `ls-files` and `diff`
//! (working-tree only) DISPLAY. `status` stays fully honest (it only notes that
//! a view is active) so it never lies about what `commit` will record.

use clap::{Parser, Subcommand};

use crate::{
    internal::sparse::SparseViewStore,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
    },
};

pub const SPARSE_VIEW_EXAMPLES: &str = "\
EXAMPLES:
    libra sparse-view set 'src/**' 'docs/**'   Scope ls-files/diff to these paths
    libra sparse-view add '!src/gen/**'        Carve a hole out of the view
    libra sparse-view list                     Show the patterns
    libra sparse-view status                   Show enabled state + pattern count
    libra sparse-view disable                  Turn the view off (patterns kept)
    libra sparse-view clear                    Drop all patterns and disable

NOTE: this is a READ-ONLY display filter over `ls-files` and `diff` — it never
changes the working tree (unlike git sparse-checkout).";

/// Manage the read-only sparse view filter (lore.md 2.2).
#[derive(Parser, Debug)]
#[command(after_help = SPARSE_VIEW_EXAMPLES)]
pub struct SparseViewArgs {
    #[command(subcommand)]
    pub command: SparseViewCommand,
}

#[derive(Subcommand, Debug)]
pub enum SparseViewCommand {
    /// Replace the pattern list and enable the view.
    Set {
        /// gitignore-syntax include patterns.
        #[arg(required = true)]
        patterns: Vec<String>,
    },
    /// Append patterns and enable the view.
    Add {
        #[arg(required = true)]
        patterns: Vec<String>,
    },
    /// Show the ordered patterns.
    List,
    /// Enable the view (patterns unchanged).
    Enable,
    /// Disable the view (patterns kept).
    Disable,
    /// Drop all patterns and disable the view.
    Clear,
    /// Show the enabled state and pattern count.
    Status,
}

pub async fn execute_safe(args: SparseViewArgs, output: &OutputConfig) -> CliResult<()> {
    crate::utils::util::require_repo().map_err(|_| CliError::repo_not_found())?;
    // Part C W0 (§C.11 transition guard): `sparse_view` rows and
    // `sparse.enabled` are repository-global, but the matcher/hydrate acts on
    // the current workdir — so one worktree set/clear/enable would change
    // another's ls-files/diff/materialization. All subcommands fail closed in a
    // linked worktree until W1 scopes the SparseViewStore call chain.
    crate::command::ensure_main_worktree_because(
        "sparse-view",
        "sparse view state is not yet worktree-scoped",
    )?;
    match args.command {
        SparseViewCommand::Set { patterns } => {
            validate_patterns(&patterns)?;
            SparseViewStore::replace(&patterns)
                .await
                .map_err(store_err)?;
            done(
                output,
                &format!("sparse view set ({} pattern(s), enabled)", patterns.len()),
            )
        }
        SparseViewCommand::Add { patterns } => {
            validate_patterns(&patterns)?;
            SparseViewStore::add(&patterns).await.map_err(store_err)?;
            done(
                output,
                &format!("added {} pattern(s) (enabled)", patterns.len()),
            )
        }
        SparseViewCommand::List => {
            let patterns = SparseViewStore::list().await.map_err(store_err)?;
            if output.is_json() {
                return emit_json_data(
                    "sparse-view",
                    &serde_json::json!({ "patterns": patterns }),
                    output,
                );
            }
            if patterns.is_empty() && !output.quiet {
                println!("no sparse-view patterns");
            } else {
                for p in &patterns {
                    println!("{p}");
                }
            }
            Ok(())
        }
        SparseViewCommand::Enable => {
            SparseViewStore::enable().await.map_err(store_err)?;
            done(output, "sparse view enabled")
        }
        SparseViewCommand::Disable => {
            SparseViewStore::disable().await.map_err(store_err)?;
            done(output, "sparse view disabled")
        }
        SparseViewCommand::Clear => {
            SparseViewStore::clear().await.map_err(store_err)?;
            done(output, "sparse view cleared and disabled")
        }
        SparseViewCommand::Status => {
            let enabled = SparseViewStore::is_enabled().await;
            let patterns = SparseViewStore::list().await.map_err(store_err)?;
            if output.is_json() {
                return emit_json_data(
                    "sparse-view",
                    &serde_json::json!({ "enabled": enabled, "pattern_count": patterns.len() }),
                    output,
                );
            }
            if !output.quiet {
                println!(
                    "sparse view: {} ({} pattern(s))",
                    if enabled { "enabled" } else { "disabled" },
                    patterns.len()
                );
            }
            Ok(())
        }
    }
}

fn validate_patterns(patterns: &[String]) -> CliResult<()> {
    for pattern in patterns {
        if pattern.trim().is_empty() {
            return Err(CliError::command_usage("empty sparse-view pattern")
                .with_stable_code(StableErrorCode::CliInvalidArguments));
        }
    }
    Ok(())
}

fn store_err(e: String) -> CliError {
    CliError::fatal(e).with_stable_code(StableErrorCode::IoWriteFailed)
}

fn done(output: &OutputConfig, message: &str) -> CliResult<()> {
    if !output.quiet && !output.is_json() {
        println!("{message}");
    }
    Ok(())
}
