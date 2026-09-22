//! Fork the current ScorpioFS-backed worktree.

use clap::Parser;

use crate::utils::error::{CliResult, StableErrorCode};
use crate::utils::output::OutputConfig;

pub const FORK_EXAMPLES: &str = r#"Examples:
  libra fork ../experiment

Fork the current ScorpioFS-backed worktree into a new materialized worktree.
The parent and child receive independent upper layers."#;

#[derive(Parser, Debug)]
#[command(after_help = FORK_EXAMPLES)]
pub struct ForkArgs {
    /// Target path for the new linked worktree.
    pub path: String,
    /// Create NEW_BRANCH (from the source HEAD) and attach it to the child, so the
    /// fork can `sync` immediately. Omitted: detached child.
    #[arg(short = 'b', long = "create-branch", value_name = "NEW_BRANCH")]
    pub new_branch: Option<String>,
}

pub async fn execute_safe(args: ForkArgs, _output: &OutputConfig) -> CliResult<()> {
    let path = crate::command::worktree::fork_scorpiofs_worktree(args.path, args.new_branch)
        .await
        .map_err(|error| {
            crate::utils::error::CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
    println!("ScorpioFS worktree forked at {path}");
    Ok(())
}
