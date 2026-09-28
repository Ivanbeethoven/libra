-- B3-16: protected down for ai_index_task_run.base_commit_ref.
-- Non-NULL tagged refs cannot be represented on the pre-migration schema,
-- so fail closed instead of silently discarding them.

CREATE TABLE IF NOT EXISTS `_ai_task_run_base_commit_ref_down_guard` (`probe` INTEGER);

CREATE TRIGGER `_ai_task_run_base_commit_ref_down_guard_trig`
BEFORE INSERT ON `_ai_task_run_base_commit_ref_down_guard`
WHEN EXISTS (
    SELECT 1 FROM pragma_table_info('ai_index_task_run')
    WHERE name = 'base_commit_ref'
)
AND EXISTS (
    SELECT 1 FROM `ai_index_task_run`
    WHERE `base_commit_ref` IS NOT NULL
)
BEGIN
    SELECT RAISE(
        ABORT,
        'refusing to drop ai_index_task_run.base_commit_ref: non-NULL tagged refs exist; restore from backup instead of rolling back'
    );
END;

INSERT INTO `_ai_task_run_base_commit_ref_down_guard` (`probe`) VALUES (1);
DROP TRIGGER `_ai_task_run_base_commit_ref_down_guard_trig`;
DROP TABLE `_ai_task_run_base_commit_ref_down_guard`;

-- Rebuild to the legacy four-column shape when the column is present (or
-- already absent — SELECT of only legacy columns stays valid either way).
CREATE TABLE IF NOT EXISTS `ai_index_task_run__rebuild` (
    `task_id` TEXT NOT NULL,
    `run_id` TEXT NOT NULL,
    `is_latest` INTEGER NOT NULL DEFAULT 0,
    `created_at` INTEGER NOT NULL,
    PRIMARY KEY (`task_id`, `run_id`)
);

INSERT INTO `ai_index_task_run__rebuild`
SELECT `task_id`, `run_id`, `is_latest`, `created_at` FROM `ai_index_task_run`;

DROP TABLE `ai_index_task_run`;
ALTER TABLE `ai_index_task_run__rebuild` RENAME TO `ai_index_task_run`;
