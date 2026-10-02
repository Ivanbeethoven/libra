//! Fast reload cache for the Git index.
//!
//! Every libra command on a large repo pays a full parse of the index file: 174k
//! entries / 19 MB measured at **292 ms per load**, and status/commit each load it
//! once or twice per process. The parse is CPU-bound and dominated by the stat
//! metadata the git format carries per entry (ctime/mtime/dev/ino/uid/gid) — data
//! libra's hot paths (the staged diff, the ScorpioFS upper-layer candidates, the
//! commit tree build) never read.
//!
//! This module caches a reduced projection — `(name, oid, mode, size)` — in a
//! compact sidecar next to the index, keyed by the index file's `(mtime, size)`.
//! A cache hit rebuilds the `Index` in ~tens of ms instead of ~292 ms; a stale or
//! missing cache falls back to the real parser and rewrites the cache. The cache
//! is a pure optimisation: every failure path degrades to `Index::load`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use git_internal::{
    errors::GitError,
    hash::{HashKind, ObjectHash},
    internal::index::{Flags, Index, IndexEntry, Time},
};

/// A zeroed `Time` — the fast cache does not carry stat metadata, and `Time`'s
/// fields are private in git-internal 0.8.4, so build one through its public
/// stream parser (eight zero bytes = 0s 0ns).
fn zero_time() -> Option<Time> {
    let zero = [0u8; 8];
    Time::from_stream(&mut &zero[..]).ok()
}

const MAGIC: &[u8; 4] = b"LIFC";
/// Self-keyed snapshot: same entry encoding, header carries the tree oid the
/// snapshot describes (see [`FastIndex::write_snapshot_to`]).
const SNAPSHOT_MAGIC: &[u8; 4] = b"LIFS";
const VERSION: u32 = 1;

fn cache_path(index_path: &Path) -> PathBuf {
    index_path.with_extension("fastcache")
}

/// Source fingerprint: the cache is valid only while the index file's mtime and
/// length are unchanged. mtime alone is enough in practice; length makes an
/// accidental same-nanosecond rewrite even less likely to alias.
fn fingerprint(index_path: &Path) -> Option<(u128, u64)> {
    let md = fs::metadata(index_path).ok()?;
    let mtime = md.modified().ok()?;
    let nanos = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((nanos, md.len()))
}

/// Load the index at `index_path`, preferring the fast cache.
pub fn load(index_path: &Path) -> Result<Index, GitError> {
    if let Some(index) = try_load_cache(index_path) {
        return Ok(index);
    }
    let index = Index::load(index_path)?;
    write_cache(index_path, &index);
    Ok(index)
}

fn try_load_cache(index_path: &Path) -> Option<Index> {
    let (mtime, len) = fingerprint(index_path)?;
    let data = fs::read(cache_path(index_path)).ok()?;
    if data.len() < 4 + 4 + 16 + 4 || &data[0..4] != MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(data[4..8].try_into().ok()?);
    if version != VERSION {
        return None;
    }
    let cached_mtime = u128::from_le_bytes(data[8..24].try_into().ok()?);
    let cached_len = u64::from_le_bytes(data[24..32].try_into().ok()?);
    if cached_mtime != mtime || cached_len != len {
        return None;
    }
    let count = u32::from_le_bytes(data[32..36].try_into().ok()?) as usize;

    let mut index = Index::new();
    let mut pos = 36usize;
    for _ in 0..count {
        let name_len = u16::from_le_bytes(data.get(pos..pos + 2)?.try_into().ok()?) as usize;
        pos += 2;
        let name = String::from_utf8(data.get(pos..pos + name_len)?.to_vec()).ok()?;
        pos += name_len;
        let kind = match *data.get(pos)? {
            0 => HashKind::Sha1,
            1 => HashKind::Sha256,
            _ => return None,
        };
        pos += 1;
        let hash_len = kind.size();
        // 0.8.4 infers the kind from the byte length.
        let hash = ObjectHash::from_bytes(data.get(pos..pos + hash_len)?).ok()?;
        if hash.kind() != kind {
            return None;
        }
        pos += hash_len;
        let mode = u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?);
        pos += 4;
        let size = u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?);
        pos += 4;

        let t = zero_time()?;
        index.update(IndexEntry {
            ctime: t.clone(),
            mtime: t,
            dev: 0,
            ino: 0,
            mode,
            uid: 0,
            gid: 0,
            size,
            hash,
            flags: Flags::new(name_len as u16),
            name,
        });
    }
    Some(index)
}

