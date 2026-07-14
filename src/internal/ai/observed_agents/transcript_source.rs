//! DR-04a — the unified `TranscriptSource` seam (ADR-DR-02).
//!
//! This is the **single writer read entry point** for external-agent
//! transcript content. Both the live checkpoint writer
//! (`hooks::runtime::write_committed_checkpoint`) and — once it lands (M4) —
//! the import writer resolve their bytes through
//! [`resolve_transcript_source`], never by re-opening a path themselves.
//!
//! Two source shapes exist (ADR-DR-02):
//!
//! - [`TranscriptSource::File`] — a provider-root-authorized, already-opened
//!   file handle ([`AuthorizedTranscriptFile`]). The handle is opened **once**
//!   inside the resolver after the provider-root precheck; the writer reads
//!   from the open descriptor and must never re-open by path, so a
//!   post-authorization path swap (symlink flip / TOCTOU) cannot change the
//!   bytes it reads.
//! - [`TranscriptSource::Bytes`] — in-memory bytes carrying an
//!   [`ExportAuthorized`] tag. This shape is **only** constructed by the
//!   OpenCode export bridge (DR-04b) after a trusted, sandboxed export; there
//!   is no public way to forge the tag, so the writer will not treat an
//!   arbitrary `&[u8]` as a trusted source.
//!
//! Security note (ADR-DR-13): the provider-root containment check here
//! ([`transcript_path_within_provider_root`]) is the **migration-period
//! precheck**. The final fd-relative `openat2(RESOLVE_BENEATH | …)` safe-open
//! lands with DR-05b; until then the resolver opens the path once (after the
//! precheck) and hands the writer the open handle, which already removes the
//! re-open-by-path TOCTOU on the read side.

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::internal::ai::observed_agents::{AgentSessionCtx, ObservedAgent};

/// Default effective byte cap for a single transcript read (GC-DR-04). Matches
/// the existing Claude adapter hard cap so DR-04a does not silently enlarge the
/// hook-path memory ceiling.
pub const TRANSCRIPT_READ_HARD_CAP_BYTES: u64 = 16 * 1024 * 1024;

/// Proof token that a [`TranscriptSource::File`] was opened from inside the
/// provider's own transcript root. Its field is private, so it can only be
/// minted by [`resolve_transcript_source`] in this module.
#[derive(Debug)]
pub struct ProviderRootAuthorized(());

/// Proof token that a [`TranscriptSource::Bytes`] payload came from this
/// process's own trusted export bridge (DR-04b). Fields are private and the
/// only constructor is crate-scoped [`ExportAuthorized::issue`], which binds
/// the tag to the exact bytes via SHA-256 — so no caller outside this crate
/// can mint a tag, and a tag cannot be re-attached to different bytes: the
/// writer re-verifies with [`ExportAuthorized::matches`].
#[derive(Debug, Clone)]
pub struct ExportAuthorized {
    agent_kind: String,
    session_id: String,
    content_digest: String,
}

impl ExportAuthorized {
    /// Mint an authorization tag for freshly exported `bytes`. Crate-scoped:
    /// only the verified export bridge (DR-04b) may issue tags.
    // Production caller lands with the DR-04b export bridge (M3); the digest
    // binding is unit-tested until then.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn issue(agent_kind: &str, session_id: &str, bytes: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        Self {
            agent_kind: agent_kind.to_string(),
            session_id: session_id.to_string(),
            content_digest: hex::encode(Sha256::digest(bytes)),
        }
    }

    /// Verify the tag is bound to this session AND to these exact bytes
    /// (recomputes the SHA-256). The writer must reject the source when this
    /// returns false.
    pub fn matches(&self, agent_kind: &str, session_id: &str, bytes: &[u8]) -> bool {
        use sha2::{Digest, Sha256};
        self.agent_kind == agent_kind
            && self.session_id == session_id
            && self.content_digest == hex::encode(Sha256::digest(bytes))
    }

    pub fn agent_kind(&self) -> &str {
        &self.agent_kind
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }
}

