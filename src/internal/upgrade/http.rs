//! Dedicated HTTPS client for the auto-upgrade subsystem (plan-20260714
//! §A.6 HTTP client/下载).
//!
//! Deliberately NOT the shared `https_client` machinery: upgrade traffic
//! pins `https_only(true)`, refuses every redirect (`Policy::none()` — a 3xx
//! is a hard failure, stricter than the shared no-downgrade policy), sets
//! conservative connect/read deadlines, and re-checks the effective URL
//! before reading any body byte. Download streaming is bounded by
//! [`SizeGate`]: a declared `Content-Length` larger than the manifest's
//! artifact size (or the global 128 MiB cap) aborts before the body, every
//! chunk is counted with immediate abort past the expected size, and the
//! stream must end at EXACTLY the expected size with a matching sha256.

use reqwest::header::{DATE, HeaderValue};
use sha2::{Digest, Sha256};

use super::manifest::{MAX_ARTIFACT_BYTES, MAX_MANIFEST_BYTES};

/// Connect deadline for upgrade traffic. The §A.7 Phase-A soft budgets
/// (5 s manifest / 10 s download) are enforced by the caller on top.
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Idle read deadline (resets while bytes flow).
pub const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Upgrade HTTP failures. None of these may write control/time state (§A.6).
#[derive(Debug, thiserror::Error)]
pub enum UpgradeHttpError {
    #[error("upgrade endpoint URL '{url}' is not https")]
    NotHttps { url: String },
    #[error("cannot build the upgrade HTTP client: {0}")]
    ClientBuild(String),
    #[error("upgrade request to {url} failed: {detail}")]
    Request { url: String, detail: String },
    #[error("upgrade endpoint redirected ({status}) — redirects are refused")]
    Redirected { status: u16 },
    #[error("upgrade endpoint returned HTTP status {status}")]
    Status { status: u16 },
    #[error("response URL '{effective}' does not match the requested '{requested}'")]
    EffectiveUrlMismatch {
        requested: String,
        effective: String,
    },
    #[error("manifest response exceeds the {MAX_MANIFEST_BYTES}-byte limit")]
    ManifestTooLarge,
    #[error("artifact stream violated its size bound: {0}")]
    SizeViolation(String),
    #[error("artifact sha256 mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },
    #[error("failed writing the downloaded artifact: {0}")]
    Sink(String),
}

/// A fetched manifest: raw bytes plus the HTTPS `Date` header (§A.6 时间 —
/// provisional per-round value; policy interpretation happens in the state
/// module).
#[derive(Debug)]
pub struct FetchedManifest {
    pub bytes: Vec<u8>,
    /// Raw `Date` header as sent by the server, if present/representable.
    pub https_date: Option<String>,
}

/// Build the dedicated client (see module docs).
pub fn upgrade_http_client() -> Result<reqwest::Client, UpgradeHttpError> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .http1_only()
        .build()
        .map_err(|e| UpgradeHttpError::ClientBuild(e.to_string()))
}

/// Shared response gate: refuse 3xx outright, refuse non-success, and verify
/// the effective URL still matches the request before any body read (§A.6).
fn check_response(requested: &str, response: &reqwest::Response) -> Result<(), UpgradeHttpError> {
    let status = response.status();
    if status.is_redirection() {
        return Err(UpgradeHttpError::Redirected {
            status: status.as_u16(),
        });
    }
    if !status.is_success() {
        return Err(UpgradeHttpError::Status {
            status: status.as_u16(),
        });
    }
    let effective = response.url().as_str();
    if effective != requested {
        return Err(UpgradeHttpError::EffectiveUrlMismatch {
            requested: requested.to_string(),
            effective: effective.to_string(),
        });
    }
    Ok(())
}

fn require_https(url: &str) -> Result<(), UpgradeHttpError> {
    if !url.starts_with("https://") {
        return Err(UpgradeHttpError::NotHttps {
            url: url.to_string(),
        });
    }
    Ok(())
}

