-- 2026071301_agent_coverage_gate: per-turn coverage claim / revision gate
-- (plan-20260713 DR-05c-0, ADR-DR-06/08/09/10/16, GC-DR-11).
--
-- `agent_coverage_claim` is the WRITE-FRONT GATE + current-coverage fact for
-- one logical turn of one captured session: at most one row per
-- (session_id, logical_turn_key, coverage_schema_version). Live hook writers
-- and (M4) import writers reserve a claim BEFORE building checkpoint objects,
-- so repeated TurnEnd events / re-imports of the same turn become no-ops
-- instead of duplicate checkpoints.
--
-- `agent_coverage_revision` is the APPEND-ONLY per-turn version history: one
-- row per committed coverage revision (e.g. an `incomplete` truncated snapshot
-- later upgraded by a `complete` one). Supersede relations live HERE, never on
-- `agent_checkpoint` (a whole-transcript checkpoint can back many turns, so a
-- checkpoint-level superseded flag would hide other turns' evidence —
-- ADR-DR-16).
--
-- Idempotency: every DDL statement uses `IF NOT EXISTS`, matching the house
-- style, so the migration is safe on databases that already have the shape.

CREATE TABLE IF NOT EXISTS `agent_coverage_claim` (
    -- Logical identity (UNIQUE below). `coverage_digest` is the CONTENT
    -- version of the turn and deliberately NOT part of the unique key: a
    -- truncated and a completed snapshot of the same turn must collide here
    -- and resolve via the revision model, not become two turns (ADR-DR-08).
    `session_id`               TEXT    NOT NULL
        REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `logical_turn_key`         TEXT    NOT NULL,
    `coverage_schema_version`  INTEGER NOT NULL,
    `coverage_digest`          TEXT    NOT NULL,
    `completeness`             TEXT    NOT NULL
        CHECK(`completeness` IN ('incomplete','complete')),
    -- Current committed revision for this turn; 0 = reserved but nothing
    -- committed yet (readers skip such rows — plan-20260713 ADR-DR-20).
    `revision`                 INTEGER NOT NULL DEFAULT 0,
    `state`                    TEXT    NOT NULL
        CHECK(`state` IN (
            'reserved_live','reserved_import','catalog_committed',
            'abandoned','conflicted'
        )),
    -- Checkpoint id minted for the in-flight attempt (may become unreachable
    -- garbage if the attempt loses the race; never becomes visible then).
    `attempt_checkpoint_id`    TEXT,
    -- Reservation lease (ADR-DR-09/10). All three nullable: a claim that has
    -- reached `catalog_committed` no longer carries an active lease. Fence
    -- increments use COALESCE(fence_token, 0) + 1 so takeover stays monotonic.
    `owner`                    TEXT,
    `lease_expires_at`         INTEGER,
    `fence_token`              INTEGER,
    -- Set only by the final atomic commit transaction (`catalog_committed`
    -- state invariant, ADR-DR-10): both stay NULL before that.
    `checkpoint_id`            TEXT,
    `traces_commit`            TEXT,
    -- Provenance only — never participates in dedup arbitration (ADR-DR-09).
    `source_channel`           TEXT    NOT NULL
        CHECK(`source_channel` IN ('live','import','export')),
    `created_at`               INTEGER NOT NULL,
    `updated_at`               INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS `idx_agent_coverage_claim_logical_key`
    ON `agent_coverage_claim`(`session_id`, `logical_turn_key`, `coverage_schema_version`);
CREATE INDEX IF NOT EXISTS `idx_agent_coverage_claim_session_state`
    ON `agent_coverage_claim`(`session_id`, `state`);
CREATE INDEX IF NOT EXISTS `idx_agent_coverage_claim_checkpoint_id`
    ON `agent_coverage_claim`(`checkpoint_id`);

CREATE TABLE IF NOT EXISTS `agent_coverage_revision` (
    -- Append-only committed history: every column NOT NULL (a committed
    -- revision always has a checkpoint / digest / completeness / channel),
    -- which is the schema-level basis for the graph JSON v1 non-null promise
    -- (plan-20260713 ADR-DR-20).
    `session_id`               TEXT    NOT NULL
        REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `logical_turn_key`         TEXT    NOT NULL,
    `coverage_schema_version`  INTEGER NOT NULL,
    `revision`                 INTEGER NOT NULL,
    `checkpoint_id`            TEXT    NOT NULL,
    `coverage_digest`          TEXT    NOT NULL,
    `completeness`             TEXT    NOT NULL
        CHECK(`completeness` IN ('incomplete','complete')),
    `source_channel`           TEXT    NOT NULL
        CHECK(`source_channel` IN ('live','import','export')),
    `created_at`               INTEGER NOT NULL,
    PRIMARY KEY (`session_id`, `logical_turn_key`, `coverage_schema_version`, `revision`)
);

CREATE INDEX IF NOT EXISTS `idx_agent_coverage_revision_checkpoint_id`
    ON `agent_coverage_revision`(`checkpoint_id`);
