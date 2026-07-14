-- 2026071402_agent_import_identity: import-job state / crash-recovery identity
-- (plan-20260713 DR-05c, ADR-DR-06/DR-10/DR-11, GC-DR-05/11/12).
--
-- One row per import job, keyed by a STABLE identity that never embeds a
-- syncable absolute home path: (agent_kind, provider_session_id, source_kind,
-- source_id, schema_version). `source_id` is a provider-root-relative id or a
-- salted, non-reversible fingerprint — never the raw `~/.claude/...` path.
--
-- This table is IMPORT-ONLY (ADR-DR-06): live capture never writes it. It
-- tracks per-job progress so a crashed/again-run import resumes without
-- double-appending — `observed_digest` vs `committed_digest`, the current
-- `attempt_id`/`attempt_checkpoint_id`, the `next_ordinal`, and an
-- owner/lease/fence trio mirroring the export-job coordinator. The claim gate
-- (agent_coverage_claim) remains the cross-path exactly-once authority; this
-- table only coordinates the import job itself.
--
-- Idempotency: every DDL statement uses `IF NOT EXISTS` (house style).

CREATE TABLE IF NOT EXISTS `agent_import_identity` (
    `identity_id`            TEXT PRIMARY KEY,
    `agent_kind`             TEXT    NOT NULL,
    `provider_session_id`    TEXT    NOT NULL,
    `source_kind`            TEXT    NOT NULL,
    -- Provider-root-relative id or salted fingerprint — NEVER an absolute
    -- home path (GC-DR-13 / ADR-DR-06: must not leak the user's home).
    `source_id`              TEXT    NOT NULL,
    `schema_version`         INTEGER NOT NULL,
    -- Content digests: what the source currently presents vs what has been
    -- committed. A digest change appends a coverage revision (never rewrites
    -- the structural checkpoint parent).
    `observed_digest`        TEXT,
    `committed_digest`       TEXT,
    -- Crash-recovery cursor: the injectable attempt checkpoint id and the
    -- next per-turn ordinal to write.
    `attempt_id`             TEXT,
    `attempt_checkpoint_id`  TEXT,
    `next_ordinal`           INTEGER NOT NULL DEFAULT 0,
    `state`                  TEXT    NOT NULL
        CHECK(`state` IN ('discovered','leased','writing','partial','committed','failed')),
    `owner`                  TEXT,
    `lease_expires_at`       INTEGER,
    `fence_token`            INTEGER,
    `last_error_code`        TEXT,
    `created_at`             INTEGER NOT NULL,
    `updated_at`             INTEGER NOT NULL
);

-- Stable-identity uniqueness (ADR-DR-06 / GC-DR): the job key excludes the
-- content digest, so a re-run resolves the SAME row and resumes.
CREATE UNIQUE INDEX IF NOT EXISTS `idx_agent_import_identity_key`
    ON `agent_import_identity`(
        `agent_kind`, `provider_session_id`, `source_kind`, `source_id`, `schema_version`
    );
