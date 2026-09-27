//! Remote object storage backend for Git objects
//!
//! This module provides an interface to interact with remote object storage services (like S3, R2).
//! It supports storing Git objects with a directory structure similar to the local object store,
//! but with optional prefixing for multi-tenant isolation.
//!
//! # Path Structure
//!
//! - Without prefix: `aa/bbcc...` (Standard Git object layout)
//! - With prefix: `prefix/objects/aa/bbcc...` (Isolated layout, e.g. `repo_id/objects/...`)
use std::{io::Read, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use git_internal::{errors::GitError, hash::ObjectHash, internal::object::types::ObjectType};
use object_store::{ObjectStore, ObjectStoreExt, path::Path as ObjectPath};

use super::Storage;

/// Remote object storage backend
/// Adapts object_store crate to Libra's StorageTrait
pub struct RemoteStorage {
    inner: Arc<dyn ObjectStore>,
    key_prefix: Option<String>,
}

impl RemoteStorage {
    /// Create a new RemoteStorage instance from an existing ObjectStore
    pub fn new(inner: Arc<dyn ObjectStore>) -> Self {
        Self {
            inner,
            key_prefix: None,
        }
    }

    pub fn new_with_prefix(inner: Arc<dyn ObjectStore>, key_prefix: String) -> Self {
        let key_prefix = key_prefix.trim_matches('/').to_string();
        let key_prefix = if key_prefix.is_empty() {
            None
        } else {
            Some(key_prefix)
        };
        Self { inner, key_prefix }
    }

    pub fn object_store(&self) -> Arc<dyn ObjectStore> {
        Arc::clone(&self.inner)
    }

    /// Convert ObjectHash to storage path (aa/bbcc...)
    fn hash_to_path(&self, hash: &ObjectHash) -> ObjectPath {
        let h = hash.to_string();
        match &self.key_prefix {
            Some(prefix) => {
                ObjectPath::from(format!("{}/objects/{}/{}", prefix, &h[0..2], &h[2..]))
            }
            None => ObjectPath::from(format!("{}/{}", &h[0..2], &h[2..])),
        }
    }

    pub async fn put_metadata(&self, data: &[u8]) -> Result<(), GitError> {
        let path = match &self.key_prefix {
            Some(prefix) => ObjectPath::from(format!("{}/metadata.json", prefix)),
            None => ObjectPath::from("metadata.json"),
        };

        self.inner
            .put(&path, Bytes::copy_from_slice(data).into())
            .await
            .map_err(|e| GitError::IOError(std::io::Error::other(e)))?;

        Ok(())
    }

    pub async fn get_metadata(&self) -> Result<Vec<u8>, GitError> {
        let path = match &self.key_prefix {
            Some(prefix) => ObjectPath::from(format!("{}/metadata.json", prefix)),
            None => ObjectPath::from("metadata.json"),
        };

        let result = self.inner.get(&path).await.map_err(|e| match e {
            object_store::Error::NotFound { .. } => {
                GitError::ObjectNotFound(format!("Metadata not found: {}", e))
            }
            _ => GitError::IOError(std::io::Error::other(e)),
        })?;

        let bytes = result
            .bytes()
            .await
            .map_err(|e| GitError::IOError(std::io::Error::other(e)))?;

        Ok(bytes.to_vec())
    }
}

#[async_trait]
impl Storage for RemoteStorage {
    /// Get object from remote storage
    /// Downloads, decompresses, and strips header
    async fn get(&self, hash: &ObjectHash) -> Result<(Vec<u8>, ObjectType), GitError> {
        let path = self.hash_to_path(hash);
        let result = self.inner.get(&path).await.map_err(|e| match e {
            object_store::Error::NotFound { .. } => {
                GitError::ObjectNotFound(format!("Remote object not found: {}", e))
            }
            _ => GitError::IOError(std::io::Error::other(e)),
        })?;

        let bytes = result
            .bytes()
            .await
            .map_err(|e| GitError::IOError(std::io::Error::other(e)))?;

        // Decompress
        let mut decoder = flate2::read::ZlibDecoder::new(&bytes[..]);
        let mut decompressed = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decompressed)?;

        // Strip header
        let end_of_header = decompressed
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| GitError::InvalidObjectInfo("No header terminator".into()))?;

        // Parse type
        let header_str = std::str::from_utf8(&decompressed[..end_of_header])
            .map_err(|_| GitError::InvalidObjectInfo("Invalid UTF-8 in header".into()))?;
        let obj_type_str = header_str.split(' ').next().unwrap_or("");
        let obj_type = ObjectType::from_string(obj_type_str)?;

        Ok((decompressed[end_of_header + 1..].to_vec(), obj_type))
    }

    async fn get_typed_bounded(
        &self,
        hash: &ObjectHash,
        max_payload_bytes: u64,
    ) -> Result<(Vec<u8>, ObjectType), GitError> {
        const HEADER_BUDGET: u64 = 64;
        const COMPRESSED_OVERHEAD_BUDGET: u64 = 64 * 1024;

        let max_compressed_bytes = max_payload_bytes
            .checked_add(COMPRESSED_OVERHEAD_BUDGET)
            .ok_or_else(|| GitError::InvalidObjectInfo("object read limit overflows u64".into()))?;
        let max_decoded_bytes = max_payload_bytes
            .checked_add(HEADER_BUDGET)
            .and_then(|bound| bound.checked_add(1))
            .ok_or_else(|| GitError::InvalidObjectInfo("object read limit overflows u64".into()))?;
        let path = self.hash_to_path(hash);
        let result = self.inner.get(&path).await.map_err(|error| match error {
            object_store::Error::NotFound { .. } => {
                GitError::ObjectNotFound(format!("Remote object not found: {error}"))
            }
            _ => GitError::IOError(std::io::Error::other(error)),
        })?;

        // Check the reported size before consuming the body. Also bound each
        // streamed chunk: a backend must not make a stale Content-Length turn
        // into an unbounded allocation in `GetResult::bytes()`.
        if result.meta.size > max_compressed_bytes || result.range != (0..result.meta.size) {
            return Err(GitError::InvalidObjectInfo(format!(
                "compressed object {hash} exceeds the bounded read or has invalid metadata"
            )));
        }
        let reported_size = result.meta.size;
        let mut compressed = Vec::new();
        let mut stream = result.into_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| GitError::IOError(std::io::Error::other(error)))?;
            let next_len = u64::try_from(compressed.len())
                .ok()
                .and_then(|length| length.checked_add(chunk.len() as u64))
                .ok_or_else(|| {
                    GitError::InvalidObjectInfo("compressed object size overflows u64".into())
                })?;
            if next_len > max_compressed_bytes {
                return Err(GitError::InvalidObjectInfo(format!(
                    "compressed object {hash} exceeds {max_compressed_bytes} bytes"
                )));
            }
            compressed.extend_from_slice(&chunk);
        }
        if u64::try_from(compressed.len()).ok() != Some(reported_size) {
            return Err(GitError::InvalidObjectInfo(format!(
                "compressed object {hash} length differs from storage metadata"
            )));
        }

        let decoder = flate2::read::ZlibDecoder::new(compressed.as_slice());
        let mut decoded = Vec::new();
        decoder
            .take(max_decoded_bytes)
            .read_to_end(&mut decoded)
            .map_err(|error| {
                GitError::InvalidObjectInfo(format!(
                    "remote object {hash} has invalid zlib data: {error}; verify the remote object and retry the fetch"
                ))
            })?;
        if u64::try_from(decoded.len()).map_or(true, |length| length == max_decoded_bytes) {
            return Err(GitError::InvalidObjectInfo(format!(
                "object {hash} exceeds {max_payload_bytes} bytes"
            )));
        }
        let header_end = decoded.iter().position(|byte| *byte == 0).ok_or_else(|| {
            GitError::InvalidObjectInfo(format!("remote object {hash} has no header terminator"))
        })?;
        if header_end > HEADER_BUDGET as usize {
            return Err(GitError::InvalidObjectInfo(format!(
                "remote object {hash} header is too long"
            )));
        }
        let header = std::str::from_utf8(&decoded[..header_end]).map_err(|_| {
            GitError::InvalidObjectInfo(format!("remote object {hash} header is not UTF-8"))
        })?;
        let (kind, declared_length) = header.split_once(' ').ok_or_else(|| {
            GitError::InvalidObjectInfo(format!("remote object {hash} header has no size"))
        })?;
        let object_type = ObjectType::from_string(kind).map_err(|error| {
            GitError::InvalidObjectInfo(format!(
                "remote object {hash} has invalid type header: {error}"
            ))
        })?;
        let declared_length = declared_length.parse::<u64>().map_err(|_| {
            GitError::InvalidObjectInfo(format!("remote object {hash} header has invalid size"))
        })?;
        let payload = &decoded[header_end + 1..];
        if u64::try_from(payload.len()).map_or(true, |length| length > max_payload_bytes) {
            return Err(GitError::InvalidObjectInfo(format!(
                "object {hash} exceeds {max_payload_bytes} bytes"
            )));
        }
        if u64::try_from(payload.len()).ok() != Some(declared_length) {
            return Err(GitError::InvalidObjectInfo(format!(
                "object {hash} length differs from its header"
            )));
        }
        let computed = ObjectHash::from_type_and_data_for_kind(hash.kind(), object_type, payload)
            .map_err(|error| {
            GitError::InvalidObjectInfo(format!("failed to verify remote object {hash}: {error}"))
        })?;
        if computed != *hash {
            return Err(GitError::InvalidObjectInfo(format!(
                "remote object {hash} has mismatched content ID {computed}"
            )));
        }
        Ok((payload.to_vec(), object_type))
    }

    async fn get_commit_bounded(
        &self,
        hash: &ObjectHash,
        max_payload_bytes: u64,
    ) -> Result<(Vec<u8>, ObjectType), GitError> {
        let (payload, object_type) = self.get_typed_bounded(hash, max_payload_bytes).await?;
        if object_type != ObjectType::Commit {
            return Err(GitError::InvalidObjectInfo(format!(
                "object {hash} is {object_type}, expected a commit"
            )));
        }
        Ok((payload, object_type))
    }

    /// Put object to remote storage
    /// Constructs header, compresses, and uploads
    async fn put(
        &self,
        hash: &ObjectHash,
        data: &[u8],
        obj_type: ObjectType,
    ) -> Result<String, GitError> {
        let path = self.hash_to_path(hash);

        // Construct header + content
        let header = format!("{} {}\0", obj_type, data.len());
        let mut full_content = Vec::with_capacity(header.len() + data.len());
        full_content.extend_from_slice(header.as_bytes());
        full_content.extend_from_slice(data);

        // Compress
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &full_content)?;
        let compressed = encoder.finish()?;

        // Upload
        self.inner
            .put(&path, Bytes::from(compressed).into())
            .await
            .map_err(|e| GitError::IOError(std::io::Error::other(e)))?;

        Ok(path.to_string())
    }

    async fn exist(&self, hash: &ObjectHash) -> bool {
        let path = self.hash_to_path(hash);
        self.inner.head(&path).await.is_ok()
    }

    async fn delete_payload(&self, hash: &ObjectHash) -> Result<(), GitError> {
        let path = self.hash_to_path(hash);
        match self.inner.delete(&path).await {
            Ok(()) => Ok(()),
            // Idempotent: an already-absent blob is a success.
            Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(error) => Err(GitError::IOError(std::io::Error::other(format!(
                "failed to delete durable-tier payload for {hash}: {error}"
            )))),
        }
    }

    async fn exist_checked(&self, hash: &ObjectHash) -> Result<bool, GitError> {
        let path = self.hash_to_path(hash);
        match self.inner.head(&path).await {
            Ok(_) => Ok(true),
            // A confirmed miss — the only case that may gate an eviction.
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            // Everything else (outage, credentials, throttling) is an ERROR,
            // never conflated with absence.
            Err(error) => Err(GitError::IOError(std::io::Error::other(format!(
                "durable-tier probe failed for {hash}: {error}"
            )))),
        }
    }

    async fn search(&self, prefix: &str) -> Vec<ObjectHash> {
        let list_prefix = if prefix.len() >= 2 {
            // Optimization: Git objects are stored in xx/yyyy...
            // If we have at least 2 chars, we can narrow down to the directory "xx".
            // We don't use the full prefix (e.g. "aabb") for the list_prefix because
            // object_store paths are segment-based, and "aa/bb" is not considered a parent of "aa/bbcc...".
            // So we list "aa" and filter client-side.
            match &self.key_prefix {
                Some(p) => ObjectPath::from(format!("{}/objects/{}", p, &prefix[0..2])),
                None => ObjectPath::from(&prefix[0..2]),
            }
        } else {
            // If < 2 chars, we must list the root. This is expensive but necessary for correctness.
            match &self.key_prefix {
                Some(p) => ObjectPath::from(format!("{}/objects", p)),
                None => ObjectPath::from(""),
            }
        };

        let mut results = Vec::new();

        // Use list instead of list_with_delimiter to get all objects under the prefix
        let mut stream = self.inner.list(Some(&list_prefix));

        while let Some(item) = stream.next().await {
            if let Ok(meta) = item {
                // path is like "aa/bbcc..."
                let mut path_str = meta.location.to_string();
                if let Some(p) = &self.key_prefix {
                    let expected = format!("{}/objects/", p);
                    if !path_str.starts_with(&expected) {
                        continue;
                    }
                    path_str = path_str[expected.len()..].to_string();
                }
                // Remove '/' to get hash "aabbcc..."
                let hash_str = path_str.replace('/', "");

                if hash_str.starts_with(prefix)
                    && let Ok(hash) = crate::internal::object_format::parse_repo_oid(&hash_str)
                {
                    results.push(hash);
                }
            }
        }
        results
    }

    /// Bounded-concurrency batch existence probe (`lore.md` §0.6): fire up to
    /// `max_connections()` HEAD requests at once instead of `N` sequential round
    /// trips, preserving input order. Each probe inherits object_store's
    /// 429/`SlowDown`/5xx backoff (lore.md §0.2). The concurrency cap is the
    /// global `--max-connections` / `LIBRA_MAX_CONNECTIONS` limit (lore.md §0.9),
    /// so a large batch on a big repo or CI run cannot exhaust connections.
    async fn exist_batch(&self, hashes: &[ObjectHash]) -> Vec<bool> {
        let max_concurrent = crate::utils::resource_limits::max_connections();
        futures::stream::iter(hashes.iter().copied())
            .map(|hash| async move { self.exist(&hash).await })
            .buffered(max_concurrent)
            .collect()
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;
    use git_internal::{
        hash::{HashKind, ObjectHash, set_hash_kind_for_test},
        internal::object::types::ObjectType,
    };
    use object_store::{ObjectStoreExt, memory::InMemory};

    use super::{RemoteStorage, Storage};

    fn test_hash() -> ObjectHash {
        crate::internal::object_format::parse_repo_oid("1111111111111111111111111111111111111111")
            .expect("test hash is valid")
    }

    #[tokio::test]
    async fn bounded_commit_read_succeeds() {
        let _hash_kind = set_hash_kind_for_test(HashKind::Sha1);
        let remote = RemoteStorage::new(Arc::new(InMemory::new()));
        let payload = b"tree 2222222222222222222222222222222222222222\n\nmessage\n";
        let hash =
            ObjectHash::from_type_and_data_for_kind(HashKind::Sha1, ObjectType::Commit, payload)
                .expect("commit hash is supported");
        remote
            .put(&hash, payload, ObjectType::Commit)
            .await
            .expect("store test commit");

        let (actual, kind) = remote
            .get_commit_bounded(&hash, payload.len() as u64)
            .await
            .expect("bounded commit read");
        assert_eq!(actual, payload);
        assert_eq!(kind, ObjectType::Commit);
    }

    #[tokio::test]
    async fn bounded_commit_read_rejects_large_compressed_object_before_materializing() {
        let _hash_kind = set_hash_kind_for_test(HashKind::Sha1);
        let store = Arc::new(InMemory::new());
        let remote = RemoteStorage::new(store.clone());
        let hash = test_hash();
        store
            .put(
                &remote.hash_to_path(&hash),
                Bytes::from(vec![0; 66_000]).into(),
            )
            .await
            .expect("store oversized compressed bytes");

        let error = remote
            .get_commit_bounded(&hash, 100)
            .await
            .expect_err("large compressed object must be rejected");
        assert!(error.to_string().contains("compressed object"), "{error}");
    }

    #[tokio::test]
    async fn bounded_commit_read_rejects_compression_bomb() {
        let _hash_kind = set_hash_kind_for_test(HashKind::Sha1);
        let remote = RemoteStorage::new(Arc::new(InMemory::new()));
        let hash = test_hash();
        remote
            .put(&hash, &vec![b'a'; 200_000], ObjectType::Commit)
            .await
            .expect("store compressed test commit");

        let error = remote
            .get_commit_bounded(&hash, 1_024)
            .await
            .expect_err("large decoded object must be rejected");
        assert!(error.to_string().contains("exceeds 1024 bytes"), "{error}");
    }

    #[tokio::test]
    async fn bounded_commit_read_rejects_wrong_object_type() {
        let _hash_kind = set_hash_kind_for_test(HashKind::Sha1);
        let remote = RemoteStorage::new(Arc::new(InMemory::new()));
        let hash = ObjectHash::from_type_and_data_for_kind(
            HashKind::Sha1,
            ObjectType::Blob,
            b"not a commit",
        )
        .expect("blob hash is supported");
        remote
            .put(&hash, b"not a commit", ObjectType::Blob)
            .await
            .expect("store test blob");

        let error = remote
            .get_commit_bounded(&hash, 100)
            .await
            .expect_err("blob is not a commit");
        assert!(error.to_string().contains("expected a commit"), "{error}");
    }

    #[tokio::test]
    async fn bounded_commit_read_rejects_content_id_mismatch() {
        let _hash_kind = set_hash_kind_for_test(HashKind::Sha1);
        let remote = RemoteStorage::new(Arc::new(InMemory::new()));
        let requested_hash = test_hash();
        let payload = b"tree 2222222222222222222222222222222222222222\n\nwrong object\n";
        remote
            .put(&requested_hash, payload, ObjectType::Commit)
            .await
            .expect("store commit under incorrect ID");

        let error = remote
            .get_commit_bounded(&requested_hash, 100)
            .await
            .expect_err("mismatched commit ID must be rejected");
        assert!(
            error.to_string().contains(&requested_hash.to_string()),
            "{error}"
        );
        assert!(
            error.to_string().contains("mismatched content ID"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn bounded_commit_read_reports_corrupt_zlib_as_invalid_object() {
        let _hash_kind = set_hash_kind_for_test(HashKind::Sha1);
        let store = Arc::new(InMemory::new());
        let remote = RemoteStorage::new(store.clone());
        let hash = test_hash();
        store
            .put(
                &remote.hash_to_path(&hash),
                Bytes::from_static(b"not a zlib stream").into(),
            )
            .await
            .expect("store corrupted compressed bytes");

        let error = remote
            .get_commit_bounded(&hash, 100)
            .await
            .expect_err("corrupt zlib must fail as invalid object");
        assert!(matches!(
            &error,
            git_internal::errors::GitError::InvalidObjectInfo(_)
        ));
        assert!(error.to_string().contains(&hash.to_string()), "{error}");
    }

    #[tokio::test]
    async fn bounded_commit_read_reports_unknown_type_as_invalid_object() {
        let _hash_kind = set_hash_kind_for_test(HashKind::Sha1);
        let store = Arc::new(InMemory::new());
        let remote = RemoteStorage::new(store.clone());
        let hash = test_hash();
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, b"unknown 3\0abc")
            .expect("encode malformed object");
        let compressed = encoder.finish().expect("finish malformed object");
        store
            .put(&remote.hash_to_path(&hash), Bytes::from(compressed).into())
            .await
            .expect("store malformed type header");

        let error = remote
            .get_commit_bounded(&hash, 100)
            .await
            .expect_err("unknown type must fail as invalid object");
        assert!(matches!(
            &error,
            git_internal::errors::GitError::InvalidObjectInfo(_)
        ));
        assert!(error.to_string().contains(&hash.to_string()), "{error}");
    }

    #[tokio::test]
    async fn bounded_typed_read_accepts_annotated_tag_with_explicit_hash_kind() {
        let _hash_kind = set_hash_kind_for_test(HashKind::Sha1);
        let remote = RemoteStorage::new(Arc::new(InMemory::new()));
        let payload = b"object 2222222222222222222222222222222222222222\ntype commit\ntag v1\ntagger Test <test@example.com> 0 +0000\n\nrelease\n";
        let hash =
            ObjectHash::from_type_and_data_for_kind(HashKind::Sha256, ObjectType::Tag, payload)
                .expect("tag hash is supported");
        remote
            .put(&hash, payload, ObjectType::Tag)
            .await
            .expect("store test tag");

        let (actual, kind) = remote
            .get_typed_bounded(&hash, payload.len() as u64)
            .await
            .expect("bounded tag read");
        assert_eq!(actual, payload);
        assert_eq!(kind, ObjectType::Tag);
    }

    #[tokio::test]
    async fn bounded_typed_read_rejects_tag_with_wrong_oid() {
        let _hash_kind = set_hash_kind_for_test(HashKind::Sha1);
        let remote = RemoteStorage::new(Arc::new(InMemory::new()));
        let hash = test_hash();
        remote
            .put(&hash, b"tag payload", ObjectType::Tag)
            .await
            .expect("store tag under wrong ID");

        let error = remote
            .get_typed_bounded(&hash, 100)
            .await
            .expect_err("tag with wrong ID must be rejected");
        assert!(error.to_string().contains(&hash.to_string()), "{error}");
        assert!(
            error.to_string().contains("mismatched content ID"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn bounded_typed_read_rejects_oversized_tag() {
        let _hash_kind = set_hash_kind_for_test(HashKind::Sha1);
        let remote = RemoteStorage::new(Arc::new(InMemory::new()));
        let hash = test_hash();
        remote
            .put(&hash, &vec![b'a'; 200_000], ObjectType::Tag)
            .await
            .expect("store large compressed tag");

        let error = remote
            .get_typed_bounded(&hash, 1_024)
            .await
            .expect_err("oversized tag must be rejected");
        assert!(error.to_string().contains("exceeds 1024 bytes"), "{error}");
    }
}
