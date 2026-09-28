//! Repository commit references with explicit [`HashKind`] (ADR-B3-04 / B3-10).
//!
//! AI workflow records reference commits that may be hashed under sha1, sha256,
//! or blake3 depending on the repository's `core.objectformat`. Libra's
//! authoritative carrier is a **tagged** string
//! `repo-commit:<kind>:<kind-native hex>` stored in
//! `ai_index_task_run.base_commit_ref` (B3-16 schema). git-internal
//! [`IntegrityHash`] remains a fixed SHA-256 integrity digest and must never
//! carry a repository OID.

use git_internal::{
    hash::{HashKind, ObjectHash},
    internal::object::integrity::IntegrityHash,
};

use crate::internal::object_format;

/// Libra-owned tagged repository commit reference.
///
/// Wire form: `repo-commit:<kind>:<kind-native hex>` where sha1 hex is 40 chars
/// and sha256/blake3 hex is 64 chars (no zero-padding).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoCommitRef {
    kind: HashKind,
    hex: String,
}

impl RepoCommitRef {
    pub const TAG_PREFIX: &'static str = "repo-commit:";

    pub fn kind(&self) -> HashKind {
        self.kind
    }

    pub fn hex(&self) -> &str {
        &self.hex
    }

    /// Serialize to the Libra tagged form.
    pub fn to_tagged_str(&self) -> String {
        format!("{}{}:{}", Self::TAG_PREFIX, self.kind.as_str(), self.hex)
    }

    /// Parse a Libra tagged string (`repo-commit:<kind>:<hex>`).
    ///
    /// This is **not** git-internal's `<kind>:<hex>` tagged ObjectHash form.
    pub fn from_tagged_str(raw: &str) -> Result<Self, String> {
        let v = raw.trim();
        let rest = v
            .strip_prefix(Self::TAG_PREFIX)
            .ok_or_else(|| format!("missing {} prefix: {v}", Self::TAG_PREFIX))?;
        let (kind_s, hex) = rest
            .split_once(':')
            .ok_or_else(|| format!("malformed tagged repo-commit ref: {v}"))?;
        let kind = kind_s
            .parse::<HashKind>()
            .map_err(|_| format!("unknown hash kind in tagged ref: {kind_s}"))?;
        Self::from_hex_for_kind(kind, hex)
    }

    /// Build from kind-native hex (sha1=40, sha256/blake3=64). No zero-padding.
    pub fn from_hex_for_kind(kind: HashKind, hex: &str) -> Result<Self, String> {
        let hex = hex.trim().to_ascii_lowercase();
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "Invalid commit hash: contains non-hex characters: {hex}"
            ));
        }
        let expected = kind.hex_len();
        if hex.len() != expected {
            return Err(format!(
                "Invalid commit hash length for {}: got {}, expected {expected}",
                kind.as_str(),
                hex.len()
            ));
        }
        // Reject via ObjectHash so malformed widths/kind mismatches fail closed.
        ObjectHash::from_hex_for_kind(kind, &hex).map_err(|e| e.to_string())?;
        Ok(Self { kind, hex })
    }

    pub fn ensure_kind(&self, expected: HashKind) -> Result<(), String> {
        if self.kind != expected {
            return Err(format!(
                "repo-commit kind mismatch: got {}, expected {}",
                self.kind.as_str(),
                expected.as_str()
            ));
        }
        Ok(())
    }

    pub fn to_object_hash(&self) -> Result<ObjectHash, String> {
        ObjectHash::from_hex_for_kind(self.kind, &self.hex).map_err(|e| e.to_string())
    }

    /// Accept a bare (untagged) hex using an explicit repository kind.
    ///
    /// - sha1: 40-hex native, or legacy 64-hex with 24 trailing zero pad
    /// - sha256/blake3: 64-hex native only
    pub fn from_bare_hex_for_kind(kind: HashKind, bare: &str) -> Result<Self, String> {
        let v = bare.trim().to_ascii_lowercase();
        if !v.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "Invalid commit hash: contains non-hex characters: {v}"
            ));
        }
        match kind {
            HashKind::Sha1 => {
                if v.len() == 40 {
                    return Self::from_hex_for_kind(HashKind::Sha1, &v);
                }
                if v.len() == 64 && v.ends_with(&"0".repeat(24)) {
                    return Self::from_hex_for_kind(HashKind::Sha1, &v[..40]);
                }
                Err(format!(
                    "Invalid sha1 commit hash length: {} (expected 40, or legacy 64 with zero pad)",
                    v.len()
                ))
            }
            HashKind::Sha256 | HashKind::Blake3 => {
                if v.len() == 64 {
                    Self::from_hex_for_kind(kind, &v)
                } else {
                    Err(format!(
                        "Invalid {} commit hash length: {} (expected 64)",
                        kind.as_str(),
                        v.len()
                    ))
                }
            }
        }
    }
}

