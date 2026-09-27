//! AI projection index substrate (task↔run reverse index).
//!
//! B3-16 lands the `base_commit_ref` storage column and the
//! snapshot-before-delete rebuild contract (GC-08). Tagged-value write/read
//! semantics for consumers land in B3-10.

pub mod index;
pub mod rebuild;
pub mod resolver;

pub use index::TaskRunIndexRow;
pub use rebuild::{RebuildSnapshot, rebuild_task_run_index, snapshot_task_run_base_commit_refs};
pub use resolver::{ResolvedTaskRun, resolve_task_run};
