//! Upgrade decision pipeline (plan-20260714 §A.6/§A.7/§A.10).
//!
//! [`decide_from_envelope`] is the pure heart of the auto-upgrade flow: given
//! the raw manifest bytes, the HTTPS `Date`, the local clock, the persisted
//! anti-rollback state, the compiled trust table, the running platform and
//! the installed version, it produces a single [`UpgradeDecision`] — install
//! a specific artifact (with the marker and anti-rollback state to persist on
//! commit) or skip with a reason. It layers, in order:
//!
//! 1. cryptographic + semantic manifest verification
//!    ([`super::manifest::verify_envelope_bytes`]);
//! 2. anti-rollback / time-policy evaluation against durable state
//!    ([`super::state::evaluate_manifest`]);
//! 3. platform support (Windows is published-but-unsupported in R0),
//!    `paused`, `revoked_versions`, and the "is it actually newer" gate;
//! 4. artifact selection for the running platform.
//!
//! The network fetch, candidate download, probe execution and locked
//! transaction that ACT on an `Install` decision live in the orchestration
//! wrappers (`phase_a`/`phase_b`) and are exercised end-to-end by the
//! `test-upgrade` integration target (§A.11). Keeping the decision pure lets
//! every §A.6/§A.7 branch be unit-tested deterministically.

use super::{
    manifest::{ManifestError, ReleaseVersion, VerifiedArtifact, VerifiedManifest},
    marker::{InstallMarker, OFFICIAL_INSTALL_SOURCE},
    platform::{Platform, PlatformSupport},
    state::{StateRejection, UpgradeState, evaluate_manifest},
};

/// Why the flow decided not to install this round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// The running platform is published but auto-upgrade is unsupported (R0
    /// Windows) — the installed binary is left untouched (§A.1).
    UnsupportedPlatform(Platform),
    /// The running platform is not part of the release matrix at all.
    PlatformNotInMatrix,
    /// The manifest is paused; no new download/install is allowed (§A.6).
    Paused,
    /// The manifest revokes its own version; it must never be installed
    /// (§A.6), even from cache/retry.
    RevokedTarget(ReleaseVersion),
    /// The manifest version is not newer than what is installed (§A.6:
    /// `manifest.version > installed_target_version`).
    NotNewer {
        manifest: ReleaseVersion,
        installed: ReleaseVersion,
    },
}

/// The outcome of the pure decision pipeline.
#[derive(Debug, Clone)]
pub enum UpgradeDecision {
    /// Install this artifact; `marker`/`new_state` are persisted on commit.
    Install(Box<InstallPlan>),
    /// Do nothing this round.
    Skip(SkipReason),
}

/// A fully-decided install: everything needed to download, verify and commit.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub version: ReleaseVersion,
    pub artifact: VerifiedArtifact,
    /// Marker to write on commit (installed_at is filled by the caller at
    /// install time from an RFC3339 timestamp — it is informational).
    pub marker: InstallMarker,
    /// Anti-rollback state to persist atomically with the commit.
    pub new_state: UpgradeState,
    /// Trusted effective clock at acceptance (for cache/throttle bookkeeping).
    pub effective_now: i64,
}

/// Errors that abort the decision before any Install/Skip verdict.
#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    #[error("manifest verification failed: {0}")]
    Manifest(#[from] ManifestError),
    #[error("manifest rejected by anti-rollback/time state: {0}")]
    State(#[from] StateRejection),
}

/// The host/clock/trust context a decision runs against (bundled so the
/// entry point stays under the argument-count lint).
pub struct DecisionContext<'a> {
    /// Persisted anti-rollback/time/throttle state.
    pub state: &'a UpgradeState,
    /// Parsed HTTPS `Date` of this round (unix seconds).
    pub https_date: Option<i64>,
    /// Local wall clock (unix seconds).
    pub local_now: i64,
    /// Compiled trust table.
    pub trust: &'a [super::trusted_keys::TrustedKey],
    /// Running platform, or `None` when not in the release matrix.
    pub platform: Option<Platform>,
    /// The running binary's version.
    pub installed_version: ReleaseVersion,
    /// Timestamp to stamp into the marker.
    pub installed_at_rfc3339: &'a str,
}

/// Pure decision: verify → anti-rollback → policy → artifact selection.
pub fn decide_from_envelope(
    ctx: &DecisionContext<'_>,
    envelope_bytes: &[u8],
) -> Result<UpgradeDecision, FlowError> {
    let manifest = super::manifest::verify_envelope_bytes(envelope_bytes, ctx.trust)?;
    let accepted = evaluate_manifest(ctx.state, &manifest, ctx.https_date, ctx.local_now)?;
    Ok(decide_after_verification(
        &manifest,
        accepted.new_state,
        accepted.effective_now,
        ctx.platform,
        ctx.installed_version,
        ctx.installed_at_rfc3339,
    ))
}

