//! Helpers to read or write compressed git objects on disk, returning raw payloads and computing their object hashes.

use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use flate2::read::ZlibDecoder;
use git_internal::{errors::GitError, hash::ObjectHash};

/// Helper function to read and decompress a git object from the object database.
pub fn read_git_object(git_dir: &Path, hash: &ObjectHash) -> Result<Vec<u8>, GitError> {
    let hash_str = hash.to_string();
    let object_path = git_dir
        .join("objects")
        .join(&hash_str[..2])
        .join(&hash_str[2..]);

    let file = fs::File::open(object_path)?;
    let mut decoder = ZlibDecoder::new(file);
    let mut buffer = Vec::new();
    decoder.read_to_end(&mut buffer)?;

    // The buffer now contains "<type> <size>\0<content>", where <type> is the git object type (e.g., commit, tree, blob, tag)
    // Strip the header (which contains the object type and size) to obtain only the object content.
    if let Some(header_end) = buffer.iter().position(|&b| b == 0) {
        Ok(buffer[header_end + 1..].to_vec())
    } else {
        Err(GitError::InvalidObjectInfo(
            "Could not find object header terminator".to_string(),
        ))
    }
}

/// Read a git object but decode at most `max_content_bytes` of content,
/// returning `(content, truncated)`. Unlike [`read_git_object`], this never
/// decompresses the whole blob into memory — the zlib stream is read
/// through a bounded reader, so a corrupt/hostile object whose inflated
/// size dwarfs the cap cannot force an unbounded allocation (AG-24a raw
/// export must respect `agent.max_transcript_read_bytes`).
pub fn read_git_object_bounded(
    git_dir: &Path,
    hash: &ObjectHash,
    max_content_bytes: u64,
) -> Result<(Vec<u8>, bool), GitError> {
    // Hard cap on the "<type> <size>\0" header. A legitimate git header is
    // well under this (type is a short word, size is decimal digits); more
    // means a corrupt object. Parsing the header separately — rather than
    // assuming it fits within a fixed content-slack — is what makes
    // truncation detection independent of header length (codex review R4).
    const HEADER_MAX: usize = 64;
    let hash_str = hash.to_string();
    let object_path = git_dir
        .join("objects")
        .join(&hash_str[..2])
        .join(&hash_str[2..]);

    let file = fs::File::open(object_path)?;
    let mut decoder = ZlibDecoder::new(file);

    // 1. Consume the header up to (and including) the NUL terminator,
    //    byte-by-byte under the hard cap. The header bytes themselves are
    //    discarded — callers only want the content.
    let mut byte = [0u8; 1];
    let mut header_len = 0usize;
    loop {
        let n = decoder.read(&mut byte)?;
        if n == 0 {
            return Err(GitError::InvalidObjectInfo(
                "object stream ended before the header terminator".to_string(),
            ));
        }
        if byte[0] == 0 {
            break;
        }
        header_len += 1;
        if header_len > HEADER_MAX {
            return Err(GitError::InvalidObjectInfo(
                "object header exceeds the maximum size (corrupt object)".to_string(),
            ));
        }
    }

    // 2. Read exactly `max_content_bytes + 1` content bytes. Observing the
    //    extra byte proves there is more content than the cap, so
    //    truncation is detected by content length alone — no dependence on
    //    how large the header was.
    let read_limit = max_content_bytes.saturating_add(1);
    let mut content = Vec::new();
    decoder.take(read_limit).read_to_end(&mut content)?;
    let truncated = content.len() as u64 > max_content_bytes;
    if truncated {
        content.truncate(max_content_bytes as usize);
    }
    Ok((content, truncated))
}

/// Helper function to write a git object to the object database.
pub fn write_git_object(
    git_dir: &Path,
    object_type: &str,
    data: &[u8],
) -> Result<ObjectHash, GitError> {
    let header = format!("{} {}\0", object_type, data.len());
    let mut content = header.into_bytes();
    content.extend_from_slice(data);
    let hash = ObjectHash::new(&content);
    let hash_str = hash.to_string();

    let object_path = git_dir
        .join("objects")
        .join(&hash_str[..2])
        .join(&hash_str[2..]);

    if !object_path.exists() {
        // INVARIANT: `object_path` is built by joining `git_dir` with three
        // additional components ("objects", first-2-of-hash, rest-of-hash),
        // so `.parent()` always returns the directory holding the loose
        // object file.
        let parent = object_path
            .parent()
            .expect("loose-object path always has a parent directory");
        fs::create_dir_all(parent)?;
        let file = fs::File::create(object_path)?;
        let mut encoder = flate2::write::ZlibEncoder::new(file, flate2::Compression::default());
        encoder.write_all(&content)?;
        encoder.finish()?;
    }

    Ok(hash)
}

#[cfg(test)]
mod bounded_read_tests {
    use super::{read_git_object_bounded, write_git_object};

    /// Bounded reads never return more than the cap, flag truncation only
    /// when real content exceeds the cap, and truncation detection does not
    /// depend on the object header length.
    #[test]
    fn bounded_read_truncates_and_flags_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path();
        std::fs::create_dir_all(git_dir.join("objects")).unwrap();

        // A "blob" (short header) with 1000 content bytes.
        let content = vec![b'x'; 1000];
        let hash = write_git_object(git_dir, "blob", &content).unwrap();

        // Cap above content: full read, not truncated.
        let (got, truncated) = read_git_object_bounded(git_dir, &hash, 2000).unwrap();
        assert!(!truncated);
        assert_eq!(got, content);

        // Cap exactly at content length: full read, not truncated
        // (truncation requires observing MORE than the cap).
        let (got, truncated) = read_git_object_bounded(git_dir, &hash, 1000).unwrap();
        assert!(!truncated);
        assert_eq!(got.len(), 1000);

        // Cap below content: truncated to the cap.
        let (got, truncated) = read_git_object_bounded(git_dir, &hash, 100).unwrap();
        assert!(truncated, "content beyond the cap must flag truncation");
        assert_eq!(got.len(), 100);

        // A longer object type name ("commit") does not shift truncation.
        let hash2 = write_git_object(git_dir, "commit", &content).unwrap();
        let (got, truncated) = read_git_object_bounded(git_dir, &hash2, 100).unwrap();
        assert!(truncated);
        assert_eq!(got.len(), 100);
    }

    /// Zero-cap reads flag truncation for any non-empty object and never
    /// allocate content.
    #[test]
    fn bounded_read_zero_cap() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path();
        std::fs::create_dir_all(git_dir.join("objects")).unwrap();
        let hash = write_git_object(git_dir, "blob", b"non-empty").unwrap();
        let (got, truncated) = read_git_object_bounded(git_dir, &hash, 0).unwrap();
        assert!(truncated);
        assert!(got.is_empty());
    }
}
