-- Down migration for 2026071402_agent_import_identity (dev/test rollback only,
-- GC-DR-05a): only safe before any import job has committed under the new
-- contract. Never a production "drop-to-rollback" data-loss path.

DROP INDEX IF EXISTS `idx_agent_import_identity_key`;
DROP TABLE IF EXISTS `agent_import_identity`;