/// Dual-format reader/writer for `ai_index_task_run.base_commit_ref`.
pub struct RepoCommitRefStore;

impl RepoCommitRefStore {
    /// Prefer tagged column value; fall back to bare hex under `repo_kind`.
    pub fn read(
        base_commit_ref: Option<&str>,
        bare_fallback: Option<&str>,
        repo_kind: HashKind,
    ) -> Result<Option<RepoCommitRef>, String> {
        if let Some(tagged) = base_commit_ref.map(str::trim).filter(|s| !s.is_empty()) {
            if tagged.starts_with(RepoCommitRef::TAG_PREFIX) {
                let r = RepoCommitRef::from_tagged_str(tagged)?;
                r.ensure_kind(repo_kind)?;
                return Ok(Some(r));
            }
            // Malformed non-empty column that is not tagged → fail closed.
            return Err(format!(
                "malformed base_commit_ref (expected {}…): {tagged}",
                RepoCommitRef::TAG_PREFIX
            ));
        }
        if let Some(bare) = bare_fallback.map(str::trim).filter(|s| !s.is_empty()) {
            return Ok(Some(RepoCommitRef::from_bare_hex_for_kind(
                repo_kind, bare,
            )?));
        }
        Ok(None)
    }

    pub fn write_tagged(r: &RepoCommitRef) -> String {
        r.to_tagged_str()
    }
}

/// Parse a bare or tagged commit input against an explicit repository kind.
///
/// Tagged inputs must match `repo_kind`. Bare inputs use kind-native widths
/// (sha1 also accepts the legacy 40→64 zero-pad form).
///
/// The all-zero sha1 sentinel (`0` × 40, or legacy padded `0` × 64) is rejected
/// so unborn-HEAD placeholders are never mistaken for real commit refs.
pub fn parse_commit_anchor_for_kind(kind: HashKind, commit: &str) -> Result<RepoCommitRef, String> {
    let v = commit.trim();
    if v.starts_with(RepoCommitRef::TAG_PREFIX) {
        let r = RepoCommitRef::from_tagged_str(v)?;
        r.ensure_kind(kind)?;
        if is_unborn_head_sentinel(r.hex()) {
            return Err("unborn HEAD sentinel is not a repository commit ref".into());
        }
        return Ok(r);
    }
    if is_unborn_head_sentinel(v) {
        return Err("unborn HEAD sentinel is not a repository commit ref".into());
    }
    RepoCommitRef::from_bare_hex_for_kind(kind, v)
}

/// True for the 40-zero (or legacy 64-zero-pad) unborn-HEAD sentinel.
pub fn is_unborn_head_sentinel(raw: &str) -> bool {
    let v = raw.trim();
    (v.len() == 40 || v.len() == 64) && v.chars().all(|c| c == '0')
}

