//! Capture-graph command surface placeholders.
//!
//! The historical orchestrator `graph` module was removed with the Code UI
//! runtime (plan-20260920 RC-23). B3-16 keeps a thin `#[cfg(test)]` Model
//! construction site so SeaORM entity field additions stay compile-checked
//! against the writeset path named in plan-20260907.

#[cfg(test)]
mod tests {
    use crate::internal::model::ai_index_task_run;

    /// Entity-shape guard: every `ai_index_task_run::Model` construction site
    /// must supply `base_commit_ref` after B3-16.
    #[test]
    fn ai_index_task_run_model_includes_base_commit_ref() {
        let model = ai_index_task_run::Model {
            task_id: "task".into(),
            run_id: "run".into(),
            is_latest: false,
            created_at: 0,
            base_commit_ref: None,
        };
        assert!(model.base_commit_ref.is_none());
    }
}
