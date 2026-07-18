//! Anti-rollback / control state and time policy (plan-20260714 §A.6 时间/
//! 缓存/节流, §A.7 first bullet).
//!
//! The state file records everything that must survive across upgrade
//! cycles so a replayed or rolled-back manifest can never install:
//!
//! - `max_seen`: highest accepted release version — lower versions are
//!   rejected, the SAME version must carry identical artifact identities;
//! - `max_control_revision` + the envelope digest accepted at that
//!   revision — a lower revision is always rejected, an equal revision is
//!   accepted only byte-identical (digest match), so a pre-revocation
//!   envelope cannot be replayed after a revocation was seen;
//! - `trusted_time_floor`: monotone time floor advanced ONLY in the same
//!   atomic write as a fully validated acceptance (§A.6: invalid envelopes
//!   or bogus future `Date` headers must never poison time state);
//! - success cooldown and failure backoff for online-check throttling.
//!
//! All decision logic here is PURE (explicit clock inputs); the durable
//! read/write is a small serialization layer using the same atomic-write +
//! `0600` discipline as the settings file. Locked, directory-fd-relative
//! access arrives with the install-lock slice (§A.5).

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::manifest::{ReleaseVersion, VerifiedManifest};

/// State file name inside the install directory (regular file, `0600`,
/// no-follow discipline enforced by the locked accessors of §A.5).
pub const STATE_FILE_NAME: &str = ".libra-upgrade-state.json";

/// Grace applied around the HTTPS `Date` and the local clock (§A.6: 300 s).
pub const TIME_SLACK_SECONDS: i64 = 300;

/// Success cooldown base (§A.6: 15 min) and its jitter bound (0..120 s).
pub const SUCCESS_COOLDOWN_SECONDS: i64 = 15 * 60;
pub const SUCCESS_COOLDOWN_JITTER_SECONDS: i64 = 120;

/// Ceiling of the valid cooldown window relative to the floor (§A.6: 17 min
/// worst-case propagation delay for pause/revocation).
pub const MAX_COOLDOWN_AHEAD_SECONDS: i64 = 17 * 60;

/// How far the local clock may run ahead of the floor before the cooldown is
/// distrusted (§A.6: 24 h).
pub const MAX_LOCAL_AHEAD_SECONDS: i64 = 24 * 60 * 60;

/// Failure backoff cap (§A.6: 1 h).
pub const MAX_BACKOFF_SECONDS: i64 = 60 * 60;

/// Identity of an accepted artifact for one platform (§A.7: same version ⇒
/// identical identity).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIdentity {
    /// Lowercase 64-hex sha256.
    pub sha256: String,
    pub size: u64,
}

/// Durable anti-rollback / control / throttle state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpgradeState {
    #[serde(default)]
    pub schema_version: u32,
    /// Highest accepted release version, as its canonical `X.Y.Z` string.
    #[serde(default)]
    pub max_seen: Option<String>,
    /// Artifact identity per platform id, recorded at `max_seen`.
    #[serde(default)]
    pub artifact_identity: std::collections::BTreeMap<String, ArtifactIdentity>,
    /// Highest accepted control revision.
    #[serde(default)]
    pub max_control_revision: u64,
    /// Hex sha256 of the payload accepted at `max_control_revision`.
    #[serde(default)]
    pub control_envelope_digest: Option<String>,
    /// Monotone trusted time floor (unix seconds).
    #[serde(default)]
    pub trusted_time_floor: i64,
    /// Cross-process success cooldown (unix seconds).
    #[serde(default)]
    pub next_success_check_not_before: Option<i64>,
    /// Failure backoff: do not retry before this instant (unix seconds).
    #[serde(default)]
    pub backoff_not_before: Option<i64>,
    /// Last applied backoff duration in seconds (doubles up to the cap).
    #[serde(default)]
    pub backoff_seconds: i64,
}

