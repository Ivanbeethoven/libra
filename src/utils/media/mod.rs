//! FastCDC LFS media chunking — CLIENT substrate (lore.md §6).
//!
//! This module is the honest v1 of lore.md §6 "LFS FastCDC chunking": a
//! strictly feature-gated (`fastcdc`, default OFF) **client** layer that
//! content-defines chunks of a media object, builds a versioned manifest,
//! stores chunks in a local content-addressed store, reassembles + verifies,
//! and negotiates a remote's chunked-vs-standard-LFS capability with an airtight
//! safe fallback. It ships ZERO real cross-machine chunked transfer: the
//! Libra-aware media SERVER (§6.5–6.8 endpoints, chunk upload/download,
//! manifest finalize, GC/fsck/heal, and every §6.7 anti-side-channel guarantee)
//! is a separate deliverable that is honestly FROZEN in lore.md §6 — against
//! every reachable remote today the capability probe resolves to standard Git
//! LFS fallback.
//!
//! ## Invariants (load-bearing)
//!
//! - **Git object graph untouched (§6.2):** a chunk is NEVER a Git object ID.
//!   Chunks and manifests live in a `.libra/media/` store that is a physical
//!   sibling of `objects/` and is never walked as loose objects; `chunk_hash`
//!   and `media_oid` address RAW bytes, never `blob <size>\0`-wrapped content.
//! - **`media_oid` is always SHA-256 (§6.3):** it reuses [`crate::utils::lfs`]'s
//!   `calc_lfs_file_hash` (ring SHA-256 over the full file), independent of the
//!   repository `core.objectformat`, so a SHA-1 repo still emits a `media_oid`
//!   byte-identical to a standard LFS pointer's `oid sha256:…`.
//! - **Never half-write (§6.4):** the capability negotiation defaults to
//!   standard LFS on any doubt and BLOCKS (never silently produces a chunk-only
//!   artifact) when the remote cannot serve a standard fallback and no local
//!   fallback object exists. Reassembly verifies the full `media_oid` BEFORE the
//!   atomic publish.

pub mod capability;
pub mod chunk_store;
pub mod chunker;
pub mod manifest;
pub mod negotiate;

use ring::digest::{Context, SHA256};

/// Lowercase-hex SHA-256 of RAW bytes (no Git object framing). Used for both
/// `chunk_hash` and any in-memory digest; the whole-file `media_oid` streams
/// through [`crate::utils::lfs::calc_lfs_file_hash`] (same primitive).
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut ctx = Context::new(&SHA256);
    ctx.update(bytes);
    hex::encode(ctx.finish().as_ref())
}

/// Whether a string is exactly 64 ASCII-lowercase-hex characters (a SHA-256
/// digest in the canonical form used by `media_oid`/`chunk_hash`).
pub(crate) fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