fn write_cache(index_path: &Path, index: &Index) {
    let Some((mtime, len)) = fingerprint(index_path) else {
        return;
    };
    let entries = index.tracked_entries(0);
    let mut out: Vec<u8> = Vec::with_capacity(entries.len() * 80 + 40);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&mtime.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in &entries {
        let name = e.name.as_bytes();
        let Ok(name_len) = u16::try_from(name.len()) else {
            return;
        };
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(name);
        let kind = e.hash.kind();
        out.push(match kind {
            HashKind::Sha1 => 0,
            HashKind::Sha256 => 1,
        });
        out.extend_from_slice(&e.hash.to_data());
        out.extend_from_slice(&e.mode.to_le_bytes());
        out.extend_from_slice(&e.size.to_le_bytes());
    }
    // Best-effort: a failed cache write only costs the next parse.
    let path = cache_path(index_path);
    let tmp = path.with_extension("fastcache.tmp");
    if let Ok(mut f) = fs::File::create(&tmp) {
        if f.write_all(&out).is_ok() {
            let _ = fs::rename(&tmp, &path);
            return;
        }
    }
    let _ = fs::remove_file(&tmp);
}

/// A lightweight, read-only projection of the index: entries sorted by name, each
/// `(name, oid, mode, size)`. No `BTreeMap`, no stat metadata.
///
/// Why this exists next to [`load`]: the git-internal `Index` stores entries in a
/// `BTreeMap`, so merely *building* one costs ~250 ms at 174k entries — measured as
/// the floor under every status/commit, and higher than git's entire `status` on
/// the same repo. The hot read paths (the staged diff, the ScorpioFS candidate
/// classification, the commit tree build) only ever do `get(name)` and a full scan,
/// both of which a sorted `Vec` serves with a binary search.
pub struct FastIndex {
    entries: Vec<FastEntry>,
}

pub struct FastEntry {
    pub name: String,
    pub hash: ObjectHash,
    pub mode: u32,
    pub size: u32,
}

impl FastIndex {
    /// Load the fastcache if fresh, else parse the real index once and cache it.
    pub fn load(index_path: &Path) -> Result<Self, GitError> {
        if let Some(fi) = Self::try_load_cache(index_path) {
            return Ok(fi);
        }
        let index = Index::load(index_path)?;
        write_cache(index_path, &index);
        Ok(Self::from_index(&index))
    }

    pub fn from_index(index: &Index) -> Self {
        let entries = index
            .tracked_entries(0)
            .into_iter()
            .map(|e| FastEntry {
                name: e.name.clone(),
                hash: e.hash,
                mode: e.mode,
                size: e.size,
            })
            .collect();
        Self { entries }
    }

