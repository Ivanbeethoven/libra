-- Refuse to discard replication/prune state that the 1406 base schema cannot
-- represent.  A claim cursor equal to its visible revision can safely fall
-- back to the original 1406 allocation rule.

CREATE TABLE `_agent_subagent_replication_down_guard` (`probe` INTEGER NOT NULL);
CREATE TRIGGER `_agent_subagent_replication_down_guard_advanced`
BEFORE INSERT ON `_agent_subagent_replication_down_guard`
WHEN EXISTS (SELECT 1 FROM `agent_capture_incarnation` LIMIT 1)
   OR EXISTS (SELECT 1 FROM `agent_capture_cloud_base` LIMIT 1)
   OR EXISTS (SELECT 1 FROM `agent_checkpoint_prune_tombstone` LIMIT 1)
   OR EXISTS (
       SELECT 1 FROM `agent_session`
       WHERE `sync_revision` <> 1
       LIMIT 1
   )
   OR EXISTS (
       SELECT 1 FROM `agent_checkpoint`
       WHERE `sync_revision` <> 1
       LIMIT 1
   )
   OR EXISTS (
       SELECT 1 FROM `agent_subagent_content_claim`
       WHERE `state` = 'reserved'
          OR `revision_cursor` <> `current_revision`
          OR `sync_revision` <> 1
       LIMIT 1
   )
   OR EXISTS (
       SELECT 1 FROM `agent_subagent_link`
       WHERE `link_state` = 'resolved' OR `sync_revision` <> 1
       LIMIT 1
   )
BEGIN
    SELECT RAISE(ABORT, 'cannot roll back subagent replication while advanced generations, prune fences, reservations, or resolved links exist');
END;
INSERT INTO `_agent_subagent_replication_down_guard` (`probe`) VALUES (1);
DROP TRIGGER `_agent_subagent_replication_down_guard_advanced`;
DROP TABLE `_agent_subagent_replication_down_guard`;

DROP TRIGGER IF EXISTS `trg_agent_subagent_boundary_delete`;
DROP INDEX IF EXISTS `idx_agent_checkpoint_prune_tombstone_session`;
DROP TABLE IF EXISTS `agent_checkpoint_prune_tombstone`;
DROP TABLE IF EXISTS `agent_capture_incarnation`;
DROP TABLE IF EXISTS `agent_capture_cloud_base`;
ALTER TABLE `agent_subagent_link` DROP COLUMN `sync_revision`;
ALTER TABLE `agent_subagent_content_claim` DROP COLUMN `sync_revision`;
ALTER TABLE `agent_subagent_content_claim` DROP COLUMN `revision_cursor`;
ALTER TABLE `agent_checkpoint` DROP COLUMN `sync_revision`;
ALTER TABLE `agent_session` DROP COLUMN `sync_revision`;
