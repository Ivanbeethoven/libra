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
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {path:?}: {err}; run tests/cloud_live_prepare.sh first"));
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
pub fn slot_is_live(slot: &WriterSlot) -> bool {
    parse_rfc3339(&slot.expires_at)
        .map(|exp| SystemTime::now() < exp)
        .unwrap_or(true)
}

fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    // YYYY-MM-DDTHH:MM:SSZ (GC-CM-15: UTC, no fractional seconds).
    let s = s.strip_suffix('Z')?;
    let dt = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()?;
    Some(
        SystemTime::UNIX_EPOCH
            + Duration::from_secs(dt.and_utc().timestamp().max(0) as u64),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
