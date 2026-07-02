-- Revision ordinal index (lore.md 1.16): rebuildable side table mapping
-- commit OID <-> 1-based ordinal on a ref's root->tip FIRST-PARENT chain
-- (Lore's linear revision numbering). Deterministic: the numbering is a pure
-- function of the tip OID (+ the refs/replace set, which the freshness
-- fingerprint includes). Freshness is validated on every read; fast-forwards
-- append, history rewrites rebuild — a stale index never answers.
-- Owner API: `internal::revision_ordinal::RevisionOrdinalIndex`.
CREATE TABLE IF NOT EXISTS `revision_ordinal` (
    `id`       INTEGER PRIMARY KEY AUTOINCREMENT,
    `ref_name` TEXT NOT NULL,
    `ordinal`  INTEGER NOT NULL,
    `oid`      TEXT NOT NULL,
    UNIQUE(`ref_name`, `ordinal`),
    UNIQUE(`ref_name`, `oid`)
);
CREATE TABLE IF NOT EXISTS `revision_ordinal_meta` (
    `ref_name`    TEXT PRIMARY KEY,
    `tip_oid`     TEXT NOT NULL,
    `replace_sig` TEXT NOT NULL DEFAULT '',
    `max_ordinal` INTEGER NOT NULL,
    `built_at`    TEXT NOT NULL
);