/// Why a manifest was rejected against the persisted state. None of these
/// may mutate the state file (§A.6/§A.7).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StateRejection {
    #[error("manifest control_revision {offered} is below the accepted {accepted}")]
    ControlRevisionRollback { offered: u64, accepted: u64 },
    #[error(
        "manifest control_revision {revision} was already accepted with a \
         different payload (digest mismatch)"
    )]
    ControlRevisionForked { revision: u64 },
    #[error("manifest version {offered} is below the accepted {accepted}")]
    VersionRollback {
        offered: ReleaseVersion,
        accepted: ReleaseVersion,
    },
    #[error(
        "artifact identity for platform '{platform}' changed for already-seen version {version}"
    )]
    ArtifactIdentityChanged {
        platform: String,
        version: ReleaseVersion,
    },
    #[error("manifest response carried no usable HTTPS Date header")]
    MissingHttpsDate,
    #[error(
        "HTTPS Date {https_date} is outside the manifest lifetime \
         [published_at-{TIME_SLACK_SECONDS}s, expires_at) = [{lower}, {upper})"
    )]
    HttpsDateOutsideLifetime {
        https_date: i64,
        lower: i64,
        upper: i64,
    },
    #[error("manifest is expired (effective_now {effective_now} >= expires_at {expires_at})")]
    Expired { effective_now: i64, expires_at: i64 },
}

/// Outcome of a successful evaluation: the state to persist atomically in
/// the SAME write as the acceptance (§A.6 时间: floor advances only here).
#[derive(Debug)]
pub struct AcceptedManifest {
    pub new_state: UpgradeState,
    /// `max(local_wall_clock, trusted_time_floor, https_date)` at acceptance.
    pub effective_now: i64,
}

/// Evaluate a verified manifest against the persisted state (pure; §A.6/§A.7
/// order — every check happens before any state write).
///
/// # Arguments
/// * `state` - current durable state (default for a fresh install dir).
/// * `manifest` - output of `verify_envelope_bytes` (crypto + semantics done).
/// * `https_date` - parsed HTTPS `Date` of the round that fetched it.
/// * `local_now` - local wall clock (unix seconds).
pub fn evaluate_manifest(
    state: &UpgradeState,
    manifest: &VerifiedManifest,
    https_date: Option<i64>,
    local_now: i64,
) -> Result<AcceptedManifest, StateRejection> {
    // Control-plane anti-rollback first (§A.7): `<` always rejected, `==`
    // only byte-identical, `>` accepts renewals/emergency control.
    let digest_hex = hex::encode(manifest.payload_digest);
    if manifest.control_revision < state.max_control_revision {
        return Err(StateRejection::ControlRevisionRollback {
            offered: manifest.control_revision,
            accepted: state.max_control_revision,
        });
    }
    if manifest.control_revision == state.max_control_revision
        && state.max_control_revision > 0
        && state.control_envelope_digest.as_deref() != Some(digest_hex.as_str())
    {
        return Err(StateRejection::ControlRevisionForked {
            revision: manifest.control_revision,
        });
    }

    // Version anti-rollback (§A.7): `<` rejected; `==` requires identical
    // artifact identity per platform.
    if let Some(max_seen) = state.max_seen.as_deref().and_then(ReleaseVersion::parse) {
        if manifest.version < max_seen {
            return Err(StateRejection::VersionRollback {
                offered: manifest.version,
                accepted: max_seen,
            });
        }
        if manifest.version == max_seen {
            for artifact in &manifest.artifacts {
                if let Some(recorded) = state.artifact_identity.get(artifact.platform.as_str())
                    && (recorded.sha256 != artifact.sha256 || recorded.size != artifact.size)
                {
                    return Err(StateRejection::ArtifactIdentityChanged {
                        platform: artifact.platform.to_string(),
                        version: manifest.version,
                    });
                }
            }
        }
    }

    // Time policy (§A.6 时间): the HTTPS Date is required and must fall
    // inside the manifest lifetime; expiry uses the trusted effective clock.
    let https_date = https_date.ok_or(StateRejection::MissingHttpsDate)?;
    let lower = manifest.published_at - TIME_SLACK_SECONDS;
    if https_date < lower || https_date >= manifest.expires_at {
        return Err(StateRejection::HttpsDateOutsideLifetime {
            https_date,
            lower,
            upper: manifest.expires_at,
        });
    }
    let effective_now = local_now.max(state.trusted_time_floor).max(https_date);
    if effective_now >= manifest.expires_at {
        return Err(StateRejection::Expired {
            effective_now,
            expires_at: manifest.expires_at,
        });
    }

    // Acceptance: floor, anti-rollback fields and the success cooldown all
    // advance in ONE durable write (performed by the caller under the lock).
    let mut new_state = state.clone();
    new_state.schema_version = 1;
    new_state.trusted_time_floor = state
        .trusted_time_floor
        .max(manifest.published_at)
        .max(https_date);
    new_state.max_control_revision = manifest.control_revision;
    new_state.control_envelope_digest = Some(digest_hex);
    let previous_max = state.max_seen.as_deref().and_then(ReleaseVersion::parse);
    if previous_max.is_none_or(|m| manifest.version >= m) {
        new_state.max_seen = Some(manifest.version.to_string());
        for artifact in &manifest.artifacts {
            new_state.artifact_identity.insert(
                artifact.platform.as_str().to_string(),
                ArtifactIdentity {
                    sha256: artifact.sha256.clone(),
                    size: artifact.size,
                },
            );
        }
    }
    new_state.next_success_check_not_before = Some(
        https_date + SUCCESS_COOLDOWN_SECONDS + deterministic_jitter(&manifest.payload_digest),
    );
    new_state.backoff_not_before = None;
    new_state.backoff_seconds = 0;
    Ok(AcceptedManifest {
        new_state,
        effective_now,
    })
}

