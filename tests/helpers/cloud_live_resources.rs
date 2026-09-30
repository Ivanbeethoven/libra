//! Cloud live test resources (plan-20260927 GC-CM-12).
//!
//! Provides the writer-identity probes, pre-allocated slot reads, and the
//! global / per-example manifest helpers the live-cloud tests use to prove
//! they only touch pre-registered D1/R2 resources. Consumes the slots that
//! `tests/cloud_live_prepare.sh` provisions under `LIBRA_CLOUD_LIVE_SLOTS_DIR`.
//!
//! These are intentionally small and read-only: they never write to a real
//! D1/R2 endpoint themselves, only expose the expected identities and slot
//! constraints so a `--features test-live-cloud` test can assert that its
//! writes stay in scope and FAIL CLOSED when they would touch an unregistered
//! repo ID / R2 prefix.
#![allow(dead_code)]

use std::{
    env,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};

/// One pre-allocated writer slot minted by `tests/cloud_live_prepare.sh`.
#[derive(Debug, Clone, Deserialize)]
pub struct WriterSlot {
    pub slot_id: String,
    pub repo_id: String,
    pub repo_name: String,
    pub r2_prefix: String,
    pub owner: String,
    pub expires_at: String,
}

/// The resolved live resource identity tuple the user confirmed
/// (DEP-CM-03 / GC-CM-18). The test layers assert against these exact values
/// rather than reading them from the ambient environment and trusting them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceIdentity {
    pub d1_account_id: String,
    pub d1_database_id: String,
    pub r2_account_id: String,
    pub r2_bucket: String,
}

impl Default for ResourceIdentity {
    fn default() -> Self {
        // GC-CM-18: only the `libra-testing` database in this account is for
        // this plan's tests. Any other D1 database belongs to another project.
        Self {
            d1_account_id: env("LIBRA_D1_ACCOUNT_ID").unwrap_or_default(),
            d1_database_id: env("LIBRA_D1_DATABASE_ID").unwrap_or_default(),
            r2_account_id: env("LIBRA_STORAGE_ENDPOINT").unwrap_or_default(),
            r2_bucket: env("LIBRA_STORAGE_BUCKET").unwrap_or_default(),
        }
    }
}

fn env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}

/// Directory holding the slots provisioned by `cloud_live_prepare.sh`.
pub fn slots_dir() -> PathBuf {
    env::var("LIBRA_CLOUD_LIVE_SLOTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            tempfile::tempdir()
                .expect("need a temp dir for live-cloud slots")
                .keep()
        })
}

/// Load the pre-allocated writer slots, or fail closed if absent.
pub fn load_writer_slots() -> Vec<WriterSlot> {
    let path = slots_dir().join("slots.json");
    let data = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("failed to read {path:?}: {err}; run tests/cloud_live_prepare.sh first")
    });
    serde_json::from_str(&data)
        .unwrap_or_else(|err| panic!("invalid slots.json at {path:?}: {err}"))
}

/// Identity probe: assert the configured D1/R2 identity is non-empty and, when
/// a `Expected` is supplied, matches exactly. Returns the resolved identity.
pub fn resource_identity_probe(expected: Option<&ResourceIdentity>) -> ResourceIdentity {
    let actual = ResourceIdentity::default();
    assert!(
        !actual.d1_account_id.is_empty(),
        "LIBRA_D1_ACCOUNT_ID must be set for live-cloud tests"
    );
    assert!(
        !actual.d1_database_id.is_empty(),
        "LIBRA_D1_DATABASE_ID must be set for live-cloud tests"
    );
    assert!(
        !actual.r2_bucket.is_empty(),
        "LIBRA_STORAGE_BUCKET must be set for live-cloud tests"
    );
    if let Some(exp) = expected {
        assert_eq!(
            actual, *exp,
            "live-cloud identity must match the user-confirmed resource tuple \
             (DEP-CM-03 / GC-CM-18); do not import an unregistered database/bucket"
        );
    }
    actual
}

/// True when the repo ID is one of the pre-allocated writer slots.
pub fn repo_id_is_registered(repo_id: &str, slots: &[WriterSlot]) -> bool {
    slots.iter().any(|s| s.repo_id == repo_id)
}

/// Assert a repo ID is registered; fail closed otherwise (GC-CM-12).
pub fn assert_registered_repo(repo_id: &str, slots: &[WriterSlot]) {
    assert!(
        repo_id_is_registered(repo_id, slots),
        "repo_id `{repo_id}` is not a pre-registered writer slot; refusing an unauthenticated \
         cloud write (verify tests/cloud_live_prepare.sh was run)"
    );
}

/// True when a R2 key's prefix lies within an allocated slot's exclusive prefix.
pub fn r2_key_in_registered_scope(key: &str, slots: &[WriterSlot]) -> bool {
    slots.iter().any(|s| key.starts_with(&s.r2_prefix))
}

/// Per-example pre-allocation record (GC-CM-12). The test process only
/// consumes pre-registered UUID/repo-ID/R2 prefixes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerExampleAllocation {
    pub repo_id: String,
    pub r2_prefix: String,
    pub owner: String,
    pub expires_at: String,
}

