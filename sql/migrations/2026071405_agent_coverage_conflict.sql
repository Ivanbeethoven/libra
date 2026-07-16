-- 2026071405_agent_coverage_conflict: durable first-challenger evidence for
-- complete-vs-different-complete coverage collisions. One row per claim keeps
-- conflict persistence bounded while preserving both candidates needed for
-- manual recovery.

CREATE TABLE IF NOT EXISTS `agent_coverage_conflict` (
    `session_id`                TEXT    NOT NULL
        REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `logical_turn_key`          TEXT    NOT NULL,
    `coverage_schema_version`   INTEGER NOT NULL,
    `incumbent_revision`        INTEGER NOT NULL,
    `incumbent_digest`          TEXT    NOT NULL,
    `incumbent_checkpoint_id`   TEXT,
    `incoming_digest`           TEXT    NOT NULL,
    `incoming_source_channel`   TEXT    NOT NULL
        CHECK(`incoming_source_channel` IN ('live','import','export')),
    `incoming_observed_at`      INTEGER NOT NULL,
    `incoming_canonical_json`   TEXT    NOT NULL,
    `incoming_redaction_report_json` TEXT NOT NULL,
    PRIMARY KEY (`session_id`, `logical_turn_key`, `coverage_schema_version`),
    FOREIGN KEY (`session_id`, `logical_turn_key`, `coverage_schema_version`)
        REFERENCES `agent_coverage_claim`(
            `session_id`, `logical_turn_key`, `coverage_schema_version`
        ) ON DELETE CASCADE,
    FOREIGN KEY (`incumbent_checkpoint_id`)
        REFERENCES `agent_checkpoint`(`checkpoint_id`) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS `idx_agent_coverage_conflict_observed_at`
    ON `agent_coverage_conflict`(`incoming_observed_at`);