/// Fetch the manifest endpoint: ≤ 1 MiB, streamed with an incremental size
/// gate, capturing the HTTPS `Date` header.
pub async fn fetch_manifest(
    client: &reqwest::Client,
    url: &str,
) -> Result<FetchedManifest, UpgradeHttpError> {
    require_https(url)?;
    let request_err = |e: reqwest::Error| UpgradeHttpError::Request {
        url: url.to_string(),
        detail: e.to_string(),
    };
    let mut response = client.get(url).send().await.map_err(request_err)?;
    check_response(url, &response)?;
    let https_date = response
        .headers()
        .get(DATE)
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .map(str::to_string);
    if let Some(declared) = response.content_length()
        && declared > MAX_MANIFEST_BYTES as u64
    {
        return Err(UpgradeHttpError::ManifestTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(request_err)? {
        if bytes.len() + chunk.len() > MAX_MANIFEST_BYTES {
            return Err(UpgradeHttpError::ManifestTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(FetchedManifest { bytes, https_date })
}

/// Incremental stream-size accounting (§A.6 下载), pure and unit-testable:
/// abort on oversized `Content-Length` before the body, abort the moment the
/// running count exceeds the expected size, and require the final count to
/// land EXACTLY on it.
#[derive(Debug)]
pub struct SizeGate {
    expected: u64,
    seen: u64,
}

impl SizeGate {
    /// `expected` must already satisfy the manifest bound
    /// `0 < expected <= 128 MiB`; violations are rejected here again as
    /// defense in depth.
    pub fn new(expected: u64) -> Result<Self, UpgradeHttpError> {
        if expected == 0 || expected > MAX_ARTIFACT_BYTES {
            return Err(UpgradeHttpError::SizeViolation(format!(
                "expected size {expected} outside (0, {MAX_ARTIFACT_BYTES}]"
            )));
        }
        Ok(Self { expected, seen: 0 })
    }

    /// Check a declared `Content-Length` before reading any body byte.
    pub fn check_declared(&self, declared: Option<u64>) -> Result<(), UpgradeHttpError> {
        if let Some(declared) = declared
            && (declared > self.expected || declared > MAX_ARTIFACT_BYTES)
        {
            return Err(UpgradeHttpError::SizeViolation(format!(
                "declared Content-Length {declared} exceeds expected {}",
                self.expected
            )));
        }
        Ok(())
    }

    /// Account one chunk; aborts as soon as the running total passes the
    /// expected size (never buffers past the bound).
    pub fn push_chunk(&mut self, len: u64) -> Result<(), UpgradeHttpError> {
        self.seen = self.seen.saturating_add(len);
        if self.seen > self.expected {
            return Err(UpgradeHttpError::SizeViolation(format!(
                "stream exceeded expected size {} (saw at least {})",
                self.expected, self.seen
            )));
        }
        Ok(())
    }

    /// The stream ended; the byte count must be exact.
    pub fn finish(&self) -> Result<(), UpgradeHttpError> {
        if self.seen != self.expected {
            return Err(UpgradeHttpError::SizeViolation(format!(
                "stream ended at {} bytes, expected exactly {}",
                self.seen, self.expected
            )));
        }
        Ok(())
    }
}

/// Download an artifact into `sink`, enforcing [`SizeGate`] and the expected
/// sha256 (lowercase hex). The caller owns the destination file semantics
/// (candidate naming, fsync, cleanup) and the Phase-A total deadline.
pub async fn download_artifact_to(
    client: &reqwest::Client,
    url: &str,
    expected_size: u64,
    expected_sha256: &str,
    sink: &mut (impl std::io::Write + Send),
) -> Result<(), UpgradeHttpError> {
    require_https(url)?;
    let request_err = |e: reqwest::Error| UpgradeHttpError::Request {
        url: url.to_string(),
        detail: e.to_string(),
    };
    let mut gate = SizeGate::new(expected_size)?;
    let mut response = client.get(url).send().await.map_err(request_err)?;
    check_response(url, &response)?;
    gate.check_declared(response.content_length())?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = response.chunk().await.map_err(request_err)? {
        gate.push_chunk(chunk.len() as u64)?;
        hasher.update(&chunk);
        sink.write_all(&chunk)
            .map_err(|e| UpgradeHttpError::Sink(e.to_string()))?;
    }
    gate.finish()?;
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(UpgradeHttpError::DigestMismatch {
            expected: expected_sha256.to_ascii_lowercase(),
            actual,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_gate_rejects_out_of_range_expectations() {
        assert!(SizeGate::new(0).is_err());
        assert!(SizeGate::new(MAX_ARTIFACT_BYTES + 1).is_err());
        assert!(SizeGate::new(1).is_ok());
        assert!(SizeGate::new(MAX_ARTIFACT_BYTES).is_ok());
    }

    #[test]
    fn size_gate_rejects_oversized_content_length_before_body() {
        let gate = SizeGate::new(100).unwrap();
        assert!(gate.check_declared(Some(101)).is_err());
        assert!(gate.check_declared(Some(100)).is_ok());
        // A smaller/absent declaration is allowed; the exact-count rule at
        // finish() still applies (wrong Content-Length cannot smuggle bytes).
        assert!(gate.check_declared(Some(10)).is_ok());
        assert!(gate.check_declared(None).is_ok());
    }

    #[test]
    fn size_gate_aborts_mid_stream_and_requires_exact_end() {
        let mut gate = SizeGate::new(10).unwrap();
        assert!(gate.push_chunk(6).is_ok());
        assert!(gate.finish().is_err(), "short stream must fail");
        assert!(gate.push_chunk(4).is_ok());
        assert!(gate.finish().is_ok(), "exact stream passes");
        assert!(gate.push_chunk(1).is_err(), "overflow aborts immediately");
    }

    #[test]
    fn size_gate_overflow_is_saturating_not_wrapping() {
        let mut gate = SizeGate::new(MAX_ARTIFACT_BYTES).unwrap();
        assert!(gate.push_chunk(u64::MAX).is_err());
    }

    #[test]
    fn client_refuses_plain_http_urls_before_any_request() {
        assert!(matches!(
            require_https("http://download.libra.tools/x"),
            Err(UpgradeHttpError::NotHttps { .. })
        ));
        assert!(require_https("https://download.libra.tools/x").is_ok());
    }

    #[test]
    fn upgrade_client_builds_with_pinned_policies() {
        // Builder success is the observable contract here; policy behavior
        // (redirect refusal, https_only, stalled reads) is exercised against
        // a live test server in the A-9 `upgrade_auto_test` target (§A.11).
        assert!(upgrade_http_client().is_ok());
    }
}
