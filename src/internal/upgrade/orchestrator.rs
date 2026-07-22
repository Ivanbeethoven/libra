//! Auto-upgrade orchestration and startup hooks (plan-20260714 §A.7/§A.8/
//! §A.10).
//!
//! This module composes the verified pieces into the two things the CLI
//! calls at startup:
//!
//! - [`startup_recovery_gate`] — runs BEFORE repo preflight/dispatch and,
//!   if a crashed install transaction is present, drives it to a terminal
//!   state (§A.7). A fatal, unclassifiable transaction stops the process
//!   before any user command runs; a clean recovery or the (overwhelmingly
//!   common) no-transaction case returns quietly.
//! - [`run_auto_upgrade_check`] — the `upgrade.mode=auto` check that fetches
//!   the signed manifest, decides, downloads + probes a candidate and
//!   installs it under the §A.5 lock. Every failure is isolated so it can
//!   never break the user's actual command (§A.8).
//!
//! Both short-circuit to a no-op before any network or filesystem work when
//! the compiled trust table is empty — which it is in production until the
//! release-key ceremony (§A.6). The auto-upgrade path is therefore inert by
//! construction until keys are provisioned.

use std::{path::PathBuf, time::Duration};

use super::{
    flow::{DecisionContext, UpgradeDecision, decide_from_envelope},
    http::{download_artifact_to, fetch_manifest, upgrade_http_client},
    lock::InstallDir,
    manifest::{MANIFEST_URL, ReleaseVersion},
    marker::{TARGET_BINARY_NAME, official_marker_for_target},
    platform::{Platform, PlatformSupport},
    probe,
    settings::{UpgradeMode, effective_mode_for_upgrade},
    state::{
        UpgradeState, backoff_defers, cooldown_permits_skip, read_state, register_failure_backoff,
        write_state,
    },
    trusted_keys::active_trust_table,
    txn::{self, CANDIDATE_NAME, OldTarget, TxnError, TxnOutcome},
};
use crate::utils::error::{CliError, CliResult};

/// Total Phase-A soft budget: 5 s manifest + 10 s download (§A.7).
pub const UPGRADE_BUDGET: Duration = Duration::from_secs(15);
/// Per-probe hard timeout for the recovery/post-install self-check.
pub const RECOVERY_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// What the auto-upgrade check did this invocation (for the CLI to surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoUpgradeReport {
    /// Nothing applicable / not the right moment / inert.
    Skipped,
    /// A newer version was installed.
    Installed(ReleaseVersion),
    /// An install was attempted but rolled back to the previous target.
    RolledBack,
}

/// Resolved install context: the validated directory and the running
/// platform. `None` whenever this binary is not an upgrade-manageable
/// official-style install (unresolvable path, failed §A.5 validation, or a
/// platform outside the release matrix) — always a non-fatal skip.
struct InstallContext {
    dir: InstallDir,
    dir_path: PathBuf,
    platform: Platform,
}

fn resolve_install_context() -> Option<InstallContext> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    // The normal installed target is named `libra`; anything else (a dev
    // binary, a renamed copy) is not upgrade-manageable.
    if exe.file_name()?.to_str()? != TARGET_BINARY_NAME {
        return None;
    }
    let dir_path = exe.parent()?.to_path_buf();
    let dir = InstallDir::open_validated(&dir_path).ok()?;
    let platform = Platform::current()?;
    Some(InstallContext {
        dir,
        dir_path,
        platform,
    })
}

/// Synchronous, bounded post-install self-check used during recovery (the
/// recovery path is not inside an obvious async context, so it spawns the
/// target with `std::process` and enforces its own timeout + group kill).
fn sync_post_install_probe(
    dir: &InstallDir,
    expected_version: &str,
    timeout: Duration,
) -> Result<bool, TxnError> {
    let target = dir.path().join(TARGET_BINARY_NAME);
    probe::run_sync_probe(&target, "post-install", expected_version, timeout)
        .map(|o| o.is_healthy())
        .map_err(|e| TxnError::Serde(format!("recovery probe failed to spawn: {e}")))
}

