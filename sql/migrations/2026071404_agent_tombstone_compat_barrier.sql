-- 2026071404_agent_tombstone_compat_barrier: schema-level anti-resurrection
-- compatibility fence for repositories that already applied 2026071403.
--
-- A pre-M4 writer does not consult `agent_import_tombstone`. These triggers
-- therefore protect the user-reachable session/checkpoint catalog entry
-- points even when an older executable opens a newer repository database.
-- The current writer still performs same-transaction checks so it can return
-- contextual LBR-AGENT-019 errors instead of relying on trigger text.

CREATE TRIGGER IF NOT EXISTS `agent_tombstone_block_session_insert`
BEFORE INSERT ON `agent_session`
WHEN EXISTS (
    SELECT 1 FROM `agent_import_tombstone`
    WHERE `agent_kind` = NEW.`agent_kind`
      AND `provider_session_id` = NEW.`provider_session_id`
)
BEGIN
    SELECT RAISE(ABORT, 'agent session is protected by an erasure tombstone');
END;

CREATE TRIGGER IF NOT EXISTS `agent_tombstone_block_session_update`
BEFORE UPDATE OF `agent_kind`, `provider_session_id`, `state`, `last_event_at`, `stopped_at`
ON `agent_session`
WHEN EXISTS (
    SELECT 1 FROM `agent_import_tombstone`
    WHERE `agent_kind` = NEW.`agent_kind`
      AND `provider_session_id` = NEW.`provider_session_id`
)
BEGIN
    SELECT RAISE(ABORT, 'agent session is protected by an erasure tombstone');
END;

CREATE TRIGGER IF NOT EXISTS `agent_tombstone_block_checkpoint_insert`
BEFORE INSERT ON `agent_checkpoint`
WHEN EXISTS (
    SELECT 1 FROM `agent_import_tombstone`
    WHERE `erased_session_id` = NEW.`session_id`
)
BEGIN
    SELECT RAISE(ABORT, 'agent checkpoint is protected by an erasure tombstone');
END;