/// Build a global write-manifest envelope (GC-CM-15-style, consumer-facing).
pub fn build_global_manifest(slots: &[WriterSlot]) -> serde_json::Value {
    serde_json::json!({
        "schema": "libra-cloud-live-manifest-v1",
        "run": {
            "uuid": env("LIBRA_CLOUD_LIVE_RUN_UUID").unwrap_or_else(|| format!("{:x}", SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs())),
            "owner": env("LIBRA_CLOUD_LIVE_OWNER").unwrap_or_else(|| "genedna".to_string()),
            "expires_at": slots.first().map(|s| s.expires_at.clone()).unwrap_or_default(),
        },
        "writer_slots": slots.iter().map(|s| PerExampleAllocation {
            repo_id: s.repo_id.clone(),
            r2_prefix: s.r2_prefix.clone(),
            owner: s.owner.clone(),
            expires_at: s.expires_at.clone(),
        }).collect::<Vec<_>>(),
    })
}

/// Deadline helper: reject a slot whose `expires_at` has already passed.
///
/// An `expires_at` that cannot be parsed is reported as NOT live. This gates
/// writes against a real cloud resource, so an unknown deadline must fail closed
/// rather than silently authorise an unbounded write window.
pub fn slot_is_live(slot: &WriterSlot) -> bool {
    parse_rfc3339(&slot.expires_at).is_some_and(|exp| SystemTime::now() < exp)
}

fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    // YYYY-MM-DDTHH:MM:SSZ (GC-CM-15: UTC, no fractional seconds).
    let s = s.strip_suffix('Z')?;
    let dt = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(dt.and_utc().timestamp().max(0) as u64))
}

/// Authorized cleanup scope derived from a validated manifest (GC-CM-15).
/// RECOVERY-CLEANUP may only delete the union of the `writer_slots` and the
/// `restore_target_slots` source repo IDs, and only R2 keys whose prefixes are
/// within a slot's exclusive prefix. Anything else must be left untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupScope {
    pub d1_repo_ids: std::collections::BTreeSet<String>,
    pub r2_prefixes: Vec<String>,
}

/// Build the bounded cleanup scope from `manifest.writer_slots` and
/// `manifest.restore_target_slots`, failing closed when a restore target
/// references a source repo ID that is not itself registered as a writer slot.
pub fn authorized_cleanup_scope(
    manifest: &serde_json::Value,
    slots: &[WriterSlot],
) -> Result<CleanupScope, String> {
    let writer_slots = manifest
        .get("writer_slots")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "manifest.writer_slots must be an array".to_string())?;
    let targets = manifest
        .get("restore_target_slots")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "manifest.restore_target_slots must be an array".to_string())?;

    let mut d1_repo_ids = std::collections::BTreeSet::new();
    let mut r2_prefixes: Vec<String> = Vec::new();
    let registered: std::collections::BTreeSet<&str> =
        slots.iter().map(|s| s.repo_id.as_str()).collect();

    for w in writer_slots {
        let repo_id = w
            .get("repo_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "writer_slots entry missing repo_id".to_string())?;
        let prefix = w
            .get("r2_prefix")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "writer_slots entry missing r2_prefix".to_string())?;
        let expected = format!("{repo_id}/");
        if prefix != expected {
            return Err(format!(
                "writer slot r2_prefix must be exactly `{expected}`"
            ));
        }
        // The manifest is caller-supplied JSON and this scope carries delete
        // authority, so a writer slot may only name a pre-registered slot -- the
        // same constraint the restore targets below already enforce. Otherwise a
        // manifest could add an arbitrary D1 repo ID to the deletion scope.
        if !registered.contains(repo_id) {
            return Err(format!(
                "writer slot references unregistered repo_id `{repo_id}`; refusing cleanup (GC-CM-15)"
            ));
        }
        d1_repo_ids.insert(repo_id.to_string());
        r2_prefixes.push(prefix.to_string());
    }
    // Restore targets reference source repo IDs; each must be registered too.
    for t in targets {
        let source_repo_id = t
            .get("source_repo_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "restore_target_slots entry missing source_repo_id".to_string())?;
        if !registered.contains(source_repo_id) {
            return Err(format!(
                "restore target references unregistered source repo_id `{source_repo_id}`; refusing cleanup (GC-CM-15)"
            ));
        }
        d1_repo_ids.insert(source_repo_id.to_string());
    }
    Ok(CleanupScope {
        d1_repo_ids,
        r2_prefixes,
    })
}

