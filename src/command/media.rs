//! `libra media` — FastCDC LFS media chunking client (lore.md §6).
//!
//! The honest, feature-gated (`fastcdc`) v1 CLIENT surface: chunk a media file,
//! inspect/validate a manifest, reassemble+verify from the local chunk store,
//! and probe a remote's chunked-LFS capability with the §6.4 safe-fallback
//! decision. Feature-enabled LFS transfers can use Mega's authenticated media
//! endpoints, with standard LFS fallback. This module is only the CLI surface; all logic
//! lives in [`crate::utils::media`].

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::{
    internal::{config::ConfigKv, protocol::lfs_client::LFSClient},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        lfs,
        media::{
            capability, chunk_store,
            manifest::{ManifestPage, ManifestSummary},
            negotiate::{self, ProbeOutcome, TransferDecision},
            transfer::{self, TRUST_BOUNDARY_C07},
        },
        output::{OutputConfig, emit_json_data},
    },
};

pub const MEDIA_EXAMPLES: &str = "\
EXAMPLES:
    libra media chunk big.psd                 FastCDC-chunk a file; print the manifest summary
    libra media chunk big.psd --store         Also persist chunks + manifest to the local media store
    libra media chunk edit.psd --prior-manifest old/summary.json --store
                                              Re-chunk with ADR-FL-04 prior coherence (requires --store)
    libra media inspect .libra/media/fastcdc-v2020-32k/manifests/<oid>/summary.json   Validate a manifest summary
    libra media verify big.psd                Reassemble from the store and verify the media_oid
    libra media probe                         Probe the remote's chunked-LFS capability (falls back to standard LFS)
    libra media fetch pointer.bin --offset 0 --length 4096 --output slice.bin
                                              Export a byte range from finalized Media (not whole hydrate)
    libra --json media chunk big.psd          Structured JSON output for agents

NOTES:
    FastCDC media chunking is a feature-gated Libra extension (lore.md §6). The
    media_oid is always SHA-256 of the full file (standard-LFS-compatible), and
    chunks live in a private .libra/media/fastcdc-v2020-32k store outside the Git object graph.
    Cross-machine chunked transfer requires Mega built with --features fastcdc
    and a stored access token. Other remotes fall back to standard Git LFS.
    `media fetch` writes a new fragment file only; it never mutates the pointer,
    index, hydrate state, or whole-object LFS cache. Range bytes come from an
    authenticated finalized summary and covering chunks — they do not
    independently prove the full-object oid/SHA-256 (C-07).";

#[derive(Parser, Debug)]
#[command(after_help = MEDIA_EXAMPLES)]
pub struct MediaArgs {
    #[command(subcommand)]
    command: MediaCommand,
}

#[derive(Subcommand, Debug)]
enum MediaCommand {
    /// FastCDC-chunk a file and emit its media manifest.
    Chunk {
        /// The media file to chunk.
        path: String,
        /// Persist the chunks and the manifest to the local media store.
        #[clap(long)]
        store: bool,
        /// Prior paged summary (or whole-manifest JSON) for same-length coherence.
        /// Requires `--store`. Illegal priors fail without publishing a new cache.
        #[clap(long = "prior-manifest", value_name = "FILE")]
        prior_manifest: Option<String>,
    },
    /// Parse and validate a manifest JSON file.
    Inspect {
        /// Path to a `<media_oid>.json` manifest file.
        manifest: String,
    },
    /// Reassemble a media object from the local chunk store and verify its
    /// media_oid. Give a file path (its media_oid is computed) or `--media-oid`.
    Verify {
        /// The original media file whose media_oid keys its stored manifest.
        path: Option<String>,
        /// The media_oid (64-hex) directly, instead of a file.
        #[clap(long = "media-oid", conflicts_with = "path")]
        media_oid: Option<String>,
    },
    /// Probe a remote's media capability endpoint and report the transfer
    /// decision (chunked vs standard-LFS fallback).
    Probe {
        /// Remote name (default: the current branch's remote, else `origin`).
        #[clap(long)]
        remote: Option<String>,
    },
    /// Export a byte range from a finalized Media object into a new file.
    ///
    /// `path` must be an LFS pointer (or Media pointer) already published as
    /// finalized Media. Unlike `hydrate`, this never replaces the tracked file.
    Fetch {
        /// Working-tree path whose LFS pointer supplies oid/size.
        path: String,
        /// Inclusive start offset into the Media object.
        #[clap(long)]
        offset: u64,
        /// Number of bytes to export (zero writes an empty file when offset ≤ size).
        #[clap(long)]
        length: u64,
        /// Destination path; must not already exist (symlink rejected).
        #[clap(long)]
        output: String,
        /// Remote name (default: the current branch's remote, else `origin`).
        #[clap(long)]
        remote: Option<String>,
    },
}