    fn try_load_cache(index_path: &Path) -> Option<Self> {
        let (mtime, len) = fingerprint(index_path)?;
        let data = fs::read(cache_path(index_path)).ok()?;
        if data.len() < 36 || &data[0..4] != MAGIC {
            return None;
        }
        if u32::from_le_bytes(data[4..8].try_into().ok()?) != VERSION {
            return None;
        }
        if u128::from_le_bytes(data[8..24].try_into().ok()?) != mtime
            || u64::from_le_bytes(data[24..32].try_into().ok()?) != len
        {
            return None;
        }
        let count = u32::from_le_bytes(data[32..36].try_into().ok()?) as usize;
        let mut entries = Vec::with_capacity(count);
        let mut pos = 36usize;
        for _ in 0..count {
            let name_len = u16::from_le_bytes(data.get(pos..pos + 2)?.try_into().ok()?) as usize;
            pos += 2;
            let name = String::from_utf8(data.get(pos..pos + name_len)?.to_vec()).ok()?;
            pos += name_len;
            let kind = match *data.get(pos)? {
                0 => HashKind::Sha1,
                1 => HashKind::Sha256,
                _ => return None,
            };
            pos += 1;
            let hash = ObjectHash::from_bytes(data.get(pos..pos + kind.size())?).ok()?;
            pos += kind.size();
            let mode = u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?);
            pos += 4;
            let size = u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?);
            pos += 4;
            entries.push(FastEntry {
                name,
                hash,
                mode,
                size,
            });
        }
        Some(Self { entries })
    }

    /// Binary search by path name (entries are written in sorted order).
    pub fn get(&self, name: &str) -> Option<&FastEntry> {
        self.entries
            .binary_search_by(|e| e.name.as_str().cmp(name))
            .ok()
            .map(|i| &self.entries[i])
    }

    /// Positional access into the sorted entry list.
    pub fn entry(&self, i: usize) -> &FastEntry {
        &self.entries[i]
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, FastEntry> {
        self.entries.iter()
    }

    /// Serialize back into the cache format (used to publish the sidecar).
    pub fn write_cache_to(&self, index_path: &Path) {
        let Some((mtime, len)) = fingerprint(index_path) else {
            return;
        };
        let mut out: Vec<u8> = Vec::with_capacity(self.entries.len() * 80 + 40);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&mtime.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        write_entries(&mut out, &self.entries);
        atomic_write(&cache_path(index_path), &out);
    }

    /// Publish this projection as a self-keyed snapshot: the same entry
    /// encoding, but the header carries `key` — the tree oid the snapshot
    /// describes — instead of an index fingerprint. This is what the commit
    /// path writes as the committed state: the diff only ever reads
    /// (name, oid, mode), so a 19 MB copy of the index itself is 19 MB of
    /// write-only I/O per commit.
    pub fn write_snapshot_to(&self, path: &Path, key: &str) {
        let mut out: Vec<u8> = Vec::with_capacity(self.entries.len() * 80 + 40);
        out.extend_from_slice(SNAPSHOT_MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        let Ok(key_len) = u16::try_from(key.len()) else {
            return;
        };
        out.extend_from_slice(&key_len.to_le_bytes());
        out.extend_from_slice(key.as_bytes());
        write_entries(&mut out, &self.entries);
        atomic_write(path, &out);
    }

    /// Load a snapshot written by [`Self::write_snapshot_to`], only when its
    /// embedded key equals `key` (the caller passes the tree oid of HEAD, so
    /// any HEAD move invalidates it by construction).
    pub fn load_snapshot(path: &Path, key: &str) -> Option<Self> {
        let data = fs::read(path).ok()?;
        if data.len() < 12 || &data[0..4] != SNAPSHOT_MAGIC {
            return None;
        }
        if u32::from_le_bytes(data[4..8].try_into().ok()?) != VERSION {
            return None;
        }
        let key_len = u16::from_le_bytes(data[8..10].try_into().ok()?) as usize;
        if data.get(10..10 + key_len)? != key.as_bytes() {
            return None;
        }
        parse_entries(&data, 10 + key_len)
    }
}

fn write_entries(out: &mut Vec<u8>, entries: &[FastEntry]) {
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        let name = e.name.as_bytes();
        let Ok(name_len) = u16::try_from(name.len()) else {
            return;
        };
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(name);
        out.push(match e.hash.kind() {
            HashKind::Sha1 => 0,
            HashKind::Sha256 => 1,
        });
        out.extend_from_slice(&e.hash.to_data());
        out.extend_from_slice(&e.mode.to_le_bytes());
        out.extend_from_slice(&e.size.to_le_bytes());
    }
}

fn parse_entries(data: &[u8], mut pos: usize) -> Option<FastIndex> {
    let count = u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?) as usize;
    pos += 4;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = u16::from_le_bytes(data.get(pos..pos + 2)?.try_into().ok()?) as usize;
        pos += 2;
        let name = String::from_utf8(data.get(pos..pos + name_len)?.to_vec()).ok()?;
        pos += name_len;
        let kind = match *data.get(pos)? {
            0 => HashKind::Sha1,
            1 => HashKind::Sha256,
            _ => return None,
        };
        pos += 1;
        let hash = ObjectHash::from_bytes(data.get(pos..pos + kind.size())?).ok()?;
        pos += kind.size();
        let mode = u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?);
        pos += 4;
        let size = u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?);
        pos += 4;
        entries.push(FastEntry {
            name,
            hash,
            mode,
            size,
        });
    }
    Some(FastIndex { entries })
}

fn atomic_write(path: &Path, out: &[u8]) {
    let tmp = path.with_extension("fastcache.tmp");
    if let Ok(mut f) = fs::File::create(&tmp) {
        if f.write_all(out).is_ok() {
            let _ = fs::rename(&tmp, path);
            return;
        }
    }
    let _ = fs::remove_file(&tmp);
}

