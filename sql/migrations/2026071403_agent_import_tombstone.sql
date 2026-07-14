-- 2026071403_agent_import_tombstone: local anti-resurrection tombstone
-- (plan-20260713 ADR-DR-06/DR-15/DR-19, GC-DR-05/13).
--
-- Written when a user ERASES a captured session, BEFORE the `agent_session`
-- row is deleted, inside the same transaction (ADR-DR-19: the tombstone is a
-- concurrent write barrier — any in-flight import/export holder is fenced out
-- and cannot re-create the session/ref after erase).
--
-- Two identities are stored so both the import-block path and the DR-07
-- read-only graph work after the catalog row is gone:
--   * (agent_kind, provider_session_id) UNIQUE — the IMPORT-BLOCK key:
--     discovery/import skip a session whose provider identity is tombstoned.
--   * erased_session_id TEXT NOT NULL UNIQUE (no FK) — the id
--     `libra agent graph <session>` queries to classify a deleted session as
--     `erased` (not `unknown`) even after `agent_session` is removed.
--
-- `source_fingerprint` is an AUDIT-ONLY, non-reversible attribute — it must
-- NOT be a matching condition (else a moved/copied session would resurrect).
-- Ordinary retention never deletes tombstones; only an explicit, audited
-- `--restore-erased` removes one (machine/non-TTY defaults to refuse).
-- Local-only: no cloud tombstone propagation is claimed.
--
-- Idempotency: every DDL statement uses `IF NOT EXISTS` (house style).

CREATE TABLE IF NOT EXISTS `agent_import_tombstone` (
    `tombstone_id`         TEXT PRIMARY KEY,
    `agent_kind`           TEXT    NOT NULL,
    `provider_session_id`  TEXT    NOT NULL,
    -- The `<provider>__<provider_session_id>` capture session id; no FK so it
    -- survives deletion of `agent_session` (DR-07 reads it to show `erased`).
    `erased_session_id`    TEXT    NOT NULL,
    -- Audit-only, non-reversible; NEVER a match/resurrection condition.
    `source_fingerprint`   TEXT,
    `erased_at`            INTEGER NOT NULL
);

-- Import-block key: at most one tombstone per provider session identity.
CREATE UNIQUE INDEX IF NOT EXISTS `idx_agent_import_tombstone_provider`
    ON `agent_import_tombstone`(`agent_kind`, `provider_session_id`);
-- Read-only `erased` classification key for DR-07.
CREATE UNIQUE INDEX IF NOT EXISTS `idx_agent_import_tombstone_erased_session`
    ON `agent_import_tombstone`(`erased_session_id`);
