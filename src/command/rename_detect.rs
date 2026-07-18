//! Shared rename-detection engine (plan-20260714 Part B §B.4).
//!
//! `match_pairs` implements Git's diffcore-rename stage order — exact OID
//! buckets, unique-basename pairing, then a bounded exhaustive inexact pass —
//! over side-agnostic [`RenameSnapshot`]s so `status` (and, incrementally,
//! `diff`) share one deterministic scorer instead of re-implementing greedy
//! heuristics per command.
//!
//! Content is pulled through a [`RenameContentSource`] so callers control
//! where bytes come from (repository objects, the worktree, or test fixtures)
//! and how read budgets apply. The engine itself never touches the
//! filesystem or the object store.
//!
//! `diff` currently consumes only [`similarity_score`]; the snapshot/engine
//! surface is wired into `status` by slices R0-2/R0-4 (plan-20260714 §B.8).
//! The module-wide `dead_code` allow below MUST be removed in R0-4 — it only
//! exists so this engine slice can land reviewed and unit-tested first.
#![allow(dead_code)]

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::OsString,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use git_internal::{errors::GitError, hash::ObjectHash};

/// Git's exact-match similarity score (`-M100%`).
pub(crate) const EXACT_SCORE: u32 = 60000;
/// Per-destination retained inexact edges (§B.4.2.5).
pub(crate) const PER_DEST_TOP_K: usize = 4;
/// `status` inexact comparison budget (§B.7); `diff` defaults to `None`.
pub(crate) const STATUS_MAX_SIMILARITY_COMPARISONS: u64 = 500_000;

/// Object-read budget defaults (§B.3.4): per-object cap, total cap, object
/// count cap, and wall-clock deadline for the whole scoring batch.
pub(crate) const OBJECT_READ_MAX_OBJECT_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const OBJECT_READ_MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const OBJECT_READ_MAX_OBJECTS: u32 = 64;
pub(crate) const OBJECT_READ_DEADLINE: Duration = Duration::from_secs(5);

/// Worktree-read budget defaults (§B.3.4/§B.7).
pub(crate) const WORKTREE_READ_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const WORKTREE_READ_MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const WORKTREE_READ_MAX_TASKS: u32 = 4096;
pub(crate) const WORKTREE_READ_DEADLINE: Duration = Duration::from_secs(5);

/// Blob kind, derived from the Git mode bits. Inexact scoring only ever
/// pairs `Regular` with `Regular`; exact pairing additionally requires the
/// kinds to be equal (and for gitlinks, the modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum BlobKind {
    Regular,
    Symlink,
    Gitlink,
}

impl BlobKind {
    pub(crate) fn from_mode(mode: u32) -> Self {
        match mode & 0o170000 {
            0o160000 => BlobKind::Gitlink,
            0o120000 => BlobKind::Symlink,
            _ => BlobKind::Regular,
        }
    }
}

/// Where a blob's OID came from (§B.4.1). Only `KnownObjectId` and
/// `ComputedWorktreeThisCall` may participate in exact pairing; `Unknown`
/// blobs join inexact scoring only after their content is successfully read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlobEvidence {
    /// OID recorded by HEAD tree / index stage-0 (content-addressed fact).
    KnownObjectId { oid: ObjectHash },
    /// OID streamed from the worktree during this status/diff call.
    ComputedWorktreeThisCall { oid: ObjectHash },
    /// No trustworthy OID; exact pairing is forbidden.
    Unknown,
}

impl BlobEvidence {
    fn oid(&self) -> Option<&ObjectHash> {
        match self {
            BlobEvidence::KnownObjectId { oid }
            | BlobEvidence::ComputedWorktreeThisCall { oid } => Some(oid),
            BlobEvidence::Unknown => None,
        }
    }
}

/// One side of a rename candidate (§B.4.1 唯一定义).
#[derive(Debug, Clone)]
pub(crate) struct BlobRef {
    pub(crate) kind: BlobKind,
    pub(crate) mode: u32,
    pub(crate) size: Option<u64>,
    pub(crate) evidence: BlobEvidence,
}

/// Snapshot of both sides of a rename-detection run. For the staged side
/// `old = HEAD` / `new = index stage-0`; for the unstaged side `old = index
/// stage-0` / `new = worktree` (§B.4.1). Keys are repo-relative paths.
#[derive(Debug, Default)]
pub(crate) struct RenameSnapshot {
    pub(crate) old_map: HashMap<PathBuf, BlobRef>,
    pub(crate) new_map: HashMap<PathBuf, BlobRef>,
}

/// Engine knobs. `threshold` uses Git's 0..=60000 scale; `rename_limit == 0`
/// means "no per-side cap"; `comparison_budget == None` means unlimited
/// (diff's default per §B.7).
#[derive(Debug, Clone)]
pub(crate) struct RenameDetectConfig {
    pub(crate) threshold: u32,
    pub(crate) rename_limit: usize,
    pub(crate) comparison_budget: Option<u64>,
}

/// A matched rename pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenameMatch {
    pub(crate) old: PathBuf,
    pub(crate) new: PathBuf,
    pub(crate) exact: bool,
    /// Git-scale similarity (0..=60000). Exact pairs always carry 60000.
    pub(crate) internal_score: u32,
}

impl RenameMatch {
    /// Percentage (0..=100) as rendered by porcelain v2 / JSON. Git floors,
    /// except a non-exact 60000 caps at 100 anyway.
    pub(crate) fn score_percent(&self) -> u32 {
        (self.internal_score / 600).min(100)
    }
}

/// Why a candidate's content could not be scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SkipReason {
    ObjectMissing,
    ObjectCorrupt,
    ObjectUnavailable,
    TooLarge,
    BudgetExceeded,
    IoFailed,
}

/// Outcome of a content read for inexact scoring.
pub(crate) enum ContentOutcome {
    Content(Rc<Vec<u8>>),
    Skipped(SkipReason),
}

/// Caller-supplied content provider. Implementations own read budgets and
/// OID de-duplication; the engine caches spanhash entries per path, so each
/// path is requested at most once per run.
pub(crate) trait RenameContentSource {
    fn old_content(&mut self, path: &Path, blob: &BlobRef) -> ContentOutcome;
    fn new_content(&mut self, path: &Path, blob: &BlobRef) -> ContentOutcome;
}