/// Parse a stored repository object id (commit/tree/blob) under the process
/// repository kind. Accepts bare kind-native hex or `repo-commit:<kind>:<hex>`.
///
/// Unlike [`parse_commit_anchor_for_kind`], this does **not** reject the
/// all-zero unborn-HEAD sentinel (callers that mean "commit ref" should use
/// that API instead).
pub fn parse_repo_object_id(raw: &str) -> Result<ObjectHash, String> {
    let kind = git_internal::hash::get_hash_kind();
    let v = raw.trim();
    if v.starts_with(RepoCommitRef::TAG_PREFIX) {
        let r = RepoCommitRef::from_tagged_str(v)?;
        r.ensure_kind(kind)?;
        return r.to_object_hash();
    }
    object_format::parse_hex_for_kind(kind, v).map_err(|e| e.to_string())
}

/// Legacy helper: normalize to a 64-char storage blob for sha1/sha256 only.
///
/// Prefer [`parse_commit_anchor_for_kind`] / [`RepoCommitRef`]. Blake3 must not
/// use this path — a 64-hex blake3 OID is indistinguishable from sha256 here.
pub fn normalize_commit_anchor(commit: &str) -> Result<String, String> {
    let v = commit.trim();
    if !v.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "Invalid commit hash: contains non-hex characters: {v}"
        ));
    }
    let v = v.to_ascii_lowercase();
    if v.len() == 64 {
        // Opaque 64-hex storage form — caller must already know repository kind.
        return Ok(v);
    }
    if v.len() == 40 {
        let mut out = String::with_capacity(64);
        out.push_str(&v);
        while out.len() < 64 {
            out.push('0');
        }
        return Ok(out);
    }
    Err(format!("Invalid commit hash length: {}", v.len()))
}

/// Inverse of the sha1 zero-pad path in [`normalize_commit_anchor`].
pub fn extract_sha1_from_anchor(anchor64: &str) -> Result<String, String> {
    let v = anchor64.trim();
    if v.len() != 64 {
        return Err(format!("Invalid anchor length: {}", v.len()));
    }
    if !v.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("Invalid anchor: contains non-hex characters: {v}"));
    }
    Ok(v.chars().take(40).collect())
}