/// REPO-SCOPE fail-closed guard at a cloud mutation sink: reject any D1/R2
/// write keyed by a repo ID / R2 prefix outside the registered writer slots.
pub fn reject_unregistered_sink_write(
    repo_id: &str,
    r2_prefix: &str,
    slots: &[WriterSlot],
) -> Result<(), String> {
    // Fail closed for every repo ID: a sink write is authorised only when the
    // ID is one of the pre-allocated writer slots. Do NOT gate this behind a
    // name prefix such as `test-repo-`; an unregistered ID that happens to fall
    // outside that family (for example a real user repository) must still be
    // rejected, otherwise the guard fails open for exactly the writes it exists
    // to stop.
    if !slots.iter().any(|s| s.repo_id == repo_id) {
        return Err(format!(
            "sink write for unregistered repo_id `{repo_id}`; run tests/cloud_live_prepare.sh \
             to pre-allocate writer slots"
        ));
    }
    if !slots.iter().any(|s| r2_prefix.starts_with(&s.r2_prefix)) {
        return Err(format!(
            "sink write R2 prefix `{r2_prefix}` is outside every registered writer slot"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_scope_is_bounded_and_fails_on_unregistered_target() {
        use crate::helpers::cloud_live_resources::*;
        let slots = vec![
            WriterSlot {
                slot_id: "s1".into(),
                repo_id: "test-repo-a".into(),
                repo_name: "a".into(),
                r2_prefix: "test-repo-a/".into(),
                owner: "o".into(),
                expires_at: "2099-01-01T00:00:00Z".into(),
            },
            WriterSlot {
                slot_id: "s2".into(),
                repo_id: "test-repo-b".into(),
                repo_name: "b".into(),
                r2_prefix: "test-repo-b/".into(),
                owner: "o".into(),
                expires_at: "2099-01-01T00:00:00Z".into(),
            },
        ];
        let manifest = serde_json::json!({
            "writer_slots": [
                {"repo_id":"test-repo-a","r2_prefix":"test-repo-a/"},
                {"repo_id":"test-repo-b","r2_prefix":"test-repo-b/"}
            ],
            "restore_target_slots": [
                {"source_repo_id":"test-repo-a"}
            ]
        });
        let scope = authorized_cleanup_scope(&manifest, &slots).expect("valid scope");
        assert!(scope.d1_repo_ids.contains("test-repo-a"));
        assert!(scope.d1_repo_ids.contains("test-repo-b"));
        assert_eq!(
            scope.r2_prefixes,
            vec!["test-repo-a/".to_string(), "test-repo-b/".to_string()]
        );

        // Unregistered restore target must fail closed.
        let bad = serde_json::json!({
            "writer_slots":[{"repo_id":"test-repo-a","r2_prefix":"test-repo-a/"}],
            "restore_target_slots":[{"source_repo_id":"test-repo-X"}]
        });
        assert!(authorized_cleanup_scope(&bad, &slots).is_err());

        // An unregistered writer slot must not be able to widen the D1 delete
        // scope through the (caller-supplied) manifest.
        let widened = serde_json::json!({
            "writer_slots":[{"repo_id":"prod-users","r2_prefix":"prod-users/"}],
            "restore_target_slots":[]
        });
        assert!(authorized_cleanup_scope(&widened, &slots).is_err());

        // Sink guard rejects unregistered repo / out-of-scope prefix.
        assert!(reject_unregistered_sink_write("test-repo-a", "test-repo-a/", &slots).is_ok());
        assert!(reject_unregistered_sink_write("test-repo-X", "test-repo-a/", &slots).is_err());
        assert!(reject_unregistered_sink_write("test-repo-a", "outside/", &slots).is_err());

        // Regression: an unregistered ID outside the `test-repo-`/`libra` name
        // families (for example a real user repo) must not bypass the check.
        for bypass in [
            "prod-users",
            "acme-monorepo",
            "user-repo-123",
            "libra-tools",
        ] {
            assert!(
                reject_unregistered_sink_write(bypass, "test-repo-a/", &slots).is_err(),
                "unregistered repo_id `{bypass}` must be rejected by the sink guard"
            );
        }
    }

    #[test]
    fn writer_slot_round_trip_deserializes() {
        #[derive(Deserialize)]
        struct W {
            v: Vec<WriterSlot>,
        }
        let w: W = serde_json::from_str(
            r#"{"v":[{"slot_id":"s","repo_id":"test-repo-abc","repo_name":"n","r2_prefix":"test-repo-abc/","owner":"o","expires_at":"2026-01-01T00:00:00Z"}]}"#,
        )
        .expect("valid slot json");
        assert_eq!(w.v[0].repo_id, "test-repo-abc");
    }

    #[test]
    fn rfc3339_parsing_rejects_and_accepts() {
        assert!(parse_rfc3339("2099-01-01T00:00:00Z").is_some());
        assert!(parse_rfc3339("not-a-date").is_none());
    }

    #[test]
    fn slot_deadline_fails_closed_on_unparseable_expiry() {
        let base = WriterSlot {
            slot_id: "s".into(),
            repo_id: "test-repo-a".into(),
            repo_name: "a".into(),
            r2_prefix: "test-repo-a/".into(),
            owner: "o".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
        };
        assert!(slot_is_live(&base), "a future expiry is live");
        let expired = WriterSlot {
            expires_at: "2000-01-01T00:00:00Z".into(),
            ..base.clone()
        };
        assert!(!slot_is_live(&expired), "a past expiry is not live");
        // An unparseable deadline must not authorise the slot.
        let malformed = WriterSlot {
            expires_at: "not-a-date".into(),
            ..base.clone()
        };
        assert!(
            !slot_is_live(&malformed),
            "an unknown expiry must fail closed, not authorise an unbounded write window"
        );
    }
}