/// Run statistics for warnings and tests (§B.4.2.6).
#[derive(Debug, Default, Clone)]
pub(crate) struct RenameDetectStats {
    pub(crate) comparisons: u64,
    /// Exhaustive stage skipped because a side exceeded `rename_limit`.
    pub(crate) skipped_by_limit: bool,
    /// Comparison budget hit: every exhaustive edge was discarded and only
    /// exact + basename pairs survive (§B.4.2.5 触顶规则).
    pub(crate) exhaustive_discarded: bool,
    /// Peak number of retained inexact edges (`≤ PER_DEST_TOP_K × dests`).
    pub(crate) peak_edges: usize,
    /// Per-reason counts of candidates dropped by content-read failures.
    pub(crate) content_skips: HashMap<SkipReason, u64>,
}

/// Engine output.
#[derive(Debug, Default)]
pub(crate) struct RenameDetectOutcome {
    /// Matched pairs, sorted by (old, new) path bytes.
    pub(crate) matches: Vec<RenameMatch>,
    pub(crate) stats: RenameDetectStats,
}

/// Per-path spanhash cache entry: `None` when content was skipped.
struct SpanhashSlot {
    entry: Option<Rc<SpanhashEntry>>,
}

pub(crate) struct SpanhashEntry {
    counts: HashMap<u64, u64>,
    len: u64,
}

/// Match renames between the two snapshot sides (§B.4.2 stage order).
pub(crate) fn match_pairs(
    snapshot: &RenameSnapshot,
    config: &RenameDetectConfig,
    source: &mut dyn RenameContentSource,
) -> RenameDetectOutcome {
    let mut stats = RenameDetectStats::default();
    let mut matches: Vec<RenameMatch> = Vec::new();

    // Deterministic path ordering: BTree keys compare as OsStr bytes.
    let mut remaining_old: BTreeMap<&PathBuf, &BlobRef> = snapshot.old_map.iter().collect();
    let mut remaining_new: BTreeMap<&PathBuf, &BlobRef> = snapshot.new_map.iter().collect();

    // ---- Stage 1: exact (bucket by oid + kind [+ gitlink mode]) ----------
    let mut buckets: HashMap<(ObjectHash, BlobKind, Option<u32>), BTreeSet<&PathBuf>> =
        HashMap::new();
    for (path, blob) in &remaining_new {
        if let Some(oid) = blob.evidence.oid() {
            let mode_key = (blob.kind == BlobKind::Gitlink).then_some(blob.mode);
            buckets
                .entry((*oid, blob.kind, mode_key))
                .or_default()
                .insert(path);
        }
    }

    let old_paths: Vec<&PathBuf> = remaining_old.keys().copied().collect();
    for old_path in old_paths {
        let old_blob = remaining_old[&old_path];
        let Some(oid) = old_blob.evidence.oid() else {
            continue;
        };
        let mode_key = (old_blob.kind == BlobKind::Gitlink).then_some(old_blob.mode);
        let Some(candidates) = buckets.get_mut(&(*oid, old_blob.kind, mode_key)) else {
            continue;
        };
        if candidates.is_empty() {
            continue;
        }
        // Prefer a same-basename destination; fall back to the first (path
        // byte order) candidate — deterministic one-to-one consumption.
        let same_basename = candidates
            .iter()
            .find(|p| p.file_name() == old_path.file_name())
            .copied();
        let picked: &PathBuf = match same_basename.or_else(|| candidates.first().copied()) {
            Some(picked) => picked,
            None => continue,
        };
        candidates.remove(picked);
        remaining_new.remove(picked);
        remaining_old.remove(old_path);
        matches.push(RenameMatch {
            old: old_path.clone(),
            new: picked.clone(),
            exact: true,
            internal_score: EXACT_SCORE,
        });
    }

    // ---- Stage 2: `-M100%` stops after exact ------------------------------
    if config.threshold >= EXACT_SCORE {
        matches.sort_by(|a, b| a.old.cmp(&b.old).then_with(|| a.new.cmp(&b.new)));
        return RenameDetectOutcome { matches, stats };
    }

    // Spanhash caches;每 path 至多读取一次内容。
    let mut old_hashes: HashMap<PathBuf, SpanhashSlot> = HashMap::new();
    let mut new_hashes: HashMap<PathBuf, SpanhashSlot> = HashMap::new();
    let mut budget_exhausted = false;

    // ---- Stage 3: unique-basename pairing (always runs, §B.4.2.3) --------
    let mut old_by_basename: BTreeMap<OsString, Vec<&PathBuf>> = BTreeMap::new();
    for path in remaining_old.keys() {
        if let Some(name) = path.file_name() {
            old_by_basename
                .entry(name.to_os_string())
                .or_default()
                .push(path);
        }
    }
    let mut new_by_basename: BTreeMap<OsString, Vec<&PathBuf>> = BTreeMap::new();
    for path in remaining_new.keys() {
        if let Some(name) = path.file_name() {
            new_by_basename
                .entry(name.to_os_string())
                .or_default()
                .push(path);
        }
    }

    let mut basename_pairs: Vec<(PathBuf, PathBuf, u32)> = Vec::new();
    for (name, olds) in &old_by_basename {
        if olds.len() != 1 {
            continue;
        }
        let Some(news) = new_by_basename.get(name) else {
            continue;
        };
        if news.len() != 1 {
            continue;
        }
        let (old_path, new_path) = (olds[0], news[0]);
        let old_blob = remaining_old[old_path];
        let new_blob = remaining_new[new_path];
        if !inexact_eligible(old_blob) || !inexact_eligible(new_blob) {
            continue;
        }
        // The basename stage always runs (§B.4.2.3): its comparisons count
        // toward the budget diagnostics but are never gated by it — only the
        // exhaustive stage is discarded on exhaustion (§B.4.2.5 触顶规则).
        stats.comparisons += 1;
        let Some(old_entry) = spanhash_for(
            &mut old_hashes,
            old_path,
            old_blob,
            Side::Old,
            source,
            &mut stats,
        ) else {
            continue;
        };
        let Some(new_entry) = spanhash_for(
            &mut new_hashes,
            new_path,
            new_blob,
            Side::New,
            source,
            &mut stats,
        ) else {
            continue;
        };
        let score = similarity_from_entries(&old_entry, &new_entry);
        if score >= config.threshold {
            basename_pairs.push(((*old_path).clone(), (*new_path).clone(), score));
        }
    }
    for (old, new, score) in basename_pairs {
        remaining_old.remove(&old);
        remaining_new.remove(&new);
        matches.push(RenameMatch {
            old,
            new,
            exact: false,
            internal_score: score,
        });
    }

    // ---- Stage 4: renameLimit gate (per-side OR, §B.7) --------------------
    let sources = remaining_old.len();
    let destinations = remaining_new.len();
    let limit_skips = config.rename_limit > 0
        && (sources > config.rename_limit || destinations > config.rename_limit);
    if limit_skips {
        stats.skipped_by_limit = true;
    }

    // ---- Stage 5: bounded exhaustive inexact ------------------------------
    if !limit_skips && !budget_exhausted {
        // Edge ordering (§B.4.2.5): score desc, same-basename first, then
        // old/new path bytes ascending. `Reverse`-free: encode as a sortable
        // tuple with inverted score.
        #[derive(PartialEq, Eq, PartialOrd, Ord)]
        struct EdgeKey(u32, bool, PathBuf, PathBuf); // (60000-score, !same_basename, old, new)

        let mut per_dest: BTreeMap<&PathBuf, Vec<(EdgeKey, u32)>> = BTreeMap::new();
        'outer: for (new_path, new_blob) in &remaining_new {
            if !inexact_eligible(new_blob) {
                continue;
            }
            let Some(new_entry) = spanhash_for(
                &mut new_hashes,
                new_path,
                new_blob,
                Side::New,
                source,
                &mut stats,
            ) else {
                continue;
            };
            for (old_path, old_blob) in &remaining_old {
                if !inexact_eligible(old_blob) {
                    continue;
                }
                if !consume_comparison(config, &mut stats, &mut budget_exhausted) {
                    break 'outer;
                }
                let Some(old_entry) = spanhash_for(
                    &mut old_hashes,
                    old_path,
                    old_blob,
                    Side::Old,
                    source,
                    &mut stats,
                ) else {
                    continue;
                };
                let score = similarity_from_entries(&old_entry, &new_entry);
                if score < config.threshold {
                    continue;
                }
                let same_basename = old_path.file_name() == new_path.file_name();
                let key = EdgeKey(
                    EXACT_SCORE - score,
                    !same_basename,
                    (*old_path).clone(),
                    (*new_path).clone(),
                );
                let edges = per_dest.entry(new_path).or_default();
                edges.push((key, score));
                edges.sort();
                if edges.len() > PER_DEST_TOP_K {
                    edges.truncate(PER_DEST_TOP_K);
                }
            }
        }

        if budget_exhausted {
            // 触顶规则：丢弃整个 exhaustive 阶段已评结果。
            stats.exhaustive_discarded = true;
        } else {
            let mut all_edges: Vec<(EdgeKey, u32)> = Vec::new();
            for (_, edges) in per_dest {
                all_edges.extend(edges);
            }
            stats.peak_edges = all_edges.len();
            all_edges.sort_by(|a, b| a.0.cmp(&b.0));
            for (EdgeKey(_, _, old, new), score) in all_edges {
                if remaining_old.contains_key(&old) && remaining_new.contains_key(&new) {
                    remaining_old.remove(&old);
                    remaining_new.remove(&new);
                    matches.push(RenameMatch {
                        old,
                        new,
                        exact: false,
                        internal_score: score,
                    });
                }
            }
        }
    }

    matches.sort_by(|a, b| a.old.cmp(&b.old).then_with(|| a.new.cmp(&b.new)));
    RenameDetectOutcome { matches, stats }
}

