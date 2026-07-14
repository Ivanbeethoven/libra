-- Down migration for 2026071403_agent_import_tombstone (dev/test rollback only,
-- GC-DR-05a). Dropping tombstones removes anti-resurrection protection, so this
-- is NEVER a production rollback path — only safe before any erase has run.

DROP INDEX IF EXISTS `idx_agent_import_tombstone_erased_session`;
DROP INDEX IF EXISTS `idx_agent_import_tombstone_provider`;
DROP TABLE IF EXISTS `agent_import_tombstone`;