/// Deterministic jitter in `[0, 120)` seconds derived from the payload
/// digest (§A.6: `deterministic_jitter(0..120s)` — no wall-clock or RNG
/// input, so retries and concurrent processes agree).
fn deterministic_jitter(payload_digest: &[u8; 32]) -> i64 {
    let mut acc = [0u8; 8];
    acc.copy_from_slice(&payload_digest[..8]);
    (u64::from_be_bytes(acc) % SUCCESS_COOLDOWN_JITTER_SECONDS as u64) as i64
}

/// Whether the cached success cooldown allows SKIPPING the online check
/// (§A.6 缓存/节流). The cooldown can only ever skip a CHECK — installs and
/// candidate reuse must revalidate online in-process regardless.
pub fn cooldown_permits_skip(state: &UpgradeState, local_now: i64) -> bool {
    let Some(not_before) = state.next_success_check_not_before else {
        return false;
    };
    let floor = state.trusted_time_floor;
    // Local clock outside the trusted window → distrust the cooldown.
    if local_now < floor - TIME_SLACK_SECONDS || local_now > floor + MAX_LOCAL_AHEAD_SECONDS {
        return false;
    }
    // Cooldown outside its legal window relative to the floor → distrust.
    if not_before < floor || not_before > floor + MAX_COOLDOWN_AHEAD_SECONDS {
        return false;
    }
    local_now < not_before
}

/// Whether cached/candidate artifacts may be INSTALLED without an online
/// refresh: never when the local clock sits more than the slack below the
/// floor (§A.6: 本地低于 floor 超 300s 时禁止 cache 安装并强制在线刷新).
pub fn local_clock_permits_cached_install(state: &UpgradeState, local_now: i64) -> bool {
    local_now >= state.trusted_time_floor - TIME_SLACK_SECONDS
}

/// Next failure backoff (§A.6: doubling, capped at 1 h, using the same
/// trusted upper bound for sanity). Returns the state to persist.
pub fn register_failure_backoff(state: &UpgradeState, local_now: i64) -> UpgradeState {
    let mut new_state = state.clone();
    new_state.schema_version = 1;
    let next = if state.backoff_seconds <= 0 {
        60
    } else {
        (state.backoff_seconds * 2).min(MAX_BACKOFF_SECONDS)
    };
    new_state.backoff_seconds = next;
    new_state.backoff_not_before = Some(local_now + next);
    new_state
}

/// Whether the failure backoff currently defers an online attempt.
pub fn backoff_defers(state: &UpgradeState, local_now: i64) -> bool {
    match state.backoff_not_before {
        // The same trusted bound as the cooldown: a backoff absurdly far in
        // the future (clock damage) is ignored rather than wedging checks.
        Some(not_before) => not_before - local_now <= MAX_BACKOFF_SECONDS && local_now < not_before,
        None => false,
    }
}