/// A transcript file that has already been safely opened inside the provider
/// root. The writer reads from the held descriptor; the path is retained only
/// for diagnostics / `source_id` derivation and is **never** re-opened.
#[derive(Debug)]
pub struct AuthorizedTranscriptFile {
    file: std::fs::File,
}

impl AuthorizedTranscriptFile {
    /// Read the transcript from the already-open descriptor, refusing to load
    /// anything larger than `cap` (matching the existing adapter baseline,
    /// which errors on oversize rather than silently truncating). Reads never
    /// re-open by path, so a concurrent path swap cannot change the bytes.
    pub fn read_bounded(&mut self, cap: u64) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        // Read one past the cap so an oversize file is detected, not silently
        // truncated; `take` still bounds memory on the hook path.
        self.file
            .by_ref()
            .take(cap.saturating_add(1))
            .read_to_end(&mut buf)
            .context("read authorized transcript handle")?;
        if buf.len() as u64 > cap {
            anyhow::bail!("transcript exceeds {cap} byte cap; refusing to load");
        }
        Ok(buf)
    }
}

/// The unified writer read source (ADR-DR-02).
#[derive(Debug)]
pub enum TranscriptSource {
    File {
        file: AuthorizedTranscriptFile,
        /// Provider-root-relative source identity (never an absolute home
        /// path — GC-DR-13 / ADR-DR-08 #6).
        source_id: String,
        auth: ProviderRootAuthorized,
    },
    Bytes {
        bytes: Vec<u8>,
        auth: ExportAuthorized,
    },
}

