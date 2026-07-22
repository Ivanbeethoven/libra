-- 2026071406_agent_subagent_content: source-scoped subagent transcript
-- attribution (plan-20260713 DR-06 / ADR-DR-05).
--
-- A content source is identified by its parent capture session, provider,
-- an opaque digest of the provider-root-relative source key, and projection schema. Revisions are
-- append-only; the claim row names the sole current content leaf and carries
-- a short-lived writer reservation.  Boundary association is deliberately a
-- separate relation: a later reliable provider id may resolve the link
-- without rewriting either checkpoint metadata or immutable traces history.

CREATE TABLE IF NOT EXISTS `agent_subagent_content_claim` (
    `parent_session_id`       TEXT    NOT NULL
        REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `provider_kind`           TEXT    NOT NULL,
    `source_key`              TEXT    NOT NULL,
    `content_schema_version`  INTEGER NOT NULL,
    `current_revision`        INTEGER NOT NULL DEFAULT 0,
    `current_checkpoint_id`   TEXT,
    `current_digest`          TEXT,
    `state`                   TEXT    NOT NULL
        CHECK(`state` IN ('idle','reserved')),
    `attempt_digest`          TEXT,
    `attempt_checkpoint_id`   TEXT,
    `owner`                   TEXT,
    `lease_expires_at`        INTEGER,
    `fence_token`             INTEGER NOT NULL DEFAULT 0,
    `created_at`              INTEGER NOT NULL,
    `updated_at`              INTEGER NOT NULL,
    PRIMARY KEY (
        `parent_session_id`, `provider_kind`, `source_key`,
        `content_schema_version`
    ),
    CHECK(
        (`current_revision` = 0 AND `current_checkpoint_id` IS NULL AND `current_digest` IS NULL)
        OR
        (`current_revision` > 0 AND `current_checkpoint_id` IS NOT NULL AND `current_digest` IS NOT NULL)
    ),
    CHECK(
        (`state` = 'idle' AND `attempt_digest` IS NULL AND `attempt_checkpoint_id` IS NULL
            AND `owner` IS NULL AND `lease_expires_at` IS NULL)
        OR
        (`state` = 'reserved' AND `attempt_digest` IS NOT NULL AND `owner` IS NOT NULL
            AND `lease_expires_at` IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS `idx_agent_subagent_content_claim_current`
    ON `agent_subagent_content_claim`(`current_checkpoint_id`);
CREATE INDEX IF NOT EXISTS `idx_agent_subagent_content_claim_state`
    ON `agent_subagent_content_claim`(`state`, `lease_expires_at`);

CREATE TABLE IF NOT EXISTS `agent_subagent_content_revision` (
    `parent_session_id`       TEXT    NOT NULL
        REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `provider_kind`           TEXT    NOT NULL,
    `source_key`              TEXT    NOT NULL,
    `content_schema_version`  INTEGER NOT NULL,
    `revision`                INTEGER NOT NULL,
    `checkpoint_id`           TEXT    NOT NULL
        REFERENCES `agent_checkpoint`(`checkpoint_id`) ON DELETE CASCADE,
    `content_digest`          TEXT    NOT NULL,
    `source_channel`          TEXT    NOT NULL
        CHECK(`source_channel` IN ('live','import')),
    `partial`                 INTEGER NOT NULL CHECK(`partial` IN (0, 1)),
    `created_at`              INTEGER NOT NULL,
    PRIMARY KEY (
        `parent_session_id`, `provider_kind`, `source_key`,
        `content_schema_version`, `revision`
    ),
    UNIQUE(`checkpoint_id`),
    FOREIGN KEY (
        `parent_session_id`, `provider_kind`, `source_key`,
        `content_schema_version`
    ) REFERENCES `agent_subagent_content_claim`(
        `parent_session_id`, `provider_kind`, `source_key`,
        `content_schema_version`
    ) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS `idx_agent_subagent_content_revision_source`
    ON `agent_subagent_content_revision`(
        `parent_session_id`, `provider_kind`, `source_key`, `revision`
    );

CREATE TABLE IF NOT EXISTS `agent_subagent_link` (
    `content_checkpoint_id`   TEXT    PRIMARY KEY
        REFERENCES `agent_checkpoint`(`checkpoint_id`) ON DELETE CASCADE,
    `parent_session_id`       TEXT    NOT NULL
        REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `link_state`              TEXT    NOT NULL
        CHECK(`link_state` IN ('resolved','unresolved')),
    `boundary_checkpoint_id`  TEXT
        REFERENCES `agent_checkpoint`(`checkpoint_id`) ON DELETE SET NULL,
    `stable_subagent_id`      TEXT,
    `created_at`              INTEGER NOT NULL,
    `updated_at`              INTEGER NOT NULL,
    CHECK(
        (`link_state` = 'resolved' AND `boundary_checkpoint_id` IS NOT NULL
            AND `stable_subagent_id` IS NOT NULL)
        OR
        (`link_state` = 'unresolved' AND `boundary_checkpoint_id` IS NULL)
    )
);

CREATE INDEX IF NOT EXISTS `idx_agent_subagent_link_parent_state`
    ON `agent_subagent_link`(`parent_session_id`, `link_state`);
CREATE INDEX IF NOT EXISTS `idx_agent_subagent_link_boundary`
    ON `agent_subagent_link`(`boundary_checkpoint_id`);
