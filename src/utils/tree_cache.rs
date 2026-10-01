//! Cache-tree sidecar: the committed tree's `directory -> tree oid` map.
//!
//! Every libra commit used to rebuild the ENTIRE tree from the index: on a
//! 174k-entry repo that is ~900 ms of grouping, hashing and serializing 1273
//! trees, of which all but the handful on the changed paths are byte-identical
//! to the parent commit's. Git avoids this with its cache-tree extension; this
//! is the libra equivalent, kept in a sidecar next to the index.
//!
//! The cache is keyed by the tree oid it describes, so any HEAD move (branch
//! switch, rebase, commit from elsewhere) invalidates it by construction. When
//! it is usable, [`IncrementalBuilder`] rebuilds only the directories that are
//! ancestors of a staged change and reuses the recorded oids for the rest; any
//! miss (missing dir, missing cache) falls back to rebuilding that subtree from
//! the index, which is always correct — the index is the source of truth, the
//! cache only ever supplies oids for subtrees the index agrees are unchanged.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use git_internal::{
    hash::{HashKind, ObjectHash},
    internal::{
        index::Index,
        object::{
            ObjectTrait,
            tree::{Tree, TreeItem, TreeItemMode},
            types::ObjectType,
        },
    },
};

use crate::utils::{fast_index::FastEntry, tree::sort_tree_items_for_git};

const MAGIC: &[u8; 4] = b"LTC2";
const VERSION: u32 = 1;

/// Every ancestor directory (plus the root `""`) of the given changed paths.
/// These are the only directories a commit needs to rebuild.
pub fn changed_dirs<'a, I>(paths: I) -> HashSet<String>
where
    I: Iterator<Item = &'a PathBuf>,
{
    let mut dirs = HashSet::new();
    dirs.insert(String::new());
    for path in paths {
        let s = crate::utils::util::path_to_string(path);
        for (i, b) in s.bytes().enumerate() {
            if b == b'/' {
                dirs.insert(s[..i].to_string());
            }
        }
    }
    dirs
}

pub struct TreeCache {
    key: String,
    dirs: HashMap<String, ObjectHash>,
}

impl TreeCache {
    /// Load the cache only when it describes `head_tree`; otherwise `None` and
    /// the caller takes the full-rebuild path.
    pub fn load(path: &Path, head_tree: &ObjectHash) -> Option<Self> {
        let data = fs::read(path).ok()?;
        if data.len() < 12 || &data[0..4] != MAGIC {
            return None;
        }
        if u32::from_le_bytes(data[4..8].try_into().ok()?) != VERSION {
            return None;
        }
        let key_len = u16::from_le_bytes(data[8..10].try_into().ok()?) as usize;
        let key = String::from_utf8(data.get(10..10 + key_len)?.to_vec()).ok()?;
        if key != head_tree.to_string() {
            return None;
        }
        let mut pos = 10 + key_len;
        let count = u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?) as usize;
        pos += 4;
        let mut dirs = HashMap::with_capacity(count);
        for _ in 0..count {
            let path_len = u16::from_le_bytes(data.get(pos..pos + 2)?.try_into().ok()?) as usize;
            pos += 2;
            let dir = String::from_utf8(data.get(pos..pos + path_len)?.to_vec()).ok()?;
            pos += path_len;
            let kind = match *data.get(pos)? {
                0 => HashKind::Sha1,
                1 => HashKind::Sha256,
                _ => return None,
            };
            pos += 1;
            let oid = ObjectHash::from_bytes(data.get(pos..pos + kind.size())?).ok()?;
            pos += kind.size();
            dirs.insert(dir, oid);
        }
        Some(Self { key, dirs })
    }

    pub fn save(&self, path: &Path) {
        let mut out: Vec<u8> = Vec::with_capacity(self.dirs.len() * 60 + 32);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        let key = self.key.as_bytes();
        let Ok(key_len) = u16::try_from(key.len()) else {
            return;
        };
        out.extend_from_slice(&key_len.to_le_bytes());
        out.extend_from_slice(key);
        out.extend_from_slice(&(self.dirs.len() as u32).to_le_bytes());
        for (dir, oid) in &self.dirs {
            let Ok(dir_len) = u16::try_from(dir.len()) else {
                return;
            };
            out.extend_from_slice(&dir_len.to_le_bytes());
            out.extend_from_slice(dir.as_bytes());
            out.push(match oid.kind() {
                HashKind::Sha1 => 0,
                HashKind::Sha256 => 1,
            });
            out.extend_from_slice(&oid.to_data());
        }
        // Best-effort, like the other sidecars: a failed write only costs the
        // next commit one full rebuild.
        let tmp = path.with_extension("trees.tmp");
        if let Ok(mut f) = fs::File::create(&tmp) {
            use std::io::Write;
            if f.write_all(&out).is_ok() {
                let _ = fs::rename(&tmp, path);
                return;
            }
        }
        let _ = fs::remove_file(&tmp);
    }

    fn oid_of(&self, dir: &str) -> Option<&ObjectHash> {
        self.dirs.get(dir)
    }
}