enum Side {
    Old,
    New,
}

/// Inexact scoring is Regular↔Regular only, and skips empty files (§B.4.1).
fn inexact_eligible(blob: &BlobRef) -> bool {
    blob.kind == BlobKind::Regular && blob.size != Some(0)
}

/// Consume one comparison from the budget. Returns false (and flags
/// exhaustion) when the budget is already spent.
fn consume_comparison(
    config: &RenameDetectConfig,
    stats: &mut RenameDetectStats,
    exhausted: &mut bool,
) -> bool {
    if let Some(budget) = config.comparison_budget
        && stats.comparisons >= budget
    {
        *exhausted = true;
        return false;
    }
    stats.comparisons += 1;
    true
}

fn spanhash_for(
    cache: &mut HashMap<PathBuf, SpanhashSlot>,
    path: &PathBuf,
    blob: &BlobRef,
    side: Side,
    source: &mut dyn RenameContentSource,
    stats: &mut RenameDetectStats,
) -> Option<Rc<SpanhashEntry>> {
    if let Some(slot) = cache.get(path) {
        return slot.entry.clone();
    }
    let outcome = match side {
        Side::Old => source.old_content(path, blob),
        Side::New => source.new_content(path, blob),
    };
    let entry = match outcome {
        ContentOutcome::Content(bytes) => Some(Rc::new(spanhash_entry(&bytes))),
        ContentOutcome::Skipped(reason) => {
            *stats.content_skips.entry(reason).or_default() += 1;
            None
        }
    };
    cache.insert(
        path.clone(),
        SpanhashSlot {
            entry: entry.clone(),
        },
    );
    entry
}

// ---------------------------------------------------------------------------
// Scoring (moved verbatim in semantics from `diff.rs`; see §B.4.2)
// ---------------------------------------------------------------------------

/// Chunk `data` the way Git's rename spanhash does — a chunk ends at a newline
/// or after 64 bytes; a `\r` in a `\r\n` is ignored for text — and accumulate
/// the byte count per chunk-hash. We hash each chunk with FNV-1a rather than
/// Git's weaker `HASHBASE` rolling hash: for real content the similarity is
/// identical (equal chunks always match; FNV collisions are astronomically
/// rare), but a contrived input engineered to collide under Git's hash can
/// score differently.
pub(crate) fn spanhash_counts(data: &[u8]) -> HashMap<u64, u64> {
    let is_text = !data.contains(&0);
    let mut counts: HashMap<u64, u64> = HashMap::new();
    let mut chunk: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let c = data[i];
        if is_text && c == b'\r' && i + 1 < data.len() && data[i + 1] == b'\n' {
            i += 1;
            continue;
        }
        chunk.push(c);
        i += 1;
        if chunk.len() >= 64 || c == b'\n' {
            *counts.entry(fnv1a(&chunk)).or_default() += chunk.len() as u64;
            chunk.clear();
        }
    }
    if !chunk.is_empty() {
        *counts.entry(fnv1a(&chunk)).or_default() += chunk.len() as u64;
    }
    counts
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub(crate) fn spanhash_entry(data: &[u8]) -> SpanhashEntry {
    SpanhashEntry {
        counts: spanhash_counts(data),
        len: data.len() as u64,
    }
}