/// Startup recovery gate (§A.7/§A.10). Must run before repo preflight and
/// user-command dispatch.
///
/// - No install context / no transaction ⇒ `Ok(())` (the common case).
/// - A clean recovery (commit / rollback / abort) ⇒ `Ok(())`, with an
///   advisory note on rollback.
/// - A fatal, unclassifiable transaction or corrupt anti-rollback state ⇒
///   `Err`, so the process exits before running the user's command.
pub async fn startup_recovery_gate() -> CliResult<()> {
    // Inert until keys exist: with no trust table there can be no official
    // signed install, hence no upgrade transaction to recover.
    if active_trust_table().is_empty() {
        return Ok(());
    }
    let Some(ctx) = resolve_install_context() else {
        return Ok(());
    };
    // A corrupt state file is fatal for the upgrade subsystem (§A.7): refuse
    // to proceed rather than silently discard anti-rollback history.
    if let Err(err) = read_state(&ctx.dir_path) {
        return Err(CliError::fatal(err.to_string())
            .with_stable_code(crate::utils::error::StableErrorCode::RepoStateInvalid));
    }
    let version = env!("CARGO_PKG_VERSION").to_string();
    let outcome = tokio::task::spawn_blocking(move || {
        let probe =
            move |dir: &InstallDir| sync_post_install_probe(dir, &version, RECOVERY_PROBE_TIMEOUT);
        txn::recover(&ctx.dir, &probe)
    })
    .await
    .map_err(|e| CliError::fatal(format!("upgrade recovery task failed: {e}")))?;

    match outcome {
        Ok(TxnOutcome::RolledBack) => {
            crate::utils::error::emit_advisory_warning(
                "a previous auto-upgrade failed its self-check and was rolled back to the prior version",
            );
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(TxnError::FatalRecovery { detail, .. }) => Err(CliError::fatal(format!(
            "auto-upgrade is in an unrecoverable state and must be repaired manually: {detail}"
        ))
        .with_stable_code(crate::utils::error::StableErrorCode::RepoStateInvalid)),
        Err(other) => {
            // Non-fatal recovery hiccup: never block the user's command.
            crate::utils::error::emit_advisory_warning(format!(
                "auto-upgrade recovery could not complete this time: {other}"
            ));
            Ok(())
        }
    }
}

/// Pure throttle gate (§A.6 缓存/节流): should this invocation actually go
/// online to check, given the mode, whether keys exist, and the persisted
/// cooldown/backoff. Split out so it is unit-testable.
pub fn should_check_now(
    mode: UpgradeMode,
    trust_is_empty: bool,
    state: &UpgradeState,
    local_now: i64,
) -> bool {
    if mode != UpgradeMode::Auto || trust_is_empty {
        return false;
    }
    if backoff_defers(state, local_now) {
        return false;
    }
    !cooldown_permits_skip(state, local_now)
}

/// Run the `upgrade.mode=auto` check (§A.8). Never returns an error: every
/// failure degrades to [`AutoUpgradeReport::Skipped`] so the user's command
/// is unaffected.
pub async fn run_auto_upgrade_check(local_now: i64) -> AutoUpgradeReport {
    let trust = active_trust_table();
    // Fast, allocation-free short-circuits in the common case.
    if trust.is_empty() {
        return AutoUpgradeReport::Skipped;
    }
    if effective_mode_for_upgrade() != UpgradeMode::Auto {
        return AutoUpgradeReport::Skipped;
    }
    let Some(ctx) = resolve_install_context() else {
        return AutoUpgradeReport::Skipped;
    };
    if ctx.platform.support() != PlatformSupport::Supported {
        return AutoUpgradeReport::Skipped;
    }
    // An install we did not sign is not eligible for auto-upgrade (§A.2).
    if official_marker_for_target(&ctx.dir, ctx.platform.as_str())
        .ok()
        .flatten()
        .is_none()
    {
        return AutoUpgradeReport::Skipped;
    }
    let Ok(state) = read_state(&ctx.dir_path) else {
        return AutoUpgradeReport::Skipped;
    };
    if !should_check_now(UpgradeMode::Auto, false, &state, local_now) {
        return AutoUpgradeReport::Skipped;
    }

    // Phase A: fetch + decide + download + candidate probe, under the budget.
    match tokio::time::timeout(UPGRADE_BUDGET, phase_a(&ctx, &state, trust, local_now)).await {
        Ok(Ok(Some(plan))) => phase_b(&ctx, plan).await,
        Ok(Ok(None)) => AutoUpgradeReport::Skipped,
        Ok(Err(())) | Err(_) => {
            // Any failure or timeout: record a backoff and skip. Errors in
            // persisting the backoff are themselves swallowed.
            let backed_off = register_failure_backoff(&state, local_now);
            let _ = write_state(&ctx.dir_path, &backed_off);
            AutoUpgradeReport::Skipped
        }
    }
}

/// The install plan plus the verified candidate already staged on disk.
struct StagedInstall {
    version: ReleaseVersion,
    marker: super::marker::InstallMarker,
    new_state: UpgradeState,
    old_target: OldTarget,
}

/// Phase A: fetch the manifest, decide, download+verify the candidate, run
/// the candidate self-check. `Ok(Some(_))` means "ready to install".
async fn phase_a(
    ctx: &InstallContext,
    state: &UpgradeState,
    trust: &[super::trusted_keys::TrustedKey],
    local_now: i64,
) -> Result<Option<StagedInstall>, ()> {
    let client = upgrade_http_client().map_err(|_| ())?;
    let fetched = fetch_manifest(&client, MANIFEST_URL)
        .await
        .map_err(|_| ())?;
    let https_date = fetched
        .https_date
        .as_deref()
        .and_then(parse_http_date_to_unix);

    let installed = ReleaseVersion::parse(env!("CARGO_PKG_VERSION")).ok_or(())?;
    let ctx_dec = DecisionContext {
        state,
        https_date,
        local_now,
        trust,
        platform: Some(ctx.platform),
        installed_version: installed,
        installed_at_rfc3339: &now_rfc3339(local_now),
    };
    let decision = decide_from_envelope(&ctx_dec, &fetched.bytes).map_err(|_| ())?;
    let plan = match decision {
        UpgradeDecision::Install(plan) => plan,
        UpgradeDecision::Skip(_) => return Ok(None),
    };

    // Download the artifact into memory (SizeGate-bounded to ≤128 MiB), then
    // stage it as the candidate file via the fd-relative writer.
    let mut buf: Vec<u8> = Vec::new();
    download_artifact_to(
        &client,
        &plan.artifact.url,
        plan.artifact.size,
        &plan.artifact.sha256,
        &mut buf,
    )
    .await
    .map_err(|_| ())?;
    ctx.dir
        .write_file_atomic(CANDIDATE_NAME, &buf, 0o755)
        .map_err(|_| ())?;

    // Candidate self-check before we trust it (§A.7 Phase A probe).
    let candidate_path = ctx.dir.path().join(CANDIDATE_NAME);
    let outcome = probe::run_phase_a_probe(
        &candidate_path,
        probe::ProbeKind::PreInstall,
        &plan.version.to_string(),
        RECOVERY_PROBE_TIMEOUT,
    )
    .await;
    if !outcome.is_healthy() {
        let _ = ctx.dir.remove_file(CANDIDATE_NAME);
        return Err(());
    }

    let old_target = current_old_target(ctx).map_err(|_| ())?;
    Ok(Some(StagedInstall {
        version: plan.version,
        marker: plan.marker,
        new_state: plan.new_state,
        old_target,
    }))
}

/// Phase B: install the staged candidate under the §A.5 lock via the
/// transaction, running the post-install self-check.
async fn phase_b(ctx: &InstallContext, staged: StagedInstall) -> AutoUpgradeReport {
    let version = staged.version;
    let dir_path = ctx.dir_path.clone();
    // Re-open a dedicated InstallDir for the blocking transaction task.
    let outcome = tokio::task::spawn_blocking(move || {
        let dir = InstallDir::open_validated(&dir_path).map_err(TxnError::Dir)?;
        let Some(_lock) = dir.try_lock()? else {
            // Another process holds the lock — skip this round.
            return Ok(TxnOutcome::NoOp);
        };
        let new_hash = staged.marker.sha256.clone();
        let expected = version.to_string();
        let probe =
            move |d: &InstallDir| sync_post_install_probe(d, &expected, RECOVERY_PROBE_TIMEOUT);
        txn::run_install(
            &dir,
            staged.old_target,
            &version.to_string(),
            &new_hash,
            staged.marker,
            staged.new_state,
            &probe,
        )
    })
    .await;

    match outcome {
        Ok(Ok(TxnOutcome::Installed)) => AutoUpgradeReport::Installed(version),
        Ok(Ok(TxnOutcome::RolledBack)) => AutoUpgradeReport::RolledBack,
        _ => AutoUpgradeReport::Skipped,
    }
}

/// Snapshot the current target as the transaction's `old_target`.
fn current_old_target(ctx: &InstallContext) -> Result<OldTarget, TxnError> {
    use super::lock::EntryKind;
    match ctx.dir.stat_entry(TARGET_BINARY_NAME)? {
        Some(EntryKind::Regular { .. }) => {
            let bytes = ctx.dir.read_file(TARGET_BINARY_NAME)?.unwrap_or_default();
            use sha2::Digest as _;
            let hash = hex::encode(sha2::Sha256::digest(&bytes));
            let marker_snapshot = official_marker_for_target(&ctx.dir, ctx.platform.as_str())
                .ok()
                .flatten();
            Ok(OldTarget::Present {
                hash,
                marker_snapshot,
            })
        }
        _ => Ok(OldTarget::Absent),
    }
}

/// Parse an HTTP `Date` header to unix seconds (RFC 1123 / RFC 850 / asctime,
/// via `chrono`). `None` on any parse failure — the caller then rejects the
/// round (§A.6 requires a usable HTTPS Date).
fn parse_http_date_to_unix(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(raw.trim())
        .ok()
        .map(|dt| dt.timestamp())
}

/// RFC3339 timestamp for a unix-seconds instant (marker `installed_at`).
fn now_rfc3339(local_now: i64) -> String {
    chrono::DateTime::from_timestamp(local_now, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_check_gates_on_mode_and_trust() {
        let state = UpgradeState::default();
        // Off mode never checks.
        assert!(!should_check_now(UpgradeMode::Off, false, &state, 1_000));
        assert!(!should_check_now(UpgradeMode::Manual, false, &state, 1_000));
        // Auto with empty trust never checks (inert in production).
        assert!(!should_check_now(UpgradeMode::Auto, true, &state, 1_000));
        // Auto with keys and no throttle ⇒ check.
        assert!(should_check_now(UpgradeMode::Auto, false, &state, 1_000));
    }

    #[test]
    fn should_check_respects_backoff_and_cooldown() {
        let state = register_failure_backoff(&UpgradeState::default(), 1_000);
        // Backoff defers.
        assert!(!should_check_now(UpgradeMode::Auto, false, &state, 1_000));
        // After the backoff window, it checks again.
        assert!(should_check_now(
            UpgradeMode::Auto,
            false,
            &state,
            1_000 + state.backoff_seconds + 1
        ));

        // A live cooldown skips checking.
        let cooled = UpgradeState {
            trusted_time_floor: 10_000,
            next_success_check_not_before: Some(10_600),
            ..Default::default()
        };
        assert!(!should_check_now(UpgradeMode::Auto, false, &cooled, 10_100));
        // Past the cooldown, it checks.
        assert!(should_check_now(UpgradeMode::Auto, false, &cooled, 10_601));
    }

    #[test]
    fn http_date_parsing_rejects_garbage() {
        assert!(parse_http_date_to_unix("not a date").is_none());
        let ts = parse_http_date_to_unix("Wed, 01 Jul 2026 00:00:00 GMT")
            .expect("test fixture operation should succeed");
        assert!(ts > 1_700_000_000);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn auto_check_is_inert_with_empty_trust_table() {
        // Production trust table is empty ⇒ the check does no I/O and skips.
        assert_eq!(
            run_auto_upgrade_check(1_000).await,
            AutoUpgradeReport::Skipped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn recovery_gate_is_inert_with_empty_trust_table() {
        assert!(startup_recovery_gate().await.is_ok());
    }
}
