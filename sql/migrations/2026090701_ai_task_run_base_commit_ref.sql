-- B3-16: additive nullable tagged repo-commit column for ai_index_task_run.
-- Forward-only storage substrate for B3-10 RepoCommitRef values.
--
-- Shape guard: when the table exists, it must expose the legacy four columns
-- (task_id, run_id, is_latest, created_at) as primary-key identity. Missing
-- table is a no-op (in-memory fixtures that run builtin migrations without
-- bootstrap never create the AI projection schema). Drifted PK columns fail
-- closed before the column add.

CREATE TABLE IF NOT EXISTS `_ai_task_run_base_commit_ref_up_guard` (`probe` INTEGER);

CREATE TRIGGER `_ai_task_run_base_commit_ref_up_shape_guard`
BEFORE INSERT ON `_ai_task_run_base_commit_ref_up_guard`
WHEN
    -- Table is present…
    (SELECT COUNT(*) FROM sqlite_master
        WHERE type = 'table' AND name = 'ai_index_task_run') = 1
    -- …but not on a recognized legacy / already-migrated shape.
    AND NOT (
        (SELECT COUNT(*) FROM pragma_table_info('ai_index_task_run')
            WHERE name IN ('task_id', 'run_id', 'is_latest', 'created_at')) = 4
        AND (
            (SELECT COUNT(*) FROM pragma_table_info('ai_index_task_run')) = 4
            OR EXISTS (
                SELECT 1 FROM pragma_table_info('ai_index_task_run')
                WHERE name = 'base_commit_ref'
            )
        )
    )
BEGIN
    SELECT RAISE(
        ABORT,
        'ai_index_task_run shape guard failed: expected legacy columns (task_id, run_id, is_latest, created_at); refusing base_commit_ref migration'
    );
END;

INSERT INTO `_ai_task_run_base_commit_ref_up_guard` (`probe`) VALUES (1);
DROP TRIGGER `_ai_task_run_base_commit_ref_up_shape_guard`;
DROP TABLE `_ai_task_run_base_commit_ref_up_guard`;

-- Column add is performed by `apply_migration_compatibility` via
-- `add_column_if_missing` (runs before this SQL) so forward re-entry stays
-- idempotent when the column is already present.