/// Serialize `index` to the git index v2 format in ONE write syscall.
/// `Index::save` (git-internal's `to_file`) writes each entry with its own
/// `write_all` on an unbuffered `File` — 174k syscalls, measured at **442 ms
/// per save** on a 174k-entry index, where the syscall count dominates. The
/// bytes produced here are identical (the layout is fixed by the index spec):
/// header, entries in BTreeMap order, then the checksum trailer. Falls back to
/// `Index::save` when conflict stages are present, whose entry set differs from
/// `tracked_entries(0)`.
pub fn write_index_fast(index: &Index, path: &Path) -> std::io::Result<()> {
    use git_internal::utils::HashAlgorithm;

    let entries = index.tracked_entries(0);
    if entries.len() != index.size() {
        return index
            .save(path)
            .map_err(|e| std::io::Error::other(e.to_string()));
    }
    let hash_len = git_internal::hash::get_hash_kind().size();
    let mut out: Vec<u8> = Vec::with_capacity(entries.len() * (hash_len + 80) + 32);
    out.extend_from_slice(b"DIRC");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for e in &entries {
        for time in [&e.ctime, &e.mtime] {
            // `Time`'s fields are private in git-internal 0.8.4; its `Display`
            // is the only public read of them ("seconds:nanos").
            let rendered = time.to_string();
            let (secs, nanos) = rendered
                .split_once(':')
                .ok_or_else(|| std::io::Error::other("invalid Time display"))?;
            let secs: u32 = secs
                .parse()
                .map_err(|_| std::io::Error::other("invalid Time seconds"))?;
            let nanos: u32 = nanos
                .parse()
                .map_err(|_| std::io::Error::other("invalid Time nanos"))?;
            out.extend_from_slice(&secs.to_be_bytes());
            out.extend_from_slice(&nanos.to_be_bytes());
        }
        out.extend_from_slice(&e.dev.to_be_bytes());
        out.extend_from_slice(&e.ino.to_be_bytes());
        out.extend_from_slice(&e.mode.to_be_bytes());
        out.extend_from_slice(&e.uid.to_be_bytes());
        out.extend_from_slice(&e.gid.to_be_bytes());
        out.extend_from_slice(&e.size.to_be_bytes());
        out.extend_from_slice(&e.hash.to_data());
        let flags: u16 = (&e.flags)
            .try_into()
            .map_err(|_| std::io::Error::other("index flags overflow"))?;
        out.extend_from_slice(&flags.to_be_bytes());
        out.extend_from_slice(e.name.as_bytes());
        let entry_len = hash_len + 2 + e.name.len();
        let padding = 1 + ((8 - ((entry_len + 1) % 8)) % 8);
        out.resize(out.len() + padding, 0);
    }
    let mut hasher = HashAlgorithm::new();
    hasher.update(&out);
    out.extend_from_slice(&hasher.finalize());
    fs::write(path, &out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(name: &str, stage: u8, mode: u32) -> IndexEntry {
        let mut flags = Flags::new(name.len() as u16);
        flags.stage = stage;
        IndexEntry {
            ctime: Time::from_system_time(UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789)),
            mtime: Time::from_system_time(UNIX_EPOCH + Duration::new(1_700_000_001, 987_654_321)),
            dev: 2049,
            ino: 424242,
            mode,
            uid: 1000,
            gid: 1000,
            size: 1234,
            hash: ObjectHash::from_type_and_data(
                git_internal::internal::object::types::ObjectType::Blob,
                name.as_bytes(),
            ),
            flags,
            name: name.to_string(),
        }
    }

    fn build_index() -> Index {
        let mut index = Index::new();
        // Names chosen to exercise every padding class of the entry layout.
        for (i, name) in ["a", "ab", "abc", "abcd", "abcde", "abcdef", "abcdefg", "abcdefgh"]
            .iter()
            .enumerate()
        {
            index.update(entry(name, 0, 0o100644 + (i as u32 & 1) * 0o111));
        }
        index.update(entry("dir/nested/file.rs", 0, 0o100644));
        index
    }

    #[test]
    fn fast_writer_is_byte_identical_to_to_file() {
        let dir = std::env::temp_dir().join(format!("lifc-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let index = build_index();
        let a = dir.join("a.index");
        let b = dir.join("b.index");
        index.to_file(&a).unwrap();
        write_index_fast(&index, &b).unwrap();
        assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
        // And the fast writer's output reloads into an equal index.
        let reloaded = Index::load(&b).unwrap();
        assert_eq!(reloaded.size(), index.size());
        for e in index.tracked_entries(0) {
            let r = reloaded.get(&e.name, 0).expect("entry survives round-trip");
            assert_eq!(r.hash, e.hash);
            assert_eq!(r.mode, e.mode);
            assert_eq!(r.size, e.size);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fast_writer_falls_back_when_conflict_stages_present() {
        let dir = std::env::temp_dir().join(format!("lifc-test-s{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut index = build_index();
        index.update(entry("conflicted", 1, 0o100644));
        let a = dir.join("a.index");
        let b = dir.join("b.index");
        index.to_file(&a).unwrap();
        write_index_fast(&index, &b).unwrap();
        assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }
}
