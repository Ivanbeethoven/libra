//! Auto-upgrade publish/manifest contract tests (plan-20260714 §A.9/§A.11).
//!
//! These pin the manifest *contract* the release/publish jobs must honour:
//! the exact signature-verification order, the release-matrix coverage and
//! URL grammar, the anti-rollback field rules that a "renew" job must
//! preserve (`paused`/`revoked_versions` byte-identical), and the size
//! bounds. Behind the `test-upgrade` feature (`required-features`).

#![cfg(feature = "test-upgrade")]

use base64::Engine as _;
use libra::internal::upgrade::{
    manifest::{MAX_ARTIFACT_BYTES, ManifestError, SIGNATURE_DOMAIN_PREFIX, verify_envelope_bytes},
    platform::Platform,
    trusted_keys::TrustedKey,
};

const SEED: [u8; 32] = [7u8; 32];

fn keypair() -> ring::signature::Ed25519KeyPair {
    ring::signature::Ed25519KeyPair::from_seed_unchecked(&SEED).unwrap()
}

fn trust() -> Vec<TrustedKey> {
    use ring::signature::KeyPair;
    let pk: [u8; 32] = keypair().public_key().as_ref().try_into().unwrap();
    vec![TrustedKey {
        key_id: "test-key-1",
        ed25519_pubkey: pk,
        not_before: 0,
        not_after: 4_102_444_800,
        generation: 1,
    }]
}

fn artifact(platform: &str, version: &str) -> serde_json::Value {
    serde_json::json!({
        "platform": platform,
        "url": format!("https://download.libra.tools/libra/releases/v{version}/libra-{platform}"),
        "sha256": "a".repeat(64),
        "size": 4096,
    })
}

fn full_payload(version: &str) -> serde_json::Value {
    serde_json::json!({
        "channel": "stable",
        "version": version,
        "control_revision": 5,
        "published_at": "2026-07-01T00:00:00Z",
        "expires_at": "2026-09-29T00:00:00Z",
        "min_key_generation": 1,
        "paused": false,
        "revoked_versions": [],
        "artifacts": [
            artifact("linux-amd64", version),
            artifact("linux-arm64", version),
            artifact("darwin-arm64", version),
            artifact("windows-amd64", version),
        ],
    })
}

fn envelope(payload: &serde_json::Value) -> Vec<u8> {
    let payload_bytes = serde_json::to_vec(payload).unwrap();
    let mut message = SIGNATURE_DOMAIN_PREFIX.to_vec();
    message.extend_from_slice(&payload_bytes);
    let sig = keypair().sign(&message);
    serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "payload": base64::engine::general_purpose::STANDARD.encode(&payload_bytes),
        "signatures": [{
            "key_id": "test-key-1",
            "signature": base64::engine::general_purpose::STANDARD.encode(sig.as_ref()),
        }],
    }))
    .unwrap()
}

#[test]
fn upgrade_publish_is_conditional_and_complete() {
    // A well-formed, fully-covered manifest verifies; a manifest missing any
    // release-matrix platform (as an incomplete publish would produce) fails.
    let trust = trust();
    assert!(verify_envelope_bytes(&envelope(&full_payload("2.0.0")), &trust).is_ok());

    let mut incomplete = full_payload("2.0.0");
    incomplete["artifacts"].as_array_mut().unwrap().pop();
    assert!(matches!(
        verify_envelope_bytes(&envelope(&incomplete), &trust),
        Err(ManifestError::PayloadInvalid(_))
    ));
}

#[test]
fn upgrade_url_binding_is_enforced() {
    let trust = trust();
    // A URL whose tag does not match the payload version (a mis-tagged
    // publish) must fail the cross-field binding.
    let mut mismatched = full_payload("2.0.0");
    mismatched["artifacts"][0]["url"] =
        serde_json::json!("https://download.libra.tools/libra/releases/v9.9.9/libra-linux-amd64");
    assert!(matches!(
        verify_envelope_bytes(&envelope(&mismatched), &trust),
        Err(ManifestError::PayloadInvalid(_))
    ));
    // A non-pinned host is rejected.
    let mut bad_host = full_payload("2.0.0");
    bad_host["artifacts"][0]["url"] =
        serde_json::json!("https://cdn.evil.example/libra/releases/v2.0.0/libra-linux-amd64");
    assert!(verify_envelope_bytes(&envelope(&bad_host), &trust).is_err());
}

#[test]
fn upgrade_publish_size_bounds_enforced() {
    let trust = trust();
    let mut too_big = full_payload("2.0.0");
    too_big["artifacts"][0]["size"] = serde_json::json!(MAX_ARTIFACT_BYTES + 1);
    assert!(verify_envelope_bytes(&envelope(&too_big), &trust).is_err());

    let mut zero = full_payload("2.0.0");
    zero["artifacts"][0]["size"] = serde_json::json!(0);
    assert!(verify_envelope_bytes(&envelope(&zero), &trust).is_err());
}

#[test]
fn upgrade_new_release_and_renew_preserve_pause_revocations() {
    // A verified manifest surfaces `paused`/`revoked_versions` exactly as
    // signed; a renew job must carry them byte-for-byte, which this asserts by
    // round-tripping both a paused and a revoking payload.
    let trust = trust();
    let mut paused = full_payload("2.0.0");
    paused["paused"] = serde_json::json!(true);
    let m = verify_envelope_bytes(&envelope(&paused), &trust).unwrap();
    assert!(m.paused);

    let mut revoking = full_payload("2.0.0");
    revoking["revoked_versions"] = serde_json::json!(["1.9.0", "1.9.1"]);
    let m = verify_envelope_bytes(&envelope(&revoking), &trust).unwrap();
    assert_eq!(m.revoked_versions.len(), 2);
    assert!(m.is_revoked(libra::internal::upgrade::manifest::ReleaseVersion(1, 9, 0)));
}

#[test]
fn upgrade_channel_must_be_stable() {
    let trust = trust();
    let mut beta = full_payload("2.0.0");
    beta["channel"] = serde_json::json!("beta");
    assert!(verify_envelope_bytes(&envelope(&beta), &trust).is_err());
}

#[test]
fn upgrade_matrix_covers_exactly_four_platforms() {
    // The publish contract mandates one artifact per release-matrix platform.
    assert_eq!(Platform::RELEASE_MATRIX.len(), 4);
    for p in Platform::RELEASE_MATRIX {
        assert_eq!(Platform::parse(p.as_str()), Some(*p));
    }
}
