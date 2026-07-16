-- Down migration for 2026071405_agent_coverage_conflict. Refuse rollback
-- while an unresolved collision exists: this row contains the only durable
-- copy of the sanitized challenger payload and its provenance.

CREATE TABLE `_agent_coverage_conflict_down_guard` (`probe` INTEGER NOT NULL);
CREATE TRIGGER `_agent_coverage_conflict_down_guard_nonempty`
BEFORE INSERT ON `_agent_coverage_conflict_down_guard`
WHEN EXISTS (SELECT 1 FROM `agent_coverage_conflict` LIMIT 1)
BEGIN
    SELECT RAISE(ABORT, 'cannot roll back agent coverage conflicts while unresolved challenger evidence exists');
END;
INSERT INTO `_agent_coverage_conflict_down_guard` (`probe`) VALUES (1);
DROP TRIGGER `_agent_coverage_conflict_down_guard_nonempty`;
DROP TABLE `_agent_coverage_conflict_down_guard`;

DROP INDEX IF EXISTS `idx_agent_coverage_conflict_observed_at`;
DROP TABLE IF EXISTS `agent_coverage_conflict`;