/// Rebuilds a tree from the index, reusing cached subtree oids wherever no
/// staged change touches them.
pub struct IncrementalBuilder<'a> {
    entries: Vec<&'a FastEntry>,
    cache: Option<&'a TreeCache>,
    changed: &'a HashSet<String>,
    persist: bool,
    pending: &'a mut Vec<(ObjectHash, Vec<u8>)>,
    rebuilt: HashMap<String, ObjectHash>,
}

impl<'a> IncrementalBuilder<'a> {
    pub fn new(
        index: &'a crate::utils::fast_index::FastIndex,
        cache: Option<&'a TreeCache>,
        changed: &'a HashSet<String>,
        persist: bool,
        pending: &'a mut Vec<(ObjectHash, Vec<u8>)>,
    ) -> Self {
        Self {
            entries: index.iter().collect(),
            cache,
            changed,
            persist,
            pending,
            rebuilt: HashMap::new(),
        }
    }

    pub fn build(&mut self) -> Result<Tree, String> {
        self.build_dir("")
    }

    /// The cache for the tree this builder produced: previous entries (still
    /// valid for every directory that was not rebuilt) plus the rebuilt ones.
    pub fn into_new_cache(self, tree: &Tree) -> TreeCache {
        let mut dirs = match self.cache {
            Some(cache) => cache.dirs.clone(),
            None => HashMap::new(),
        };
        dirs.extend(self.rebuilt);
        TreeCache {
            key: tree.id.to_string(),
            dirs,
        }
    }

    fn build_dir(&mut self, dir: &str) -> Result<Tree, String> {
        // The whole point: a directory no staged change touches is exactly the
        // parent commit's directory, so its oid can be reused verbatim.
        if !self.changed.contains(dir) {
            if let Some(oid) = self.cache.and_then(|c| c.oid_of(dir)) {
                return Ok(Tree {
                    id: *oid,
                    tree_items: Vec::new(),
                });
            }
        }

        // Entries under `dir/` are contiguous in the name-sorted entry list.
        let (start, end) = self.range_of(dir);
        let prefix_len = if dir.is_empty() { 0 } else { dir.len() + 1 };
        let mut items: Vec<TreeItem> = Vec::new();
        let mut child_dirs: Vec<String> = Vec::new();
        for entry in &self.entries[start..end] {
            let rest = &entry.name[prefix_len..];
            match rest.find('/') {
                None => {
                    let mode =
                        TreeItemMode::tree_item_type_from_bytes(format!("{:o}", entry.mode).as_bytes())
                            .map_err(|e| format!("invalid mode for {}: {e}", entry.name))?;
                    items.push(TreeItem {
                        name: rest.to_string(),
                        mode,
                        id: entry.hash,
                    });
                }
                Some(pos) => {
                    // Entries are sorted, so each child directory is one run.
                    let comp = &rest[..pos];
                    if child_dirs.last().map(|s| s.as_str()) != Some(comp) {
                        child_dirs.push(comp.to_string());
                    }
                }
            }
        }
        for comp in child_dirs {
            let child_path = if dir.is_empty() {
                comp.clone()
            } else {
                format!("{dir}/{comp}")
            };
            let sub = self.build_dir(&child_path)?;
            items.push(TreeItem {
                name: comp,
                mode: TreeItemMode::Tree,
                id: sub.id,
            });
        }

        sort_tree_items_for_git(&mut items);
        let tree = if items.is_empty() {
            let id = ObjectHash::from_type_and_data(ObjectType::Tree, &[]);
            Tree::from_bytes(&[], id).map_err(|e| e.to_string())?
        } else {
            Tree::from_tree_items(items).map_err(|e| e.to_string())?
        };
        self.rebuilt.insert(dir.to_string(), tree.id);
        if self.persist {
            let data = tree.to_data().map_err(|e| e.to_string())?;
            self.pending.push((tree.id, data));
        }
        Ok(tree)
    }

    /// The `[start, end)` slice of `entries` whose names live under `dir/`.
    /// Uses `partition_point` (binary search) so the scan cost is proportional
    /// to the directory's own entries, not the whole index.
    fn range_of(&self, dir: &str) -> (usize, usize) {
        if dir.is_empty() {
            return (0, self.entries.len());
        }
        let start_key = format!("{dir}/");
        // '/' + 1: any name under `dir/` sorts below this, and no name outside does.
        let end_key = format!("{dir}0");
        let start = self
            .entries
            .partition_point(|e| e.name.as_str() < start_key.as_str());
        let end = self
            .entries
            .partition_point(|e| e.name.as_str() < end_key.as_str());
        (start, end)
    }
}