/// Resolve the provider root that contains `canonical_path`, if any. Mirrors
/// the Codex `$CODEX_HOME` relocation honored elsewhere in the codex chain so a
/// relocated home is not silently captured with an empty transcript.
fn provider_root_containing(adapter: &dyn ObservedAgent, canonical_path: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("LIBRA_TEST_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)?;
    adapter.protected_dirs().iter().find_map(|dir| {
        let root = if *dir == ".codex" {
            match std::env::var_os("CODEX_HOME").map(PathBuf::from) {
                Some(path) if path.is_absolute() => path,
                _ => home.join(dir),
            }
        } else {
            home.join(dir)
        };
        let root = root.canonicalize().ok()?;
        canonical_path.starts_with(&root).then_some(root)
    })
}

/// Migration-period provider-root containment precheck (ADR-DR-13). Returns
/// true when `path` canonicalises to a location inside the adapter's own
/// transcript root. Not a final TOCTOU boundary on its own — the resolver
/// additionally opens the handle once and reads from the descriptor.
pub fn transcript_path_within_provider_root(adapter: &dyn ObservedAgent, path: &Path) -> bool {
    let Ok(canonical_path) = path.canonicalize() else {
        return false;
    };
    provider_root_containing(adapter, &canonical_path).is_some()
}

/// Derive a provider-root-relative source identity for `path`. Falls back to
/// the file name when the path is not (or no longer) under the root, so we
/// never persist an absolute home path.
fn provider_root_relative_source_id(adapter: &dyn ObservedAgent, path: &Path) -> String {
    if let Ok(canonical_path) = path.canonicalize()
        && let Some(root) = provider_root_containing(adapter, &canonical_path)
        && let Ok(rel) = canonical_path.strip_prefix(&root)
    {
        return rel.to_string_lossy().into_owned();
    }
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The unified writer read entry point (ADR-DR-02).
///
/// Returns:
/// - `Ok(Some(File { … }))` when the ctx carries a `transcript_path` that
///   passes the provider-root precheck and opens successfully — the handle is
///   opened here, once.
/// - `Ok(None)` when there is no path, the path is untrusted (outside the
///   provider root), or the file is absent. The writer treats this as "no
///   transcript" and falls back to the redacted prompt, preserving existing
///   fail-open-on-absent semantics while staying fail-closed on untrusted
///   paths.
/// - `Err(_)` only on an unexpected I/O error opening a trusted, present path.
pub fn resolve_transcript_source(
    adapter: &dyn ObservedAgent,
    ctx: &AgentSessionCtx,
) -> Result<Option<TranscriptSource>> {
    let Some(path) = ctx.transcript_path.as_deref() else {
        return Ok(None);
    };
    // Authorize before invoking any provider hook: preparers may inspect the
    // file (Claude's flush-wait does), so an untrusted hook-supplied path must
    // never reach them. Re-check after preparation to narrow the component-
    // swap window before the final open.
    if !transcript_path_within_provider_root(adapter, path) {
        return Ok(None);
    }
    // DR-01 (ADR-DR-02/07): the optional flush-wait side-effect hook runs
    // BEFORE the safe open, so the handle sees a settled tail whenever the
    // budget allows. Always non-fatal.
    if let Some(preparer) = adapter.as_transcript_preparer()
        && let Err(err) = preparer.prepare_transcript(ctx)
    {
        tracing::warn!(error = %format!("{err:#}"), "transcript preparer failed; continuing");
    }
    if !transcript_path_within_provider_root(adapter, path) {
        // The path moved outside the trusted root while the preparer ran.
        return Ok(None);
    }
    match std::fs::File::open(path) {
        Ok(file) => {
            let source_id = provider_root_relative_source_id(adapter, path);
            Ok(Some(TranscriptSource::File {
                file: AuthorizedTranscriptFile { file },
                source_id,
                auth: ProviderRootAuthorized(()),
            }))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| {
            format!(
                "open authorized transcript for '{}'",
                adapter.provider_name()
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serial_test::serial;

    use super::*;
    use crate::internal::ai::observed_agents::{
        AgentKind, builtin::ClaudeCodeObservedAgent, capability::TranscriptPreparer,
    };

    #[derive(Default)]
    struct CountingPreparer {
        calls: AtomicUsize,
    }

    impl ObservedAgent for CountingPreparer {
        fn provider_kind(&self) -> AgentKind {
            AgentKind::ClaudeCode
        }

        fn provider_name(&self) -> &'static str {
            "counting-preparer"
        }

        fn read_transcript(&self, _session: &AgentSessionCtx) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }

        fn protected_dirs(&self) -> &'static [&'static str] {
            &[".claude"]
        }

        fn as_transcript_preparer(&self) -> Option<&dyn TranscriptPreparer> {
            Some(self)
        }
    }

    impl TranscriptPreparer for CountingPreparer {
        fn prepare_transcript(&self, _session: &AgentSessionCtx) -> Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// RAII guard that points `LIBRA_TEST_HOME` at `path` and restores the
    /// prior value on drop. Env mutation is `unsafe` and the tests carry
    /// `#[serial]` so it cannot race other env readers.
    fn test_ctx(path: Option<PathBuf>) -> AgentSessionCtx {
        AgentSessionCtx {
            session_id: "claude_code__t".to_string(),
            provider_session_id: "t".to_string(),
            working_dir: PathBuf::from("/tmp"),
            transcript_path: path,
        }
    }

    struct HomeGuard {
        prior: Option<std::ffi::OsString>,
    }
    impl HomeGuard {
        fn set(path: &Path) -> Self {
            let prior = std::env::var_os("LIBRA_TEST_HOME");
            unsafe { std::env::set_var("LIBRA_TEST_HOME", path) };
            Self { prior }
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prior {
                    Some(v) => std::env::set_var("LIBRA_TEST_HOME", v),
                    None => std::env::remove_var("LIBRA_TEST_HOME"),
                }
            }
        }
    }

    fn make_claude_transcript(home: &Path, name: &str, content: &[u8]) -> PathBuf {
        let dir = home.join(".claude").join("projects").join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn resolve_none_when_no_path() {
        let agent = ClaudeCodeObservedAgent::new();
        let adapter: &dyn ObservedAgent = &agent;
        assert!(
            resolve_transcript_source(adapter, &test_ctx(None))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    #[serial]
    fn resolve_none_when_untrusted_path() {
        let home = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(home.path());
        // A real file that lives OUTSIDE ~/.claude — the security gate must
        // refuse it (fail-closed) so the writer falls back to the prompt.
        let outside = home.path().join("evil.jsonl");
        std::fs::write(&outside, b"secret").unwrap();
        let agent = CountingPreparer::default();
        let adapter: &dyn ObservedAgent = &agent;
        assert!(
            resolve_transcript_source(adapter, &test_ctx(Some(outside.clone())))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            agent.calls.load(Ordering::SeqCst),
            0,
            "provider-root rejection must happen before any preparer read"
        );
    }

    #[test]
    #[serial]
    fn resolve_file_reads_bytes_and_root_relative_source_id() {
        let home = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(home.path());
        let path = make_claude_transcript(home.path(), "s.jsonl", b"hello");
        let agent = ClaudeCodeObservedAgent::new();
        let adapter: &dyn ObservedAgent = &agent;
        let src = resolve_transcript_source(adapter, &test_ctx(Some(path.clone())))
            .unwrap()
            .expect("trusted path yields a File source");
        match src {
            TranscriptSource::File {
                mut file,
                source_id,
                ..
            } => {
                assert_eq!(
                    file.read_bounded(TRANSCRIPT_READ_HARD_CAP_BYTES).unwrap(),
                    b"hello"
                );
                // Provider-root-relative identity, never an absolute home path.
                assert!(!source_id.starts_with('/'));
                assert!(source_id.contains("projects"));
                assert!(source_id.ends_with("s.jsonl"));
                assert!(!source_id.contains(home.path().to_string_lossy().as_ref()));
            }
            _ => panic!("expected File source"),
        }
    }

    // On Unix a held descriptor keeps reading the original inode even after the
    // path is unlinked and replaced, so a post-authorization symlink/path swap
    // cannot change the bytes the writer reads (the TOCTOU invariant).
    #[cfg(unix)]
    #[test]
    #[serial]
    fn open_handle_survives_path_swap() {
        let home = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(home.path());
        let path = make_claude_transcript(home.path(), "s.jsonl", b"ORIGINAL");
        let agent = ClaudeCodeObservedAgent::new();
        let adapter: &dyn ObservedAgent = &agent;
        let src = resolve_transcript_source(adapter, &test_ctx(Some(path.clone())))
            .unwrap()
            .unwrap();
        // Swap the path to a NEW file with different content after auth.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"SWAPPED-EVIL-CONTENT").unwrap();
        match src {
            TranscriptSource::File { mut file, .. } => {
                assert_eq!(
                    file.read_bounded(TRANSCRIPT_READ_HARD_CAP_BYTES).unwrap(),
                    b"ORIGINAL",
                    "held descriptor must not observe the post-auth path swap"
                );
            }
            _ => panic!("expected File source"),
        }
    }

    #[test]
    #[serial]
    fn read_bounded_refuses_oversize() {
        let home = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(home.path());
        let path = make_claude_transcript(home.path(), "big.jsonl", b"0123456789");
        let agent = ClaudeCodeObservedAgent::new();
        let adapter: &dyn ObservedAgent = &agent;
        let src = resolve_transcript_source(adapter, &test_ctx(Some(path.clone())))
            .unwrap()
            .unwrap();
        match src {
            TranscriptSource::File { mut file, .. } => {
                assert!(
                    file.read_bounded(4).is_err(),
                    "oversize transcript must be refused, not truncated"
                );
            }
            _ => panic!("expected File source"),
        }
    }

    #[test]
    fn bytes_source_carries_digest_bound_export_tag() {
        // `ExportAuthorized` can only be minted crate-side via `issue`, which
        // binds the tag to the exact bytes; `matches` re-verifies session AND
        // digest, so a tag cannot authorize different bytes.
        let bytes = b"exported".to_vec();
        let auth = ExportAuthorized::issue("opencode", "opencode__abc", &bytes);
        assert!(auth.matches("opencode", "opencode__abc", &bytes));
        assert!(
            !auth.matches("opencode", "opencode__abc", b"tampered"),
            "digest binding must reject different bytes"
        );
        assert!(
            !auth.matches("opencode", "opencode__other", &bytes),
            "session binding must reject a different session"
        );
        let src = TranscriptSource::Bytes { bytes, auth };
        match src {
            TranscriptSource::Bytes { bytes, auth } => {
                assert!(auth.matches("opencode", "opencode__abc", &bytes));
                assert_eq!(auth.agent_kind(), "opencode");
            }
            _ => panic!("expected Bytes source"),
        }
    }
}
