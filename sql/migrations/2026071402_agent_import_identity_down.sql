-- Down migration for 2026071402_agent_import_identity (dev/test rollback only,
-- GC-DR-05a): only safe before any import job has committed under the new
-- contract. Never a production "drop-to-rollback" data-loss path.

CREATE TABLE `_agent_import_identity_down_guard` (`probe` INTEGER NOT NULL);
CREATE TRIGGER `_agent_import_identity_down_guard_nonempty`
BEFORE INSERT ON `_agent_import_identity_down_guard`
WHEN EXISTS (SELECT 1 FROM `agent_import_identity` LIMIT 1)
BEGIN
    SELECT RAISE(ABORT, 'cannot roll back agent import identities while import recovery state exists');
END;
INSERT INTO `_agent_import_identity_down_guard` (`probe`) VALUES (1);
DROP TRIGGER `_agent_import_identity_down_guard_nonempty`;
DROP TABLE `_agent_import_identity_down_guard`;

DROP INDEX IF EXISTS `idx_agent_import_identity_key`;
DROP TABLE IF EXISTS `agent_import_identity`;
