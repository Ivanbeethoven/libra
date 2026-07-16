-- Down migration for 2026071404_agent_tombstone_compat_barrier.
-- Refuse to remove the compatibility fence while any anti-resurrection
-- tombstone exists; otherwise a failed later rollback of 2026071403 would
-- leave the data in place but expose it to older writers.

CREATE TABLE `_agent_tombstone_compat_down_guard` (`probe` INTEGER NOT NULL);
CREATE TRIGGER `_agent_tombstone_compat_down_guard_nonempty`
BEFORE INSERT ON `_agent_tombstone_compat_down_guard`
WHEN EXISTS (SELECT 1 FROM `agent_import_tombstone` LIMIT 1)
BEGIN
    SELECT RAISE(ABORT, 'cannot roll back agent tombstones or remove their compatibility barrier while an erased identity exists');
END;
INSERT INTO `_agent_tombstone_compat_down_guard` (`probe`) VALUES (1);
DROP TRIGGER `_agent_tombstone_compat_down_guard_nonempty`;
DROP TABLE `_agent_tombstone_compat_down_guard`;

DROP TRIGGER IF EXISTS `agent_tombstone_block_checkpoint_insert`;
DROP TRIGGER IF EXISTS `agent_tombstone_block_session_update`;
DROP TRIGGER IF EXISTS `agent_tombstone_block_session_insert`;