/// The policy + selection step, split out so it is testable against a
/// hand-built [`VerifiedManifest`] without re-deriving signatures.
pub fn decide_after_verification(
    manifest: &VerifiedManifest,
    new_state: UpgradeState,
    effective_now: i64,
    platform: Option<Platform>,
    installed_version: ReleaseVersion,
    installed_at_rfc3339: &str,
) -> UpgradeDecision {
    // Platform gate first: unsupported/absent platforms never download.
    let Some(platform) = platform else {
        return UpgradeDecision::Skip(SkipReason::PlatformNotInMatrix);
    };
    if platform.support() == PlatformSupport::Unsupported {
        return UpgradeDecision::Skip(SkipReason::UnsupportedPlatform(platform));
    }
    // Control gates: pause and self-revocation forbid any install (§A.6).
    if manifest.paused {
        return UpgradeDecision::Skip(SkipReason::Paused);
    }
    if manifest.is_revoked(manifest.version) {
        return UpgradeDecision::Skip(SkipReason::RevokedTarget(manifest.version));
    }
    // Only install something strictly newer than what is running.
    if manifest.version <= installed_version {
        return UpgradeDecision::Skip(SkipReason::NotNewer {
            manifest: manifest.version,
            installed: installed_version,
        });
    }
    // Artifact for our platform is guaranteed present after validation.
    let Some(artifact) = manifest.artifact_for(platform).cloned() else {
        // Defensive: validation guarantees coverage, so treat a miss as
        // "not in matrix" rather than panicking.
        return UpgradeDecision::Skip(SkipReason::PlatformNotInMatrix);
    };
    let marker = InstallMarker {
        schema_version: 1,
        installed_at: installed_at_rfc3339.to_string(),
        install_source: OFFICIAL_INSTALL_SOURCE.to_string(),
        platform: platform.as_str().to_string(),
        version: manifest.version.to_string(),
        sha256: artifact.sha256.clone(),
        size: artifact.size,
        manifest_key_id: manifest.signer_key_id.clone(),
    };
    UpgradeDecision::Install(Box::new(InstallPlan {
        version: manifest.version,
        artifact,
        marker,
        new_state,
        effective_now,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::upgrade::manifest::VerifiedManifest;

    fn manifest(version: &str, paused: bool, revoked: &[&str]) -> VerifiedManifest {
        let published_at = 1_000_000;
        VerifiedManifest {
            payload_digest: [3u8; 32],
            signer_key_id: "test-key-1".into(),
            version: ReleaseVersion::parse(version).unwrap(),
            version_raw: version.into(),
            control_revision: 5,
            published_at,
            expires_at: published_at + 90 * 24 * 3600,
            min_key_generation: 1,
            paused,
            revoked_versions: revoked
                .iter()
                .map(|v| ReleaseVersion::parse(v).unwrap())
                .collect(),
            artifacts: Platform::RELEASE_MATRIX
                .iter()
                .map(|p| VerifiedArtifact {
                    platform: *p,
                    url: format!(
                        "https://download.libra.tools/libra/releases/v{version}/libra-{p}"
                    ),
                    sha256: "a".repeat(64),
                    size: 4096,
                })
                .collect(),
        }
    }

    fn decide(
        m: &VerifiedManifest,
        platform: Option<Platform>,
        installed: &str,
    ) -> UpgradeDecision {
        decide_after_verification(
            m,
            UpgradeState::default(),
            m.published_at,
            platform,
            ReleaseVersion::parse(installed).unwrap(),
            "2026-07-17T00:00:00Z",
        )
    }

    #[test]
    fn newer_supported_platform_installs_with_marker() {
        let m = manifest("2.0.0", false, &[]);
        let decision = decide(&m, Some(Platform::DarwinArm64), "1.0.0");
        let UpgradeDecision::Install(plan) = decision else {
            panic!("expected install, got {decision:?}");
        };
        assert_eq!(plan.version, ReleaseVersion(2, 0, 0));
        assert_eq!(plan.artifact.platform, Platform::DarwinArm64);
        assert_eq!(plan.marker.install_source, OFFICIAL_INSTALL_SOURCE);
        assert_eq!(plan.marker.version, "2.0.0");
        assert_eq!(plan.marker.platform, "darwin-arm64");
        assert_eq!(plan.marker.manifest_key_id, "test-key-1");
    }

    #[test]
    fn windows_is_skipped_unsupported_even_when_newer() {
        let m = manifest("2.0.0", false, &[]);
        assert_eq!(
            decide(&m, Some(Platform::WindowsAmd64), "1.0.0"),
            UpgradeDecision::Skip(SkipReason::UnsupportedPlatform(Platform::WindowsAmd64))
        );
    }

    #[test]
    fn platform_not_in_matrix_skips() {
        let m = manifest("2.0.0", false, &[]);
        assert_eq!(
            decide(&m, None, "1.0.0"),
            UpgradeDecision::Skip(SkipReason::PlatformNotInMatrix)
        );
    }

    #[test]
    fn paused_manifest_never_installs() {
        let m = manifest("2.0.0", true, &[]);
        assert_eq!(
            decide(&m, Some(Platform::LinuxAmd64), "1.0.0"),
            UpgradeDecision::Skip(SkipReason::Paused)
        );
    }

    #[test]
    fn self_revoked_version_never_installs() {
        let m = manifest("2.0.0", false, &["2.0.0"]);
        assert_eq!(
            decide(&m, Some(Platform::LinuxAmd64), "1.0.0"),
            UpgradeDecision::Skip(SkipReason::RevokedTarget(ReleaseVersion(2, 0, 0)))
        );
    }

    #[test]
    fn same_or_older_version_is_not_newer() {
        let m = manifest("2.0.0", false, &[]);
        assert!(matches!(
            decide(&m, Some(Platform::LinuxAmd64), "2.0.0"),
            UpgradeDecision::Skip(SkipReason::NotNewer { .. })
        ));
        assert!(matches!(
            decide(&m, Some(Platform::LinuxAmd64), "2.1.0"),
            UpgradeDecision::Skip(SkipReason::NotNewer { .. })
        ));
    }

    impl PartialEq for UpgradeDecision {
        fn eq(&self, other: &Self) -> bool {
            match (self, other) {
                (UpgradeDecision::Skip(a), UpgradeDecision::Skip(b)) => a == b,
                _ => false,
            }
        }
    }
}
