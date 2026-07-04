//! `libra agent list` — focused capability matrix for known agents (AG-17).
//!
//! The rows derive from the AG-16 static registry
//! (`observed_agents::registry`), overlaid with the runtime installation
//! state for hook-installable agents. The `--json` shape is a frozen public
//! contract (see `docs/development/tracing/agent.md` AG-17): key set changes
//! require a schema bump and a compat-test update in the same PR.
//!
//! Per E9, non-first-batch agents are listed (they stay `registered` so
//! historical data remains discoverable) but must never surface as
//! `supported`, `hook_installable`, `installed` or `launchable_*`.

use clap::Args;
use serde::Serialize;

use crate::{
    internal::ai::observed_agents::{
        AgentKind, AgentStability, DeclaredAgentCaps, agent_for, registration_for,
    },
    utils::{
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
    },
};

/// Bump when the `list --json` row shape changes (frozen AG-17 contract).
pub const AGENT_LIST_SCHEMA_VERSION: u32 = 1;

#[derive(Args, Debug)]
pub struct ListArgs {}

#[derive(Debug, Serialize)]
struct ListOutput {
    schema_version: u32,
    agents: Vec<ListAgentRow>,
}

/// One agent row. Field set frozen per AG-17 (`slug`/`agent_kind`/
/// `stability`/`hook_installable`/`installed`/`transcript_readable`/
/// `external_binary` + the roster fields `supported`/`support_wave`/
/// `launchable_*`; the install-state wire key is `installed`).
#[derive(Debug, Serialize)]
struct ListAgentRow {
    slug: &'static str,
    agent_kind: &'static str,
    db_value: &'static str,
    provider_name: &'static str,
    stability: AgentStability,
    supported: bool,
    /// `null` for rows outside the supported roster — the key itself is
    /// part of the frozen row shape.
    support_wave: Option<&'static str>,
    registered: bool,
    transcript_readable: bool,
    hook_installable: bool,
    installed: bool,
    launchable_review: bool,
    launchable_investigate: bool,
    external_binary: bool,
    config_paths: &'static [&'static str],
    protected_dirs: &'static [&'static str],
    capabilities: DeclaredAgentCaps,
}

pub async fn execute_safe(_args: ListArgs, output: &OutputConfig) -> CliResult<()> {
    let mut agents = Vec::with_capacity(AgentKind::all().len());
    for kind in AgentKind::all() {
        let row = registration_for(*kind);
        let adapter = agent_for(*kind);
        // Runtime install state is only meaningful (and per E9 only
        // allowed) for supported, hook-installable agents; everything
        // else is pinned to `installed=false` regardless of leftover
        // provider config (doctor surfaces residual legacy hooks). An
        // inspection failure (e.g. unreadable provider settings) is a
        // real error, not `installed=false`.
        let installed = match adapter.as_hooks() {
            Some(hooks) if row.hook_installable => hooks.hooks_are_installed().map_err(|err| {
                CliError::fatal(format!(
                    "failed to inspect '{}' hook installation state: {err}",
                    row.slug
                ))
            })?,
            _ => false,
        };
        agents.push(ListAgentRow {
            slug: row.slug,
            agent_kind: row.agent_kind,
            db_value: row.db_value,
            provider_name: adapter.provider_name(),
            stability: adapter.stability(),
            supported: row.supported,
            support_wave: row.support_wave,
            registered: row.registered,
            transcript_readable: row.transcript_readable,
            hook_installable: row.hook_installable,
            installed,
            launchable_review: row.launchable_review,
            launchable_investigate: row.launchable_investigate,
            external_binary: row.external_binary,
            config_paths: row.config_paths,
            protected_dirs: adapter.protected_dirs(),
            capabilities: row.capabilities,
        });
    }

    let payload = ListOutput {
        schema_version: AGENT_LIST_SCHEMA_VERSION,
        agents,
    };
    if output.is_json() {
        return emit_json_data("agent_list", &payload, output);
    }
    if output.quiet {
        return Ok(());
    }
    println!(
        "{:<13} {:<12} {:<11} {:<10} {:<11}",
        "SLUG", "WAVE", "HOOKS", "INSTALLED", "TRANSCRIPT"
    );
    for row in &payload.agents {
        println!(
            "{:<13} {:<12} {:<11} {:<10} {:<11}",
            row.slug,
            row.support_wave.unwrap_or("-"),
            if row.hook_installable {
                "installable"
            } else {
                "-"
            },
            if row.installed { "yes" } else { "no" },
            if row.transcript_readable {
                "readable"
            } else {
                "-"
            },
        );
    }
    println!(
        "\nSupported roster: {}. Use 'libra agent add <name>' to install hooks.",
        payload
            .agents
            .iter()
            .filter(|row| row.supported)
            .map(|row| row.slug)
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}