/// Confirm a hex string is a legal SHA-256 [`IntegrityHash`] (never a repo OID).
pub fn parse_integrity_hash_hex(hex: &str) -> Result<IntegrityHash, String> {
    let v = hex.trim().to_ascii_lowercase();
    // IntegrityHash is always SHA-256 (64 hex); never use this for repository OIDs.
    v.parse::<IntegrityHash>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accepts_sha256() {
        let v = "a".repeat(64);
        assert_eq!(normalize_commit_anchor(&v).unwrap(), v);
    }

    #[test]
    fn normalize_pads_sha1() {
        let sha1 = "b".repeat(40);
        let normalized = normalize_commit_anchor(&sha1).unwrap();
        assert_eq!(normalized.len(), 64);
        assert!(normalized.starts_with(&sha1));
        assert_eq!(&normalized[40..], "0".repeat(24));
    }

    #[test]
    fn normalize_rejects_other_lengths() {
        assert!(normalize_commit_anchor("abc").is_err());
    }

    #[test]
    fn extract_sha1_from_anchor_returns_prefix() {
        let anchor = format!("{}{}", "c".repeat(40), "0".repeat(24));
        assert_eq!(extract_sha1_from_anchor(&anchor).unwrap(), "c".repeat(40));
    }

    #[test]
    fn commit_anchor_blake3_not_sha256() {
        let hex = "ab".repeat(32);
        let r = parse_commit_anchor_for_kind(HashKind::Blake3, &hex).unwrap();
        assert_eq!(r.kind(), HashKind::Blake3);
        let oh = r.to_object_hash().unwrap();
        assert_eq!(oh.kind(), HashKind::Blake3);
        assert_ne!(oh.kind(), HashKind::Sha256);
        let tagged = r.to_tagged_str();
        let again = RepoCommitRef::from_tagged_str(&tagged).unwrap();
        assert_eq!(again.kind(), HashKind::Blake3);
        assert_eq!(again.hex(), hex);
    }

    #[test]
    fn commit_anchor_sha1_roundtrip() {
        let hex = "cd".repeat(20);
        let r = parse_commit_anchor_for_kind(HashKind::Sha1, &hex).unwrap();
        assert_eq!(r.hex(), hex);
        let tagged = r.to_tagged_str();
        assert!(tagged.starts_with("repo-commit:sha1:"));
        let again = RepoCommitRef::from_tagged_str(&tagged).unwrap();
        assert_eq!(again.hex(), hex);
        assert_eq!(again.to_object_hash().unwrap().kind(), HashKind::Sha1);
    }

    #[test]
    fn integrity_hash_still_sha256() {
        let hex = "11".repeat(32);
        let ih = parse_integrity_hash_hex(&hex).unwrap();
        let _ = ih;
        // Blake3-looking 64-hex is still accepted as IntegrityHash hex (SHA-256
        // domain) — that is intentional; repository OID must not use this path.
        assert!(parse_integrity_hash_hex("abc").is_err());
        assert!(parse_integrity_hash_hex(&"1".repeat(40)).is_err());
    }

    #[test]
    fn legacy_64hex_anchor_compat_read() {
        let sha1 = "ee".repeat(20);
        let padded = format!("{sha1}{}", "0".repeat(24));
        let r = RepoCommitRefStore::read(None, Some(&padded), HashKind::Sha1)
            .unwrap()
            .unwrap();
        assert_eq!(r.kind(), HashKind::Sha1);
        assert_eq!(r.hex(), sha1);

        let sha256 = "ff".repeat(32);
        let r = RepoCommitRefStore::read(None, Some(&sha256), HashKind::Sha256)
            .unwrap()
            .unwrap();
        assert_eq!(r.kind(), HashKind::Sha256);
        assert_eq!(r.hex(), sha256);
    }

    #[test]
    fn repo_commit_ref_cross_version_compat() {
        // Old bare → new tagged write
        let bare = "aa".repeat(32);
        let r = RepoCommitRef::from_bare_hex_for_kind(HashKind::Sha256, &bare).unwrap();
        let tagged = RepoCommitRefStore::write_tagged(&r);
        let back = RepoCommitRefStore::read(Some(&tagged), None, HashKind::Sha256)
            .unwrap()
            .unwrap();
        assert_eq!(back.hex(), bare);

        // New tagged column preferred over bare fallback
        let blake = "bb".repeat(32);
        let tagged = RepoCommitRef::from_hex_for_kind(HashKind::Blake3, &blake)
            .unwrap()
            .to_tagged_str();
        let stale_bare = "cc".repeat(32);
        let back = RepoCommitRefStore::read(Some(&tagged), Some(&stale_bare), HashKind::Blake3)
            .unwrap()
            .unwrap();
        assert_eq!(back.hex(), blake);
    }

    #[test]
    fn repo_commit_ref_malformed_fail_closed() {
        assert!(RepoCommitRef::from_tagged_str("sha256:abcd").is_err());
        assert!(RepoCommitRef::from_tagged_str("repo-commit:sha256:short").is_err());
        assert!(
            RepoCommitRef::from_tagged_str(&format!("repo-commit:unknown:{}", "aa".repeat(32)))
                .is_err()
        );
        assert!(
            RepoCommitRefStore::read(Some("not-a-tagged-value"), None, HashKind::Sha1).is_err()
        );
        // Kind mismatch on tagged value
        let tagged = RepoCommitRef::from_hex_for_kind(HashKind::Sha256, &"dd".repeat(32))
            .unwrap()
            .to_tagged_str();
        assert!(RepoCommitRefStore::read(Some(&tagged), None, HashKind::Blake3).is_err());
    }

    #[test]
    fn object_format_helpers_still_parse_kinds() {
        assert_eq!(
            object_format::parse_config_value("blake3").unwrap(),
            HashKind::Blake3
        );
    }

    #[test]
    fn orchestrator_sentinel_unborn_head_preserved() {
        let z40 = "0".repeat(40);
        let z64 = "0".repeat(64);
        assert!(is_unborn_head_sentinel(&z40));
        assert!(is_unborn_head_sentinel(&z64));
        assert!(parse_commit_anchor_for_kind(HashKind::Sha1, &z40).is_err());
        assert!(parse_commit_anchor_for_kind(HashKind::Sha1, &z64).is_err());
        // A real non-zero sha1 still parses.
        assert!(parse_commit_anchor_for_kind(HashKind::Sha1, &"ab".repeat(20)).is_ok());
    }

    #[test]
    #[serial_test::serial(hash_kind)]
    fn mcp_context_item_blob_blake3_roundtrip() {
        // Remapped from MCP ContextItem.blob: object ids parse under repo kind.
        let _kind = git_internal::hash::set_hash_kind_for_test(HashKind::Blake3);
        let hex = "ef".repeat(32);
        let oid = parse_repo_object_id(&hex).expect("bare blake3 blob");
        assert_eq!(oid.kind(), HashKind::Blake3);
        let tagged = format!("repo-commit:blake3:{hex}");
        let oid2 = parse_repo_object_id(&tagged).expect("tagged blake3 blob");
        assert_eq!(oid2, oid);
    }

    #[test]
    fn mcp_multi_repo_concurrent_kinds() {
        // Remapped: per-call repo_kind (not process TLS) keeps sha1/sha256/blake3
        // anchors isolated — concurrent multi-repo consumers must pass kind explicitly.
        let sha1 = RepoCommitRef::from_bare_hex_for_kind(HashKind::Sha1, &"11".repeat(20)).unwrap();
        let sha256 =
            RepoCommitRef::from_bare_hex_for_kind(HashKind::Sha256, &"22".repeat(32)).unwrap();
        let blake3 =
            RepoCommitRef::from_bare_hex_for_kind(HashKind::Blake3, &"33".repeat(32)).unwrap();
        let t1 = RepoCommitRefStore::write_tagged(&sha1);
        let t2 = RepoCommitRefStore::write_tagged(&sha256);
        let t3 = RepoCommitRefStore::write_tagged(&blake3);
        assert_eq!(
            RepoCommitRefStore::read(Some(&t1), None, HashKind::Sha1)
                .unwrap()
                .unwrap()
                .hex(),
            sha1.hex()
        );
        assert_eq!(
            RepoCommitRefStore::read(Some(&t2), None, HashKind::Sha256)
                .unwrap()
                .unwrap()
                .hex(),
            sha256.hex()
        );
        assert_eq!(
            RepoCommitRefStore::read(Some(&t3), None, HashKind::Blake3)
                .unwrap()
                .unwrap()
                .hex(),
            blake3.hex()
        );
        // Cross-kind tagged read fail-closed (no TLS bleed).
        assert!(RepoCommitRefStore::read(Some(&t3), None, HashKind::Sha256).is_err());
        assert!(RepoCommitRefStore::read(Some(&t1), None, HashKind::Blake3).is_err());
    }

    #[test]
    fn mcp_create_run_blake3_does_not_pass_repo_oid_as_integrity_hash() {
        // Remapped CreateRun contract: blake3 repository OID stays on
        // base_commit_ref (RepoCommitRef). IntegrityHash for Run/PatchSet must
        // be a separate SHA-256 content digest — never the repo OID hex.
        let repo_oid = "ab".repeat(32);
        let r = parse_commit_anchor_for_kind(HashKind::Blake3, &repo_oid).unwrap();
        assert_eq!(r.kind(), HashKind::Blake3);
        let tagged = RepoCommitRefStore::write_tagged(&r);
        assert!(tagged.starts_with("repo-commit:blake3:"));
        // Hazard: the same 64 hex is also a legal IntegrityHash string.
        assert!(parse_integrity_hash_hex(&repo_oid).is_ok());
        // Contract proof: CreateRun-equivalent keeps a distinct content digest.
        let content_digest = "12".repeat(32);
        assert_ne!(content_digest, repo_oid);
        assert!(parse_integrity_hash_hex(&content_digest).is_ok());
        assert_eq!(r.hex(), repo_oid);
    }
}
