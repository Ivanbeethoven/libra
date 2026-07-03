//! Remote capability negotiation → transfer decision (lore.md §6.4).
//!
//! This is the safety core: a PURE function that, given the probe outcome, the
//! repo policy, and whether a local complete fallback object exists, decides
//! whether a transfer may use chunked LFS or must fall back to standard LFS —
//! and, crucially, BLOCKS (never silently produces a chunk-only artifact) when
//! the remote cannot serve a standard fallback and no local fallback exists
//! (§6.4:438, "never half-write"). The default for a fully-compatible remote is
//! Chunked; every doubt degrades to standard LFS.

use super::{capability::Capabilities, chunker};

/// Outcome of probing the remote's media-capability endpoint. Distinguishes a
/// missing endpoint (404 / connection refused) from a server error that
/// survived §0.2 backoff, because the two map to different fallback reasons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The endpoint answered with a capability document.
    Ok(Capabilities),
    /// No capability endpoint (404 / connection refused / DNS) — plain remote.
    NoEndpoint,
    /// The endpoint returned 429/5xx and did not recover after §0.2 retries.
    ServerErrorAfterBackoff,
}

/// The decided transfer mode for a media object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferDecision {
    /// Use Libra chunked LFS with the given algorithm.
    Chunked { algorithm: String },
    /// Fall back to standard Git LFS (safe default), with the reason.
    StandardLfs { reason: FallbackReason },
    /// Refuse the operation: chunked would be the only option but there is no
    /// standard fallback safety net (server refuses fallback AND no local
    /// complete object). Never silently produce a chunk-only artifact.
    Block { reason: BlockReason },
}

/// Why a transfer fell back to standard LFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    NoCapabilityEndpoint,
    ServerErrorAfterBackoff,
    UnknownHigherVersion,
    ChunkedDisabledByServer,
    IncompatibleAlgorithm,
    /// The server advertises chunked LFS but cannot accept our frozen max chunk
    /// size or lacks a required API (batch existence), so chunked transfer is not
    /// viable.
    InsufficientServerCapability,
    DisabledByRepoPolicy,
}

/// Why a transfer was blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// The server does not keep a standard LFS fallback object AND the client
    /// has no local complete fallback object — a chunk-only upload would leave
    /// no interoperable object, so the operation is refused.
    NoFallbackAndServerRefuses,
}

impl FallbackReason {
    pub fn as_str(self) -> &'static str {
        match self {
            FallbackReason::NoCapabilityEndpoint => "no-capability-endpoint",
            FallbackReason::ServerErrorAfterBackoff => "server-error-after-backoff",
            FallbackReason::UnknownHigherVersion => "unknown-higher-version",
            FallbackReason::ChunkedDisabledByServer => "chunked-disabled-by-server",
            FallbackReason::IncompatibleAlgorithm => "incompatible-algorithm",
            FallbackReason::InsufficientServerCapability => "insufficient-server-capability",
            FallbackReason::DisabledByRepoPolicy => "disabled-by-repo-policy",
        }
    }
}

impl BlockReason {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockReason::NoFallbackAndServerRefuses => "no-fallback-and-server-refuses",
        }
    }
}

/// Highest media-protocol major version this client understands.
const SUPPORTED_MAJOR: u64 = 1;

/// Decide the transfer mode (§6.4 matrix). First match wins; the all-green case
/// (a fully-compatible remote with a fallback safety net) defaults to Chunked.
///
/// - `probe`: the capability-probe outcome.
/// - `repo_policy_chunked_enabled`: whether this repo allows chunked LFS.
/// - `local_fallback_present`: whether a complete standard LFS media object is
///   available locally as a fallback safety net.
pub fn negotiate(
    probe: &ProbeOutcome,
    repo_policy_chunked_enabled: bool,
    local_fallback_present: bool,
) -> TransferDecision {
    let caps = match probe {
        ProbeOutcome::NoEndpoint => {
            return TransferDecision::StandardLfs {
                reason: FallbackReason::NoCapabilityEndpoint,
            };
        }
        ProbeOutcome::ServerErrorAfterBackoff => {
            return TransferDecision::StandardLfs {
                reason: FallbackReason::ServerErrorAfterBackoff,
            };
        }
        ProbeOutcome::Ok(caps) => caps,
    };

    // Safe default on an unrecognized higher major version.
    if parse_major(&caps.version).is_none_or(|major| major > SUPPORTED_MAJOR) {
        return TransferDecision::StandardLfs {
            reason: FallbackReason::UnknownHigherVersion,
        };
    }
    if !caps.chunked_lfs {
        return TransferDecision::StandardLfs {
            reason: FallbackReason::ChunkedDisabledByServer,
        };
    }
    let algo_ok = caps
        .chunk_algorithms
        .iter()
        .any(|a| a == chunker::ALGORITHM);
    let hash_ok = caps.hash_algorithms.iter().any(|h| h == "sha256");
    if !algo_ok || !hash_ok {
        return TransferDecision::StandardLfs {
            reason: FallbackReason::IncompatibleAlgorithm,
        };
    }
    // The server must accept our frozen maximum chunk size and expose the batch
    // existence API that chunk dedup depends on; otherwise chunked transfer would
    // fail mid-stream. (Range read is only for deferred range-hydration, so it is
    // NOT required for the basic chunked upload/download decision.)
    if caps.max_chunk_size < chunker::MAX_SIZE as u64 || !caps.supports_batch_exists {
        return TransferDecision::StandardLfs {
            reason: FallbackReason::InsufficientServerCapability,
        };
    }
    if !repo_policy_chunked_enabled {
        return TransferDecision::StandardLfs {
            reason: FallbackReason::DisabledByRepoPolicy,
        };
    }
    // Chunked is viable. NEVER half-write: if the server keeps no standard
    // fallback object AND we have no local complete object, refuse rather than
    // create a chunk-only artifact nothing else can read.
    if !caps.supports_standard_lfs_fallback && !local_fallback_present {
        return TransferDecision::Block {
            reason: BlockReason::NoFallbackAndServerRefuses,
        };
    }
    TransferDecision::Chunked {
        algorithm: chunker::ALGORITHM.to_string(),
    }
}

