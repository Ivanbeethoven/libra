//! Row shape for the task→run reverse index projection.

/// One projected `ai_index_task_run` row, including the nullable tagged
/// repository-commit column introduced by B3-16.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRunIndexRow {
    pub task_id: String,
    pub run_id: String,
    pub is_latest: bool,
    pub created_at: i64,
    /// Tagged `repo-commit:<kind>:<hex>`, or `None` for legacy bare-anchor rows.
    pub base_commit_ref: Option<String>,
}

impl TaskRunIndexRow {
    /// Build a row with a nullable tagged ref (test and rebuild helpers).
    pub fn new(
        task_id: impl Into<String>,
        run_id: impl Into<String>,
        is_latest: bool,
        created_at: i64,
        base_commit_ref: Option<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            run_id: run_id.into(),
            is_latest,
            created_at,
            base_commit_ref,
        }
    }
}