/// Git's similarity score (0..60000) from precomputed spanhash entries.
pub(crate) fn similarity_from_entries(old: &SpanhashEntry, new: &SpanhashEntry) -> u32 {
    let max_size = old.len.max(new.len);
    if max_size == 0 {
        return EXACT_SCORE;
    }
    let mut common: u64 = 0;
    for (hash, &old_bytes) in &old.counts {
        if let Some(&new_bytes) = new.counts.get(hash) {
            common += old_bytes.min(new_bytes);
        }
    }
    ((common * u64::from(EXACT_SCORE)) / max_size) as u32
}

/// Git's similarity score (0..60000): common chunk bytes * 60000 / max file
/// size. Two empty files are identical (full score). The displayed percent is
/// `score / 600`.
pub(crate) fn similarity_score(old: &[u8], new: &[u8]) -> u32 {
    similarity_from_entries(&spanhash_entry(old), &spanhash_entry(new))
}

// ---------------------------------------------------------------------------
// Bounded content readers (§B.3.4 预算表)
// ---------------------------------------------------------------------------

/// Budgeted, OID-deduplicated repository blob reader for inexact scoring.
/// Known-OID exact pairing never reads objects; only candidates that need
/// content pass through here. Failures map to [`SkipReason`]s so a missing or
/// corrupt object degrades to "skip this inexact candidate" (§B.4.1) instead
/// of failing base status.
pub(crate) struct ObjectReadBudget {
    per_object_cap: u64,
    remaining_total: u64,
    remaining_objects: u32,
    deadline: Instant,
    cache: HashMap<ObjectHash, ContentSlot>,
}

enum ContentSlot {
    Content(Rc<Vec<u8>>),
    Skipped(SkipReason),
}

impl ObjectReadBudget {
    pub(crate) fn with_defaults() -> Self {
        Self::new(
            OBJECT_READ_MAX_OBJECT_BYTES,
            OBJECT_READ_MAX_TOTAL_BYTES,
            OBJECT_READ_MAX_OBJECTS,
            OBJECT_READ_DEADLINE,
        )
    }

    pub(crate) fn new(
        per_object_cap: u64,
        max_total_bytes: u64,
        max_objects: u32,
        deadline: Duration,
    ) -> Self {
        Self {
            per_object_cap,
            remaining_total: max_total_bytes,
            remaining_objects: max_objects,
            deadline: Instant::now() + deadline,
            cache: HashMap::new(),
        }
    }

    /// Read a blob's content under budget, de-duplicating by OID: repeated
    /// requests for the same object return the cached outcome without
    /// consuming budget again.
    pub(crate) fn read_blob(&mut self, oid: &ObjectHash) -> ContentOutcome {
        if let Some(slot) = self.cache.get(oid) {
            return match slot {
                ContentSlot::Content(bytes) => ContentOutcome::Content(bytes.clone()),
                ContentSlot::Skipped(reason) => ContentOutcome::Skipped(*reason),
            };
        }
        // Budget-exceeded outcomes are NOT cached per-OID: they describe the
        // batch, not the object, and later slices may retry with fresh budgets.
        if Instant::now() >= self.deadline
            || self.remaining_objects == 0
            || self.remaining_total == 0
        {
            return ContentOutcome::Skipped(SkipReason::BudgetExceeded);
        }

        // Reserve the object slot up front: failed lookups (missing, corrupt,
        // unavailable, too large) must also consume the 64-object cap, or a
        // pathological candidate set could hammer the store until the
        // wall-clock deadline.
        self.remaining_objects -= 1;
        let cap = self.per_object_cap.min(self.remaining_total);
        // Keep replace-peel semantics consistent with `load_object` (diff and
        // show read through the same substitution).
        let peeled = super::replace::resolve(*oid);
        let storage = crate::utils::util::objects_storage();
        let outcome = match storage.get_with_limit(&peeled, cap) {
            Ok(bytes) => {
                self.remaining_total = self.remaining_total.saturating_sub(bytes.len() as u64);
                ContentSlot::Content(Rc::new(bytes))
            }
            Err(err) => ContentSlot::Skipped(classify_object_error(&err)),
        };
        let result = match &outcome {
            ContentSlot::Content(bytes) => ContentOutcome::Content(bytes.clone()),
            ContentSlot::Skipped(reason) => ContentOutcome::Skipped(*reason),
        };
        self.cache.insert(*oid, outcome);
        result
    }
}

/// Map storage errors onto skip reasons (§B.4.1: Missing/Corrupt/Unavailable/
/// BudgetExceeded 只跳过依赖该对象的 inexact 候选). Classification is owned by
/// the storage layer so the message details and the mapping evolve together.
fn classify_object_error(err: &GitError) -> SkipReason {
    use crate::utils::client_storage::{ClientStorage, ObjectReadFailure};
    match ClientStorage::classify_read_failure(err) {
        ObjectReadFailure::Missing => SkipReason::ObjectMissing,
        ObjectReadFailure::Corrupt => SkipReason::ObjectCorrupt,
        ObjectReadFailure::Unavailable => SkipReason::ObjectUnavailable,
        ObjectReadFailure::TooLarge => SkipReason::TooLarge,
        ObjectReadFailure::Other => SkipReason::IoFailed,
    }
}

/// Budgeted worktree reader: bounded per-file and total bytes, task count and
/// a batch deadline. Every path costs a task (even empty files, §B.3.4).
pub(crate) struct WorktreeReadBudget {
    per_file_cap: u64,
    remaining_total: u64,
    remaining_tasks: u32,
    deadline: Instant,
}

impl WorktreeReadBudget {
    pub(crate) fn with_defaults() -> Self {
        Self::new(
            WORKTREE_READ_MAX_FILE_BYTES,
            WORKTREE_READ_MAX_TOTAL_BYTES,
            WORKTREE_READ_MAX_TASKS,
            WORKTREE_READ_DEADLINE,
        )
    }

    pub(crate) fn new(
        per_file_cap: u64,
        max_total_bytes: u64,
        max_tasks: u32,
        deadline: Duration,
    ) -> Self {
        Self {
            per_file_cap,
            remaining_total: max_total_bytes,
            remaining_tasks: max_tasks,
            deadline: Instant::now() + deadline,
        }
    }

    fn take_task(&mut self) -> bool {
        if Instant::now() >= self.deadline || self.remaining_tasks == 0 {
            return false;
        }
        self.remaining_tasks -= 1;
        true
    }