/// Failures of the durable state layer.
#[derive(Debug, thiserror::Error)]
pub enum StateStoreError {
    #[error("cannot read upgrade state at {path}: {source}")]
    Unreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Corrupt anti-rollback state is FATAL for the upgrade cycle: silently
    /// resetting it would erase the rollback/replay protections, so the
    /// caller must skip upgrading and surface a warning instead (§A.7).
    #[error(
        "upgrade state file {path} is corrupt: {detail}; refusing to upgrade \
         (delete the file only if you understand this discards anti-rollback history)"
    )]
    Corrupt { path: PathBuf, detail: String },
    #[error("cannot write upgrade state at {path}: {source}")]
    WriteFailed {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Read the state file inside `install_dir`. Missing file → default state.
/// Corrupt file → error (fail closed; see [`StateStoreError::Corrupt`]).
pub fn read_state(install_dir: &Path) -> Result<UpgradeState, StateStoreError> {
    let path = install_dir.join(STATE_FILE_NAME);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(UpgradeState::default()),
        Err(err) => return Err(StateStoreError::Unreadable { path, source: err }),
    };
    serde_json::from_slice(&bytes).map_err(|err| StateStoreError::Corrupt {
        path,
        detail: err.to_string(),
    })
}

/// Atomically persist `state` inside `install_dir` (`0600` on Unix). The
/// §A.5 lock and directory-fd discipline wrap this call.
pub fn write_state(install_dir: &Path, state: &UpgradeState) -> Result<(), StateStoreError> {
    let path = install_dir.join(STATE_FILE_NAME);
    let write_failed = |err: io::Error| StateStoreError::WriteFailed {
        path: path.clone(),
        source: err,
    };
    let mut bytes = serde_json::to_vec_pretty(state).map_err(|err| StateStoreError::Corrupt {
        path: path.clone(),
        detail: format!("cannot serialize state: {err}"),
    })?;
    bytes.push(b'\n');
    crate::utils::atomic_write::write_atomic(&path, &bytes, true).map_err(write_failed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(write_failed)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::upgrade::{manifest::VerifiedArtifact, platform::Platform};

    fn manifest(version: &str, control: u64, digest_byte: u8) -> VerifiedManifest {
        let published_at = 1_000_000;
        VerifiedManifest {
            payload_digest: [digest_byte; 32],
            signer_key_id: "test-key-1".into(),
            version: ReleaseVersion::parse(version).unwrap(),
            version_raw: version.to_string(),
            control_revision: control,
            published_at,
            expires_at: published_at + 90 * 24 * 3600,
            min_key_generation: 1,
            paused: false,
            revoked_versions: vec![],
            artifacts: Platform::RELEASE_MATRIX
                .iter()
                .map(|p| VerifiedArtifact {
                    platform: *p,
                    url: format!(
                        "https://download.libra.tools/libra/releases/v{version}/libra-{p}"
                    ),
                    sha256: "a".repeat(64),
                    size: 1024,
                })
                .collect(),
        }
    }

    const GOOD_DATE: i64 = 1_000_100;

    #[test]
    fn fresh_state_accepts_and_advances_everything_in_one_step() {
        let state = UpgradeState::default();
        let m = manifest("1.2.3", 5, 7);
        let accepted = evaluate_manifest(&state, &m, Some(GOOD_DATE), GOOD_DATE + 1).unwrap();
        let s = &accepted.new_state;
        assert_eq!(s.max_seen.as_deref(), Some("1.2.3"));
        assert_eq!(s.max_control_revision, 5);
        assert_eq!(
            s.control_envelope_digest.as_deref(),
            Some(hex::encode([7u8; 32]).as_str())
        );
        assert_eq!(s.trusted_time_floor, GOOD_DATE.max(m.published_at));
        assert_eq!(s.artifact_identity.len(), 4);
        let cd = s.next_success_check_not_before.unwrap();
        assert!(
            (GOOD_DATE + SUCCESS_COOLDOWN_SECONDS..GOOD_DATE + SUCCESS_COOLDOWN_SECONDS + 120)
                .contains(&cd)
        );
    }

    #[test]
    fn control_revision_rollback_and_fork_rejected() {
        let state = evaluate_manifest(
            &UpgradeState::default(),
            &manifest("1.2.3", 5, 7),
            Some(GOOD_DATE),
            GOOD_DATE,
        )
        .unwrap()
        .new_state;
        // Replaying an older, still-unexpired control revision (e.g. the
        // pre-revocation envelope) is rejected (§A.7 test mandate).
        assert_eq!(
            evaluate_manifest(&state, &manifest("1.2.3", 4, 9), Some(GOOD_DATE), GOOD_DATE)
                .unwrap_err(),
            StateRejection::ControlRevisionRollback {
                offered: 4,
                accepted: 5
            }
        );
        // Same revision, different payload → forked control plane.
        assert_eq!(
            evaluate_manifest(&state, &manifest("1.2.3", 5, 9), Some(GOOD_DATE), GOOD_DATE)
                .unwrap_err(),
            StateRejection::ControlRevisionForked { revision: 5 }
        );
        // Same revision, identical payload → idempotent acceptance.
        assert!(
            evaluate_manifest(&state, &manifest("1.2.3", 5, 7), Some(GOOD_DATE), GOOD_DATE).is_ok()
        );
    }

    #[test]
    fn version_rollback_and_identity_mutation_rejected() {
        let state = evaluate_manifest(
            &UpgradeState::default(),
            &manifest("1.2.3", 5, 7),
            Some(GOOD_DATE),
            GOOD_DATE,
        )
        .unwrap()
        .new_state;
        assert!(matches!(
            evaluate_manifest(&state, &manifest("1.2.2", 6, 8), Some(GOOD_DATE), GOOD_DATE),
            Err(StateRejection::VersionRollback { .. })
        ));
        // Same version with a different artifact identity is immutable
        // (§A.11 upgrade_same_version_artifact_identity_immutable).
        let mut same_version = manifest("1.2.3", 6, 8);
        same_version.artifacts[0].sha256 = "b".repeat(64);
        assert!(matches!(
            evaluate_manifest(&state, &same_version, Some(GOOD_DATE), GOOD_DATE),
            Err(StateRejection::ArtifactIdentityChanged { .. })
        ));
    }

    #[test]
    fn missing_or_out_of_lifetime_https_date_rejected_without_poisoning() {
        let state = UpgradeState::default();
        let m = manifest("1.2.3", 5, 7);
        assert_eq!(
            evaluate_manifest(&state, &m, None, GOOD_DATE).unwrap_err(),
            StateRejection::MissingHttpsDate
        );
        // A Date far past expiry — rejected, and (crucially) evaluate never
        // returns a new state, so nothing can be written (§A.11
        // upgrade_invalid_envelope_future_date_does_not_poison_time).
        assert!(matches!(
            evaluate_manifest(&state, &m, Some(m.expires_at + 1), GOOD_DATE),
            Err(StateRejection::HttpsDateOutsideLifetime { .. })
        ));
        // A Date before published_at-300s likewise.
        assert!(matches!(
            evaluate_manifest(&state, &m, Some(m.published_at - 301), GOOD_DATE),
            Err(StateRejection::HttpsDateOutsideLifetime { .. })
        ));
    }

    #[test]
    fn expiry_uses_effective_now_so_clock_rollback_cannot_extend_life() {
        // Floor already far past expiry: even a rolled-back local clock and
        // a (stale-but-in-lifetime) Date cannot resurrect the manifest
        // (§A.11 upgrade_clock_rollback_after_restart_blocks_install).
        let m = manifest("1.2.3", 5, 7);
        let state = UpgradeState {
            trusted_time_floor: m.expires_at + 10,
            ..Default::default()
        };
        assert!(matches!(
            evaluate_manifest(&state, &m, Some(GOOD_DATE), m.published_at),
            Err(StateRejection::Expired { .. })
        ));
    }

    #[test]
    fn future_local_clock_rejects_round_but_recovers_after_correction() {
        // A local clock absurdly in the future only rejects the CURRENT
        // round via expiry; it never writes the floor, so a corrected clock
        // succeeds later (§A.11 upgrade_future_local_clock_cache_then_
        // corrected_refreshes).
        let state = UpgradeState::default();
        let m = manifest("1.2.3", 5, 7);
        assert!(matches!(
            evaluate_manifest(&state, &m, Some(GOOD_DATE), m.expires_at + 500),
            Err(StateRejection::Expired { .. })
        ));
        assert_eq!(state.trusted_time_floor, 0, "rejection wrote nothing");
        assert!(evaluate_manifest(&state, &m, Some(GOOD_DATE), GOOD_DATE).is_ok());
    }

    #[test]
    fn cooldown_window_rules() {
        let m = manifest("1.2.3", 5, 7);
        let state = evaluate_manifest(&UpgradeState::default(), &m, Some(GOOD_DATE), GOOD_DATE)
            .unwrap()
            .new_state;
        let floor = state.trusted_time_floor;
        let not_before = state.next_success_check_not_before.unwrap();
        // Within the cooldown and a sane clock → skip.
        assert!(cooldown_permits_skip(&state, not_before - 10));
        // Past the cooldown → check online.
        assert!(!cooldown_permits_skip(&state, not_before + 1));
        // Local clock below floor-300 → distrust cooldown.
        assert!(!cooldown_permits_skip(&state, floor - 301));
        // Local clock more than 24h ahead of floor → distrust cooldown.
        assert!(!cooldown_permits_skip(
            &state,
            floor + MAX_LOCAL_AHEAD_SECONDS + 1
        ));
        // Cooldown outside [floor, floor+17min] → distrust.
        let mut weird = state.clone();
        weird.next_success_check_not_before = Some(floor + MAX_COOLDOWN_AHEAD_SECONDS + 1);
        assert!(!cooldown_permits_skip(&weird, floor + 10));
        let mut below = state;
        below.next_success_check_not_before = Some(floor - 1);
        assert!(!cooldown_permits_skip(&below, floor - 10));
    }

    #[test]
    fn cached_install_forbidden_when_clock_below_floor() {
        let state = UpgradeState {
            trusted_time_floor: 10_000,
            ..Default::default()
        };
        assert!(!local_clock_permits_cached_install(&state, 9_000));
        assert!(local_clock_permits_cached_install(&state, 9_800));
        assert!(local_clock_permits_cached_install(&state, 20_000));
    }

    #[test]
    fn failure_backoff_doubles_and_caps() {
        let mut state = UpgradeState::default();
        let mut expected = 60;
        for _ in 0..8 {
            state = register_failure_backoff(&state, 1_000);
            assert_eq!(state.backoff_seconds, expected);
            assert!(backoff_defers(&state, 1_000));
            assert!(!backoff_defers(&state, 1_000 + expected));
            expected = (expected * 2).min(MAX_BACKOFF_SECONDS);
        }
        assert_eq!(state.backoff_seconds, MAX_BACKOFF_SECONDS);
        // An absurd backoff instant (clock damage) is ignored.
        let mut damaged = state.clone();
        damaged.backoff_not_before = Some(1_000 + MAX_BACKOFF_SECONDS + 10_000);
        assert!(!backoff_defers(&damaged, 1_000));
    }

    #[test]
    fn state_store_roundtrip_missing_and_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        // Missing → default.
        let state = read_state(dir.path()).unwrap();
        assert_eq!(state.max_control_revision, 0);
        // Roundtrip.
        let m = manifest("1.2.3", 5, 7);
        let accepted = evaluate_manifest(&state, &m, Some(GOOD_DATE), GOOD_DATE).unwrap();
        write_state(dir.path(), &accepted.new_state).unwrap();
        let reread = read_state(dir.path()).unwrap();
        assert_eq!(reread.max_control_revision, 5);
        assert_eq!(reread.max_seen.as_deref(), Some("1.2.3"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join(STATE_FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Corrupt → hard error, never a silent reset.
        fs::write(dir.path().join(STATE_FILE_NAME), b"{ nope").unwrap();
        assert!(matches!(
            read_state(dir.path()),
            Err(StateStoreError::Corrupt { .. })
        ));
    }

    #[test]
    fn jitter_is_deterministic_and_bounded() {
        let a = deterministic_jitter(&[7u8; 32]);
        assert_eq!(a, deterministic_jitter(&[7u8; 32]));
        assert!((0..SUCCESS_COOLDOWN_JITTER_SECONDS).contains(&a));
        let b = deterministic_jitter(&[8u8; 32]);
        assert!((0..SUCCESS_COOLDOWN_JITTER_SECONDS).contains(&b));
    }
}
