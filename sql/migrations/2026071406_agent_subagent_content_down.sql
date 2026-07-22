-- Down migration for 2026071406_agent_subagent_content.  Refuse to discard
-- the only source-scoped revision/link index while any content has landed.

CREATE TABLE `_agent_subagent_content_down_guard` (`probe` INTEGER NOT NULL);
CREATE TRIGGER `_agent_subagent_content_down_guard_nonempty`
BEFORE INSERT ON `_agent_subagent_content_down_guard`
WHEN EXISTS (SELECT 1 FROM `agent_subagent_content_revision` LIMIT 1)
   OR EXISTS (SELECT 1 FROM `agent_subagent_link` LIMIT 1)
   OR EXISTS (
       SELECT 1 FROM `agent_subagent_content_claim`
       WHERE `state` = 'reserved'
       LIMIT 1
   )
BEGIN
    SELECT RAISE(ABORT, 'cannot roll back subagent content attribution while revisions, links, reservations, or advanced session generations exist');
END;
INSERT INTO `_agent_subagent_content_down_guard` (`probe`) VALUES (1);
DROP TRIGGER `_agent_subagent_content_down_guard_nonempty`;
DROP TABLE `_agent_subagent_content_down_guard`;

DROP INDEX IF EXISTS `idx_agent_subagent_link_boundary`;
DROP INDEX IF EXISTS `idx_agent_subagent_link_parent_state`;
DROP TABLE IF EXISTS `agent_subagent_link`;
DROP INDEX IF EXISTS `idx_agent_subagent_content_revision_source`;
DROP TABLE IF EXISTS `agent_subagent_content_revision`;
DROP INDEX IF EXISTS `idx_agent_subagent_content_claim_state`;
DROP INDEX IF EXISTS `idx_agent_subagent_content_claim_current`;
DROP TABLE IF EXISTS `agent_subagent_content_claim`;