    /// Read the blob bytes Git would store for a worktree path (regular file
    /// content, LFS pointer, or symlink target bytes) under budget.
    pub(crate) fn read_worktree_blob(&mut self, abs_path: &Path) -> ContentOutcome {
        if !self.take_task() {
            return ContentOutcome::Skipped(SkipReason::BudgetExceeded);
        }
        let metadata = match std::fs::symlink_metadata(abs_path) {
            Ok(metadata) => metadata,
            Err(_) => return ContentOutcome::Skipped(SkipReason::IoFailed),
        };
        if metadata.file_type().is_symlink() {
            return match super::read_symlink_blob_bytes(abs_path) {
                Ok(bytes) => self.account(bytes),
                Err(_) => ContentOutcome::Skipped(SkipReason::IoFailed),
            };
        }
        if crate::utils::lfs::is_lfs_tracked(abs_path) {
            // The budget covers hydrated bytes, not pointer length (§B.9
            // lfs_budget_counts_hydrated_bytes): charge the on-disk size.
            if metadata.len() > self.per_file_cap.min(self.remaining_total) {
                return ContentOutcome::Skipped(SkipReason::TooLarge);
            }
            return match crate::utils::lfs::generate_pointer_file_result(abs_path) {
                Ok((pointer, _)) => {
                    self.remaining_total = self.remaining_total.saturating_sub(metadata.len());
                    ContentOutcome::Content(Rc::new(pointer.into_bytes()))
                }
                Err(_) => ContentOutcome::Skipped(SkipReason::IoFailed),
            };
        }
        if metadata.len() > self.per_file_cap.min(self.remaining_total) {
            return ContentOutcome::Skipped(SkipReason::TooLarge);
        }
        match std::fs::read(abs_path) {
            Ok(bytes) => self.account(bytes),
            Err(_) => ContentOutcome::Skipped(SkipReason::IoFailed),
        }
    }

    fn account(&mut self, bytes: Vec<u8>) -> ContentOutcome {
        let len = bytes.len() as u64;
        if len > self.per_file_cap.min(self.remaining_total) {
            return ContentOutcome::Skipped(SkipReason::TooLarge);
        }
        self.remaining_total = self.remaining_total.saturating_sub(len);
        ContentOutcome::Content(Rc::new(bytes))
    }