/// Parse the leading integer of a `"<major>"` / `"<major>.<minor>"` version.
fn parse_major(version: &str) -> Option<u64> {
    version.split('.').next()?.trim().parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_caps() -> Capabilities {
        Capabilities {
            version: "1".to_string(),
            chunked_lfs: true,
            chunk_algorithms: vec!["fastcdc-v1".to_string()],
            hash_algorithms: vec!["sha256".to_string()],
            max_chunk_size: 8 * 1024 * 1024,
            max_manifest_size: 10 * 1024 * 1024,
            supports_batch_exists: true,
            supports_range_read: true,
            supports_standard_lfs_fallback: true,
        }
    }

    #[test]
    fn all_green_defaults_to_chunked() {
        // The positive happy path — a buggy negotiate() returning StandardLfs or
        // Block here must FAIL this test.
        let d = negotiate(&ProbeOutcome::Ok(good_caps()), true, true);
        assert_eq!(
            d,
            TransferDecision::Chunked {
                algorithm: "fastcdc-v1".to_string()
            }
        );
        // Chunked is still chosen when the server keeps a fallback even without a
        // local fallback object.
        assert!(matches!(
            negotiate(&ProbeOutcome::Ok(good_caps()), true, false),
            TransferDecision::Chunked { .. }
        ));
    }

    #[test]
    fn every_fallback_row() {
        use FallbackReason::*;
        let sl = |c, reason| {
            assert_eq!(
                negotiate(&ProbeOutcome::Ok(c), true, true),
                TransferDecision::StandardLfs { reason }
            );
        };
        assert_eq!(
            negotiate(&ProbeOutcome::NoEndpoint, true, true),
            TransferDecision::StandardLfs {
                reason: NoCapabilityEndpoint
            }
        );
        assert_eq!(
            negotiate(&ProbeOutcome::ServerErrorAfterBackoff, true, true),
            TransferDecision::StandardLfs {
                reason: ServerErrorAfterBackoff
            }
        );
        let mut c = good_caps();
        c.version = "2".to_string();
        sl(c, UnknownHigherVersion);
        let mut c = good_caps();
        c.version = "not-a-number".to_string();
        sl(c, UnknownHigherVersion);
        let mut c = good_caps();
        c.chunked_lfs = false;
        sl(c, ChunkedDisabledByServer);
        let mut c = good_caps();
        c.chunk_algorithms = vec!["fastcdc-v9".to_string()];
        sl(c, IncompatibleAlgorithm);
        let mut c = good_caps();
        c.hash_algorithms = vec!["blake3".to_string()];
        sl(c, IncompatibleAlgorithm);
        let mut c = good_caps();
        c.max_chunk_size = 1024; // smaller than our frozen MAX_SIZE
        sl(c, InsufficientServerCapability);
        let mut c = good_caps();
        c.supports_batch_exists = false;
        sl(c, InsufficientServerCapability);
        // repo policy disabled → fallback (checked with an otherwise-green caps)
        assert_eq!(
            negotiate(&ProbeOutcome::Ok(good_caps()), false, true),
            TransferDecision::StandardLfs {
                reason: DisabledByRepoPolicy
            }
        );
    }

    #[test]
    fn block_when_no_fallback_and_server_refuses() {
        let mut c = good_caps();
        c.supports_standard_lfs_fallback = false;
        assert_eq!(
            negotiate(&ProbeOutcome::Ok(c.clone()), true, false),
            TransferDecision::Block {
                reason: BlockReason::NoFallbackAndServerRefuses
            }
        );
        // …but a local fallback object rescues it → chunked.
        assert!(matches!(
            negotiate(&ProbeOutcome::Ok(c), true, true),
            TransferDecision::Chunked { .. }
        ));
    }
}
