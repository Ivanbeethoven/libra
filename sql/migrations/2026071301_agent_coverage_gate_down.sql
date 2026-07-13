-- Rollback for 2026071301_agent_coverage_gate.
--
-- Claim/revision rows are COORDINATION METADATA, not user content: the
-- captured transcripts live in `agent_checkpoint` + `refs/libra/traces`,
-- which this rollback does not touch. Dropping the gate tables returns the
-- writer to its pre-gate behavior (repeated events may append duplicate
-- checkpoints again) — acceptable for the dev/test rollback this promises
-- (plan-20260713 GC-DR-05a); production rollback must stop writers first.
--
-- Children/indexes before tables, matching the house style.

DROP INDEX IF EXISTS `idx_agent_coverage_revision_checkpoint_id`;
DROP TABLE IF EXISTS `agent_coverage_revision`;

DROP INDEX IF EXISTS `idx_agent_coverage_claim_checkpoint_id`;
DROP INDEX IF EXISTS `idx_agent_coverage_claim_session_state`;
DROP INDEX IF EXISTS `idx_agent_coverage_claim_logical_key`;
DROP TABLE IF EXISTS `agent_coverage_claim`;