    /// Stream a worktree path's Git blob OID and byte size under budget
    /// (§B.4.1 `worktree_blob_oid_and_size`). Costs one task; the streamed
    /// bytes are charged against the total budget but never buffered.
    pub(crate) fn worktree_blob_oid_and_size(
        &mut self,
        abs_path: &Path,
    ) -> Result<(ObjectHash, u64), SkipReason> {
        if !self.take_task() {
            return Err(SkipReason::BudgetExceeded);
        }
        let metadata = std::fs::symlink_metadata(abs_path).map_err(|_| SkipReason::IoFailed)?;
        if metadata.file_type().is_symlink() {
            let bytes =
                super::read_symlink_blob_bytes(abs_path).map_err(|_| SkipReason::IoFailed)?;
            let len = bytes.len() as u64;
            if len > self.per_file_cap.min(self.remaining_total) {
                return Err(SkipReason::TooLarge);
            }
            self.remaining_total = self.remaining_total.saturating_sub(len);
            let oid = git_internal::internal::object::blob::Blob::from_content_bytes(bytes).id;
            return Ok((oid, len));
        }
        if metadata.len() > self.per_file_cap.min(self.remaining_total) {
            return Err(SkipReason::TooLarge);
        }
        if crate::utils::lfs::is_lfs_tracked(abs_path) {
            let (pointer, _) = crate::utils::lfs::generate_pointer_file_result(abs_path)
                .map_err(|_| SkipReason::IoFailed)?;
            self.remaining_total = self.remaining_total.saturating_sub(metadata.len());
            let len = pointer.len() as u64;
            let oid = git_internal::internal::object::blob::Blob::from_content(&pointer).id;
            return Ok((oid, len));
        }
        let oid = super::stream_file_blob_hash(abs_path).map_err(|_| SkipReason::IoFailed)?;
        self.remaining_total = self.remaining_total.saturating_sub(metadata.len());
        Ok((oid, metadata.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regular(evidence: BlobEvidence, size: u64) -> BlobRef {
        BlobRef {
            kind: BlobKind::Regular,
            mode: 0o100644,
            size: Some(size),
            evidence,
        }
    }

    fn oid_of(data: &[u8]) -> ObjectHash {
        git_internal::internal::object::blob::Blob::from_content_bytes(data.to_vec()).id
    }

    fn known(data: &[u8]) -> BlobEvidence {
        BlobEvidence::KnownObjectId { oid: oid_of(data) }
    }

    /// Test content source backed by in-memory maps; counts reads.
    struct MapSource {
        old: HashMap<PathBuf, Vec<u8>>,
        new: HashMap<PathBuf, Vec<u8>>,
        reads: u64,
    }

    impl MapSource {
        fn new() -> Self {
            Self {
                old: HashMap::new(),
                new: HashMap::new(),
                reads: 0,
            }
        }
    }

    impl RenameContentSource for MapSource {
        fn old_content(&mut self, path: &Path, _blob: &BlobRef) -> ContentOutcome {
            self.reads += 1;
            match self.old.get(path) {
                Some(bytes) => ContentOutcome::Content(Rc::new(bytes.clone())),
                None => ContentOutcome::Skipped(SkipReason::ObjectMissing),
            }
        }

        fn new_content(&mut self, path: &Path, _blob: &BlobRef) -> ContentOutcome {
            self.reads += 1;
            match self.new.get(path) {
                Some(bytes) => ContentOutcome::Content(Rc::new(bytes.clone())),
                None => ContentOutcome::Skipped(SkipReason::ObjectMissing),
            }
        }
    }

    fn config(threshold: u32) -> RenameDetectConfig {
        RenameDetectConfig {
            threshold,
            rename_limit: 1000,
            comparison_budget: None,
        }
    }

    fn snapshot(old: &[(&str, BlobRef)], new: &[(&str, BlobRef)]) -> RenameSnapshot {
        RenameSnapshot {
            old_map: old
                .iter()
                .map(|(p, b)| (PathBuf::from(p), b.clone()))
                .collect(),
            new_map: new
                .iter()
                .map(|(p, b)| (PathBuf::from(p), b.clone()))
                .collect(),
        }
    }

    #[test]
    fn exact_pairs_without_content_reads() {
        let data = b"identical content\n";
        let snap = snapshot(
            &[("a.txt", regular(known(data), data.len() as u64))],
            &[("b.txt", regular(known(data), data.len() as u64))],
        );
        let mut source = MapSource::new();
        let outcome = match_pairs(&snap, &config(30000), &mut source);
        assert_eq!(outcome.matches.len(), 1);
        assert!(outcome.matches[0].exact);
        assert_eq!(outcome.matches[0].internal_score, EXACT_SCORE);
        assert_eq!(source.reads, 0, "exact pairing must not read content");
    }

    #[test]
    fn exact_bucket_prefers_same_basename() {
        let data = b"same bytes\n";
        let blob = || regular(known(data), data.len() as u64);
        let snap = snapshot(
            &[("dir/a.txt", blob())],
            &[("z/a.txt", blob()), ("aaa/first.txt", blob())],
        );
        let mut source = MapSource::new();
        let outcome = match_pairs(&snap, &config(30000), &mut source);
        // Same-basename destination wins even though "aaa/first.txt" sorts first.
        assert_eq!(outcome.matches[0].new, PathBuf::from("z/a.txt"));
    }

    #[test]
    fn exact_duplicate_hash_order_stable() {
        let data = b"dup\n";
        let blob = || regular(known(data), data.len() as u64);
        // Two identical sources, two identical destinations — pairing must be
        // deterministic by path byte order regardless of map iteration.
        let snap = snapshot(
            &[("b_old", blob()), ("a_old", blob())],
            &[("d_new", blob()), ("c_new", blob())],
        );
        let mut source = MapSource::new();
        let outcome = match_pairs(&snap, &config(30000), &mut source);
        assert_eq!(outcome.matches.len(), 2);
        assert_eq!(outcome.matches[0].old, PathBuf::from("a_old"));
        assert_eq!(outcome.matches[0].new, PathBuf::from("c_new"));
        assert_eq!(outcome.matches[1].old, PathBuf::from("b_old"));
        assert_eq!(outcome.matches[1].new, PathBuf::from("d_new"));
    }

    #[test]
    fn unknown_evidence_never_pairs_exact() {
        let data = b"content\n";
        let snap = snapshot(
            &[("a.txt", regular(BlobEvidence::Unknown, data.len() as u64))],
            &[("b.txt", regular(known(data), data.len() as u64))],
        );
        let mut source = MapSource::new();
        // Threshold 60000: only exact stage runs; Unknown must not pair.
        let outcome = match_pairs(&snap, &config(EXACT_SCORE), &mut source);
        assert!(outcome.matches.is_empty());
    }

    #[test]
    fn gitlink_exact_requires_same_mode() {
        let oid = oid_of(b"submodule");
        let gitlink = |mode: u32| BlobRef {
            kind: BlobKind::Gitlink,
            mode,
            size: None,
            evidence: BlobEvidence::KnownObjectId { oid },
        };
        let snap = snapshot(
            &[("sub_a", gitlink(0o160000))],
            &[("sub_b", gitlink(0o160001))],
        );
        let mut source = MapSource::new();
        let outcome = match_pairs(&snap, &config(30000), &mut source);
        assert!(
            outcome.matches.is_empty(),
            "gitlink mode mismatch must not pair"
        );
    }

    #[test]
    fn cross_kind_never_pairs() {
        let data = b"target";
        let symlink = BlobRef {
            kind: BlobKind::Symlink,
            mode: 0o120000,
            size: Some(data.len() as u64),
            evidence: known(data),
        };
        let snap = snapshot(
            &[("link", symlink)],
            &[("file", regular(known(data), data.len() as u64))],
        );
        let mut source = MapSource::new();
        let outcome = match_pairs(&snap, &config(30000), &mut source);
        assert!(outcome.matches.is_empty());
    }

    #[test]
    fn empty_files_pair_exact_but_not_inexact() {
        let empty = b"";
        let exact_snap = snapshot(
            &[("a", regular(known(empty), 0))],
            &[("b", regular(known(empty), 0))],
        );
        let mut source = MapSource::new();
        let outcome = match_pairs(&exact_snap, &config(30000), &mut source);
        assert_eq!(outcome.matches.len(), 1, "empty Known-OID exact must pair");

        // Different evidence (no shared OID): empty files must not inexact-pair.
        let inexact_snap = snapshot(
            &[("a", regular(BlobEvidence::Unknown, 0))],
            &[("b", regular(BlobEvidence::Unknown, 0))],
        );
        let mut source = MapSource::new();
        source.old.insert(PathBuf::from("a"), Vec::new());
        source.new.insert(PathBuf::from("b"), Vec::new());
        let outcome = match_pairs(&inexact_snap, &config(30000), &mut source);
        assert!(outcome.matches.is_empty(), "size==0 skips inexact");
    }

    #[test]
    fn basename_unique_pairs_before_exhaustive() {
        let old_bytes = b"one\ntwo\nthree\nfour\n".to_vec();
        let new_bytes = b"one\ntwo\nchanged\nfour\n".to_vec();
        let snap = snapshot(
            &[(
                "src/util.rs",
                regular(BlobEvidence::Unknown, old_bytes.len() as u64),
            )],
            &[(
                "lib/util.rs",
                regular(BlobEvidence::Unknown, new_bytes.len() as u64),
            )],
        );
        let mut source = MapSource::new();
        source.old.insert(PathBuf::from("src/util.rs"), old_bytes);
        source.new.insert(PathBuf::from("lib/util.rs"), new_bytes);
        let outcome = match_pairs(&snap, &config(30000), &mut source);
        assert_eq!(outcome.matches.len(), 1);
        assert!(!outcome.matches[0].exact);
        assert!(outcome.matches[0].internal_score >= 30000);
    }

    #[test]
    fn duplicate_basenames_skip_basename_stage() {
        // Two sources share a basename → basename stage must not pair them;
        // exhaustive resolves by score.
        let a = b"alpha alpha alpha alpha\n".to_vec();
        let b = b"beta beta beta beta beta\n".to_vec();
        let snap = snapshot(
            &[
                ("x/f.txt", regular(BlobEvidence::Unknown, a.len() as u64)),
                ("y/f.txt", regular(BlobEvidence::Unknown, b.len() as u64)),
            ],
            &[("z/f.txt", regular(BlobEvidence::Unknown, b.len() as u64))],
        );
        let mut source = MapSource::new();
        source.old.insert(PathBuf::from("x/f.txt"), a);
        source.old.insert(PathBuf::from("y/f.txt"), b.clone());
        source.new.insert(PathBuf::from("z/f.txt"), b);
        let outcome = match_pairs(&snap, &config(30000), &mut source);
        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(outcome.matches[0].old, PathBuf::from("y/f.txt"));
    }

    #[test]
    fn exhaustive_greedy_two_by_two_matrix() {
        // Score matrix (approx): old1 is near-identical to new1; old2 to new2.
        // Cross scores are below both. Greedy must take the diagonal.
        let o1 = b"aaaa\nbbbb\ncccc\ndddd\n".to_vec();
        let n1 = b"aaaa\nbbbb\ncccc\nxxxx\n".to_vec();
        let o2 = b"1111\n2222\n3333\n4444\n".to_vec();
        let n2 = b"1111\n2222\n3333\nyyyy\n".to_vec();
        let snap = snapshot(
            &[
                ("old1", regular(BlobEvidence::Unknown, o1.len() as u64)),
                ("old2", regular(BlobEvidence::Unknown, o2.len() as u64)),
            ],
            &[
                ("new1", regular(BlobEvidence::Unknown, n1.len() as u64)),
                ("new2", regular(BlobEvidence::Unknown, n2.len() as u64)),
            ],
        );
        let mut source = MapSource::new();
        source.old.insert(PathBuf::from("old1"), o1);
        source.old.insert(PathBuf::from("old2"), o2);
        source.new.insert(PathBuf::from("new1"), n1);
        source.new.insert(PathBuf::from("new2"), n2);
        let outcome = match_pairs(&snap, &config(30000), &mut source);
        assert_eq!(outcome.matches.len(), 2);
        let by_old: HashMap<_, _> = outcome
            .matches
            .iter()
            .map(|m| (m.old.clone(), m.new.clone()))
            .collect();
        assert_eq!(by_old[&PathBuf::from("old1")], PathBuf::from("new1"));
        assert_eq!(by_old[&PathBuf::from("old2")], PathBuf::from("new2"));
    }

    #[test]
    fn rename_limit_per_side_gates_exhaustive_only() {
        let content = |i: usize| format!("unique line {i}\nshared shared shared\n").into_bytes();
        let mut old = Vec::new();
        let mut source = MapSource::new();
        for i in 0..3 {
            let path = format!("old{i}");
            let bytes = content(i);
            source.old.insert(PathBuf::from(&path), bytes.clone());
            old.push((path, bytes.len() as u64));
        }
        let new_bytes = content(0);
        source
            .new
            .insert(PathBuf::from("moved0"), new_bytes.clone());
        let snap = RenameSnapshot {
            old_map: old
                .iter()
                .map(|(p, len)| (PathBuf::from(p), regular(BlobEvidence::Unknown, *len)))
                .collect(),
            new_map: [(
                PathBuf::from("moved0"),
                regular(BlobEvidence::Unknown, new_bytes.len() as u64),
            )]
            .into(),
        };
        // limit=2 < sources=3 → exhaustive skipped entirely.
        let cfg = RenameDetectConfig {
            threshold: 30000,
            rename_limit: 2,
            comparison_budget: None,
        };
        let outcome = match_pairs(&snap, &cfg, &mut source);
        assert!(outcome.stats.skipped_by_limit);
        assert!(
            outcome.matches.is_empty(),
            "unique basenames differ; nothing pairs"
        );

        // limit=0 → uncapped; exhaustive runs and pairs old0→moved0.
        let cfg = RenameDetectConfig {
            threshold: 30000,
            rename_limit: 0,
            comparison_budget: None,
        };
        let mut source2 = MapSource::new();
        source2.old = source.old.clone();
        source2.new = source.new.clone();
        let outcome = match_pairs(&snap, &cfg, &mut source2);
        assert!(!outcome.stats.skipped_by_limit);
        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(outcome.matches[0].old, PathBuf::from("old0"));
    }

    #[test]
    fn comparison_budget_discards_exhaustive_keeps_exact_and_basename() {
        let exact_data = b"exact\n";
        let base_old = b"one\ntwo\nthree\nfour\n".to_vec();
        let base_new = b"one\ntwo\nthree\nchanged\n".to_vec();
        let filler = |i: usize| format!("filler {i} {i} {i} {i}\n").into_bytes();
        let mut snap = snapshot(
            &[
                (
                    "exact_old",
                    regular(known(exact_data), exact_data.len() as u64),
                ),
                (
                    "base/same.rs",
                    regular(BlobEvidence::Unknown, base_old.len() as u64),
                ),
            ],
            &[
                (
                    "exact_new",
                    regular(known(exact_data), exact_data.len() as u64),
                ),
                (
                    "moved/same.rs",
                    regular(BlobEvidence::Unknown, base_new.len() as u64),
                ),
            ],
        );
        let mut source = MapSource::new();
        source.old.insert(PathBuf::from("base/same.rs"), base_old);
        source.new.insert(PathBuf::from("moved/same.rs"), base_new);
        for i in 0..4 {
            let old_path = PathBuf::from(format!("bulk/old{i}"));
            let new_path = PathBuf::from(format!("bulk/new{i}"));
            let bytes = filler(i);
            snap.old_map.insert(
                old_path.clone(),
                regular(BlobEvidence::Unknown, bytes.len() as u64),
            );
            snap.new_map.insert(
                new_path.clone(),
                regular(BlobEvidence::Unknown, bytes.len() as u64),
            );
            source.old.insert(old_path, bytes.clone());
            source.new.insert(new_path, bytes);
        }
        // Budget = 1: the single basename comparison consumes it; the
        // exhaustive stage over bulk/* then hits the ceiling immediately and
        // must discard all its scored edges.
        let cfg = RenameDetectConfig {
            threshold: 30000,
            rename_limit: 0,
            comparison_budget: Some(1),
        };
        let outcome = match_pairs(&snap, &cfg, &mut source);
        assert!(outcome.stats.exhaustive_discarded);
        let kinds: Vec<_> = outcome.matches.iter().map(|m| m.exact).collect();
        assert_eq!(
            outcome.matches.len(),
            2,
            "exact + basename survive: {kinds:?}"
        );
        assert!(outcome.matches.iter().any(|m| m.exact));
        assert!(
            outcome
                .matches
                .iter()
                .any(|m| !m.exact && m.new == Path::new("moved/same.rs"))
        );
        assert!(
            !outcome.matches.iter().any(|m| m.new.starts_with("bulk")),
            "exhaustive results must be discarded on budget exhaustion"
        );
    }

    #[test]
    fn top_k_keeps_highest_scoring_old() {
        // Five sources target one destination; top-K=4 must keep the best
        // scorer even when it arrives last in path order.
        let dest = b"alpha\nbravo\ncharlie\ndelta\necho\n".to_vec();
        let near = b"alpha\nbravo\ncharlie\ndelta\nfoxtrot\n".to_vec();
        let far = |i: usize| format!("alpha\n{i} {i} {i}\n{i}{i}\nx\ny\n").into_bytes();
        let mut source = MapSource::new();
        let mut old_map = HashMap::new();
        for i in 0..4 {
            let p = PathBuf::from(format!("a{i}"));
            let bytes = far(i);
            old_map.insert(
                p.clone(),
                regular(BlobEvidence::Unknown, bytes.len() as u64),
            );
            source.old.insert(p, bytes);
        }
        // "z_best" sorts after the four fillers but scores highest.
        let best = PathBuf::from("z_best");
        old_map.insert(
            best.clone(),
            regular(BlobEvidence::Unknown, near.len() as u64),
        );
        source.old.insert(best.clone(), near);
        let snap = RenameSnapshot {
            old_map,
            new_map: [(
                PathBuf::from("dest"),
                regular(BlobEvidence::Unknown, dest.len() as u64),
            )]
            .into(),
        };
        source.new.insert(PathBuf::from("dest"), dest);
        let cfg = RenameDetectConfig {
            threshold: 6000, // 10%: keep the low-score fillers as edges too
            rename_limit: 0,
            comparison_budget: None,
        };
        let outcome = match_pairs(&snap, &cfg, &mut source);
        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(
            outcome.matches[0].old, best,
            "5th edge must replace a worse one"
        );
        assert!(outcome.stats.peak_edges <= PER_DEST_TOP_K);
    }

    #[test]
    fn content_skip_only_drops_affected_candidate() {
        let ok_old = b"one\ntwo\nthree\nfour\n".to_vec();
        let ok_new = b"one\ntwo\nthree\nfive\n".to_vec();
        let snap = snapshot(
            &[
                (
                    "ok/x.rs",
                    regular(BlobEvidence::Unknown, ok_old.len() as u64),
                ),
                ("broken/y.rs", regular(BlobEvidence::Unknown, 10)),
            ],
            &[
                (
                    "moved/x.rs",
                    regular(BlobEvidence::Unknown, ok_new.len() as u64),
                ),
                ("moved/y.rs", regular(BlobEvidence::Unknown, 10)),
            ],
        );
        let mut source = MapSource::new();
        source.old.insert(PathBuf::from("ok/x.rs"), ok_old);
        source.new.insert(PathBuf::from("moved/x.rs"), ok_new);
        source
            .new
            .insert(PathBuf::from("moved/y.rs"), b"whatever\n".to_vec());
        // broken/y.rs has no content → ObjectMissing skip; x.rs still pairs.
        let outcome = match_pairs(&snap, &config(30000), &mut source);
        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(outcome.matches[0].old, PathBuf::from("ok/x.rs"));
        assert!(
            outcome.stats.content_skips[&SkipReason::ObjectMissing] >= 1,
            "skip must be recorded: {:?}",
            outcome.stats.content_skips
        );
    }

    #[test]
    fn tie_break_is_deterministic_by_path_bytes() {
        // Two sources with identical content compete for two identical
        // destinations: ties must resolve old↑ then new↑ by path bytes.
        let bytes = b"tie tie tie tie\n".to_vec();
        let snap = snapshot(
            &[
                ("o/b", regular(BlobEvidence::Unknown, bytes.len() as u64)),
                ("o/a", regular(BlobEvidence::Unknown, bytes.len() as u64)),
            ],
            &[
                ("n/d", regular(BlobEvidence::Unknown, bytes.len() as u64)),
                ("n/c", regular(BlobEvidence::Unknown, bytes.len() as u64)),
            ],
        );
        let mut source = MapSource::new();
        for p in ["o/b", "o/a"] {
            source.old.insert(PathBuf::from(p), bytes.clone());
        }
        for p in ["n/d", "n/c"] {
            source.new.insert(PathBuf::from(p), bytes.clone());
        }
        let outcome = match_pairs(&snap, &config(30000), &mut source);
        assert_eq!(outcome.matches.len(), 2);
        assert_eq!(outcome.matches[0].old, PathBuf::from("o/a"));
        assert_eq!(outcome.matches[0].new, PathBuf::from("n/c"));
        assert_eq!(outcome.matches[1].old, PathBuf::from("o/b"));
        assert_eq!(outcome.matches[1].new, PathBuf::from("n/d"));
    }

    #[test]
    fn threshold_60000_is_exact_only() {
        let near_old = b"one\ntwo\nthree\nfour\n".to_vec();
        let near_new = b"one\ntwo\nthree\nfour!\n".to_vec();
        let snap = snapshot(
            &[("a", regular(BlobEvidence::Unknown, near_old.len() as u64))],
            &[("b", regular(BlobEvidence::Unknown, near_new.len() as u64))],
        );
        let mut source = MapSource::new();
        source.old.insert(PathBuf::from("a"), near_old);
        source.new.insert(PathBuf::from("b"), near_new);
        let outcome = match_pairs(&snap, &config(EXACT_SCORE), &mut source);
        assert!(outcome.matches.is_empty());
        assert_eq!(source.reads, 0, "-M100% must not read content");
    }

    #[test]
    fn similarity_score_matches_span_semantics() {
        assert_eq!(similarity_score(b"", b""), EXACT_SCORE);
        assert_eq!(similarity_score(b"abc\n", b"abc\n"), EXACT_SCORE);
        let half = similarity_score(b"aaaa\nbbbb\n", b"aaaa\ncccc\n");
        assert!(half > 20000 && half < 40000, "got {half}");
    }

    #[test]
    fn score_percent_floors_and_caps() {
        let m = |score: u32| RenameMatch {
            old: PathBuf::from("a"),
            new: PathBuf::from("b"),
            exact: false,
            internal_score: score,
        };
        assert_eq!(m(EXACT_SCORE).score_percent(), 100);
        assert_eq!(m(59999).score_percent(), 99, "§B.9: 59999→99 floor");
        assert_eq!(m(30000).score_percent(), 50);
    }
}
