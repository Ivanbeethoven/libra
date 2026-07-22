-- 2026071407_agent_subagent_replication: monotonic replication generations,
-- source revision allocation, and compatibility hardening for M5 subagent
-- content attribution.
--
-- Keep 2026071406 immutable: early M5 development builds applied its base
-- claim/revision/link schema before cloud replication and prune fencing were
-- added.  This follow-up makes those additions visible to already-migrated
-- repositories as well as fresh installs.

-- SQLite has no `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`.  The migration
-- runner therefore installs the five additive columns conditionally, inside
-- the same transaction as this SQL and the schema_versions row.  Keeping the
-- table/index/trigger DDL here idempotent lets both the early 1406 shape and
-- the later development 1406 shape converge on the same schema.

-- Preserve a fresh replication epoch and opaque source namespace across a
-- local session cascade so an explicit restore cannot reuse remote counters.
CREATE TABLE IF NOT EXISTS `agent_capture_incarnation` (
    `agent_kind`                    TEXT    NOT NULL,
    `provider_session_id`           TEXT    NOT NULL,
    `next_session_sync_revision`    INTEGER NOT NULL
        CHECK(`next_session_sync_revision` > 1),
    `source_namespace`              TEXT    NOT NULL
        CHECK(length(`source_namespace`) = 32),
    `updated_at`                    INTEGER NOT NULL,
    PRIMARY KEY (`agent_kind`, `provider_session_id`)
);

-- Remember the completed D1 generation from which the local catalog
-- descended; row-local counters alone cannot order divergent clones.
CREATE TABLE IF NOT EXISTS `agent_capture_cloud_base` (
    `repo_id`             TEXT    PRIMARY KEY,
    `remote_generation`   INTEGER NOT NULL CHECK(`remote_generation` > 0),
    `updated_at`          INTEGER NOT NULL
);

-- Replicate ordinary checkpoint retention independently from deferred
-- session-erasure propagation.
CREATE TABLE IF NOT EXISTS `agent_checkpoint_prune_tombstone` (
    `checkpoint_id`  TEXT    PRIMARY KEY,
    `session_id`     TEXT    NOT NULL,
    `pruned_at`      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS `idx_agent_checkpoint_prune_tombstone_session`
    ON `agent_checkpoint_prune_tombstone`(`session_id`, `pruned_at`);

-- A boundary checkpoint may be pruned independently from its content source.
-- Make the loss of association explicit before the FK's SET NULL action so
-- the resolved-row CHECK remains valid.
CREATE TRIGGER IF NOT EXISTS `trg_agent_subagent_boundary_delete`
BEFORE DELETE ON `agent_checkpoint`
BEGIN
    UPDATE `agent_subagent_link`
       SET `link_state` = 'unresolved',
           `boundary_checkpoint_id` = NULL,
           `sync_revision` = `sync_revision` + 1,
           `updated_at` = CAST(strftime('%s', 'now') AS INTEGER) * 1000
     WHERE `boundary_checkpoint_id` = OLD.`checkpoint_id`;
END;
