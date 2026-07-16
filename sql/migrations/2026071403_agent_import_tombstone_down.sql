-- Down migration for 2026071403_agent_import_tombstone (GC-DR-05a).
-- Rollback is permitted only while the barrier is empty. Once an erase has
-- recorded a tombstone, dropping this table would make that provider identity
-- importable again and is therefore refused transactionally.

CREATE TABLE `_agent_tombstone_down_guard` (`probe` INTEGER NOT NULL);
CREATE TRIGGER `_agent_tombstone_down_guard_nonempty`
BEFORE INSERT ON `_agent_tombstone_down_guard`
WHEN EXISTS (SELECT 1 FROM `agent_import_tombstone` LIMIT 1)
BEGIN
    SELECT RAISE(ABORT, 'cannot roll back agent tombstones after an erased identity has been recorded');
END;
INSERT INTO `_agent_tombstone_down_guard` (`probe`) VALUES (1);
DROP TRIGGER `_agent_tombstone_down_guard_nonempty`;
DROP TABLE `_agent_tombstone_down_guard`;

DROP TRIGGER IF EXISTS `agent_tombstone_block_checkpoint_insert`;
DROP TRIGGER IF EXISTS `agent_tombstone_block_session_update`;
DROP TRIGGER IF EXISTS `agent_tombstone_block_session_insert`;
DROP INDEX IF EXISTS `idx_agent_import_tombstone_erased_session`;
DROP INDEX IF EXISTS `idx_agent_import_tombstone_provider`;
DROP TABLE IF EXISTS `agent_import_tombstone`;