pub async fn execute_safe(args: MediaArgs, output: &OutputConfig) -> CliResult<()> {
    match args.command {
        MediaCommand::Chunk {
            path,
            store,
            prior_manifest,
        } => chunk(&path, store, prior_manifest.as_deref(), output).await,
        MediaCommand::Inspect { manifest } => inspect(&manifest, output),
        MediaCommand::Verify { path, media_oid } => verify(path, media_oid, output).await,
        MediaCommand::Probe { remote } => probe(remote, output).await,
        MediaCommand::Fetch {
            path,
            offset,
            length,
            output: dest,
            remote,
        } => fetch(&path, offset, length, &dest, remote, output).await,
    }
}

#[derive(Serialize)]
struct ChunkSummary {
    media_oid: String,
    media_size: u64,
    chunk_count: u64,
    page_count: u32,
    unique_chunks: u64,
    manifest_id: String,
    algorithm: String,
    stored: bool,
    manifest_path: Option<String>,
}

async fn chunk(
    path: &str,
    store: bool,
    prior_manifest: Option<&str>,
    output: &OutputConfig,
) -> CliResult<()> {
    if prior_manifest.is_some() && !store {
        return Err(CliError::fatal(
            "--prior-manifest requires --store so the coherent layout is cached for upload"
                .to_string(),
        )
        .with_stable_code(StableErrorCode::CliInvalidArguments));
    }
    let stored_root;
    let scratch;
    let root = if store {
        stored_root = chunk_store::repo_media_root()
            .map_err(|err| media_store_err("open media store", err))?;
        &stored_root
    } else {
        scratch = tempfile::tempdir().map_err(|source| {
            CliError::fatal(format!(
                "failed to create a temporary media index: {source}"
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
        scratch.path()
    };
    let prior_path = prior_manifest.map(std::path::Path::new);
    let outcome = chunk_store::stream_media_file_with_prior(
        std::path::Path::new(path),
        root,
        store,
        prior_path,
    )
    .map_err(|err| media_store_err("chunk media file", err))?;
    let manifest_path = store.then(|| outcome.manifest_path.display().to_string());
    let summary = ChunkSummary {
        media_oid: outcome.summary.oid,
        media_size: outcome.summary.size,
        chunk_count: outcome.summary.chunk_count,
        page_count: outcome.summary.page_count,
        unique_chunks: outcome.unique_chunks,
        manifest_id: outcome.summary.manifest_id,
        algorithm: outcome.summary.algorithm,
        stored: store,
        manifest_path: manifest_path.clone(),
    };

    if output.is_json() {
        return emit_json_data("media.chunk", &summary, output);
    }
    if !output.quiet {
        println!("media_oid: {}", summary.media_oid);
        println!("size:      {} bytes", summary.media_size);
        println!(
            "chunks:    {} across {} pages ({} unique, algorithm {})",
            summary.chunk_count, summary.page_count, summary.unique_chunks, summary.algorithm
        );
        if let Some(p) = &manifest_path {
            println!("stored:    chunks + manifest at {p}");
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct PageInspect {
    page_no: u32,
    entry_count: usize,
}

fn inspect(manifest_path: &str, output: &OutputConfig) -> CliResult<()> {
    let path = std::path::Path::new(manifest_path);
    let target = if path.is_dir() {
        path.join("summary.json")
    } else {
        path.to_path_buf()
    };
    let text = chunk_store::read_envelope_file(&target).map_err(|err| {
        CliError::fatal(format!(
            "failed to read manifest '{}': {err}",
            target.display()
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    if let Ok(summary) = ManifestSummary::from_json(&text) {
        return emit_summary(&summary, output);
    }
    let page = ManifestPage::from_json(&text).map_err(|err| {
        CliError::fatal(format!("invalid manifest '{}': {err}", target.display()))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
    })?;
    let report = PageInspect {
        page_no: page.page_no,
        entry_count: page.entries.len(),
    };
    if output.is_json() {
        return emit_json_data("media.inspect", &report, output);
    }
    if !output.quiet {
        println!(
            "valid manifest page {} ({} entries)",
            report.page_no, report.entry_count
        );
    }
    Ok(())
}

fn emit_summary(summary: &ManifestSummary, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("media.inspect", summary, output);
    }
    if !output.quiet {
        println!("valid manifest summary (version {})", summary.version);
        println!("media_oid:    {}", summary.oid);
        println!("manifest_id:  {}", summary.manifest_id);
        println!("size:         {} bytes", summary.size);
        println!(
            "chunks:       {} across {} pages",
            summary.chunk_count, summary.page_count
        );
        println!("algorithm:    {}", summary.algorithm);
    }
    Ok(())
}

#[derive(Serialize)]
struct VerifyResult {
    media_oid: String,
    verified: bool,
}

async fn verify(
    path: Option<String>,
    media_oid: Option<String>,
    output: &OutputConfig,
) -> CliResult<()> {
    let oid = match (path, media_oid) {
        (Some(p), None) => crate::utils::lfs::calc_lfs_file_hash(&p).map_err(|source| {
            CliError::fatal(format!("failed to hash '{p}': {source}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?,
        (None, Some(oid)) => oid,
        _ => {
            return Err(CliError::command_usage(
                "provide exactly one of <path> or --media-oid",
            ));
        }
    };
    let root =
        chunk_store::repo_media_root().map_err(|err| media_store_err("open media store", err))?;
    let summary = chunk_store::load_summary(&root, &oid).map_err(|_| {
        CliError::fatal(format!(
            "no stored manifest summary for media_oid {oid} (expected {}); run 'libra media chunk --store' first",
            chunk_store::summary_path(&root, &oid).display()
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
    })?;

    // Reassemble one page at a time. verify-then-rename inside reassemble_paged
    // guarantees no partial/corrupt file survives a mismatch.
    let tmp = std::env::temp_dir().join(format!("libra-media-verify-{oid}"));
    let result = chunk_store::reassemble_paged(&root, &summary, &tmp);
    let _ = std::fs::remove_file(&tmp);

    let verified = result.is_ok();
    let vr = VerifyResult {
        media_oid: oid.clone(),
        verified,
    };
    if output.is_json() {
        emit_json_data("media.verify", &vr, output)?;
        return if verified {
            Ok(())
        } else {
            Err(CliError::fatal("media verification failed"))
        };
    }
    match result {
        Ok(()) => {
            if !output.quiet {
                println!("verified: {oid}");
            }
            Ok(())
        }
        Err(e) => Err(CliError::fatal(format!("media verification failed: {e}"))
            .with_stable_code(StableErrorCode::CliInvalidTarget)),
    }
}

#[derive(Serialize)]
struct ProbeReport {
    remote: String,
    base_url: String,
    probe: String,
    decision: String,
    reason: Option<String>,
    chunked: bool,
}

#[derive(Serialize)]
struct FetchReport {
    path: String,
    remote: String,
    output: String,
    oid: String,
    size: u64,
    offset: u64,
    length: u64,
    manifest_id: String,
    covering_chunks: u64,
    downloaded_chunks: u64,
    trust_boundary: &'static str,
}

async fn fetch(
    path: &str,
    offset: u64,
    length: u64,
    dest: &str,
    remote: Option<String>,
    output: &OutputConfig,
) -> CliResult<()> {
    let (oid, size) = lfs::parse_pointer_file(path).map_err(|e| {
        CliError::fatal(format!(
            "media fetch requires an LFS/Media pointer at '{path}': {e}"
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
    })?;
    let (remote_name, url) = resolve_remote_url(remote).await?;
    let lfs = LFSClient::from_remote_url(&url).map_err(|e| {
        CliError::fatal(format!(
            "failed to resolve LFS URL for remote '{remote_name}': {e}"
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
    })?;
    let client = transfer::MediaClient::discover_for_range(lfs.client.clone(), &lfs.lfs_url)
        .await
        .map_err(|e| {
            CliError::fatal(format!("media fetch negotiation failed: {e}"))
                .with_stable_code(StableErrorCode::NetworkProtocol)
        })?;
    let store = chunk_store::MediaChunkStore::open();
    let report = client
        .export_range(
            &oid,
            size,
            offset,
            length,
            std::path::Path::new(dest),
            &store,
        )
        .await
        .map_err(|e| {
            CliError::fatal(format!("media fetch failed: {e}"))
                .with_stable_code(StableErrorCode::NetworkProtocol)
        })?;
    let body = FetchReport {
        path: path.to_string(),
        remote: remote_name,
        output: dest.to_string(),
        oid: report.oid,
        size: report.size,
        offset: report.offset,
        length: report.length,
        manifest_id: report.manifest_id,
        covering_chunks: report.covering_chunks,
        downloaded_chunks: report.downloaded_chunks,
        trust_boundary: TRUST_BOUNDARY_C07,
    };
    if output.is_json() {
        return emit_json_data("media.fetch", &body, output);
    }
    if !output.quiet {
        println!(
            "wrote {} bytes from {}@{}+{} (covering {}, downloaded {})",
            body.length,
            body.oid,
            body.offset,
            body.length,
            body.covering_chunks,
            body.downloaded_chunks
        );
        println!("trust: {}", body.trust_boundary);
        println!("output: {}", body.output);
    }
    Ok(())
}

async fn probe(remote: Option<String>, output: &OutputConfig) -> CliResult<()> {
    // Resolve the remote URL: explicit --remote, else the current branch's
    // remote, else `origin`.
    let (remote_name, url) = resolve_remote_url(remote).await?;

    let outcome = capability::probe(&url).await;
    // Report what WOULD happen assuming the repo enabled chunked LFS and a local
    // fallback object is available — i.e. characterise the remote itself.
    let decision = negotiate::negotiate(&outcome, true, true);
    let (decision_str, reason, chunked) = describe(&decision);
    let probe_str = match &outcome {
        ProbeOutcome::Ok(_) => "ok",
        ProbeOutcome::NoEndpoint => "no-endpoint",
        ProbeOutcome::ServerErrorAfterBackoff => "server-error-after-backoff",
    }
    .to_string();

    let report = ProbeReport {
        remote: remote_name,
        base_url: crate::utils::redact::redact_url_credentials(&url),
        probe: probe_str,
        decision: decision_str,
        reason,
        chunked,
    };
    if output.is_json() {
        return emit_json_data("media.probe", &report, output);
    }
    if !output.quiet {
        println!("remote:   {} ({})", report.remote, report.base_url);
        println!("probe:    {}", report.probe);
        match &report.reason {
            Some(r) => println!("decision: {} ({r})", report.decision),
            None => println!("decision: {}", report.decision),
        }
    }
    Ok(())
}

async fn resolve_remote_url(remote: Option<String>) -> CliResult<(String, String)> {
    match remote {
        Some(name) => {
            let u = ConfigKv::get_remote_url(&name).await.map_err(|_| {
                CliError::fatal(format!("remote '{name}' has no configured URL"))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
            })?;
            Ok((name, u))
        }
        None => match ConfigKv::get_current_remote_url().await {
            Ok(Some(u)) => Ok(("origin".to_string(), u)),
            _ => {
                let u = ConfigKv::get_remote_url("origin").await.map_err(|_| {
                    CliError::fatal("no remote configured (pass --remote <name>)")
                        .with_stable_code(StableErrorCode::CliInvalidTarget)
                })?;
                Ok(("origin".to_string(), u))
            }
        },
    }
}

fn describe(decision: &TransferDecision) -> (String, Option<String>, bool) {
    match decision {
        TransferDecision::Chunked { algorithm } => (format!("chunked ({algorithm})"), None, true),
        TransferDecision::StandardLfs { reason } => (
            "standard-lfs (fallback)".to_string(),
            Some(reason.as_str().to_string()),
            false,
        ),
        TransferDecision::Block { reason } => (
            "blocked".to_string(),
            Some(reason.as_str().to_string()),
            false,
        ),
    }
}

fn media_store_err(action: &str, e: chunk_store::MediaStoreError) -> CliError {
    CliError::fatal(format!("failed to {action}: {e}"))
        .with_stable_code(StableErrorCode::IoWriteFailed)
}
