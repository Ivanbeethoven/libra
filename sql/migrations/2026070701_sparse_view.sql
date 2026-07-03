-- lore.md 2.2: read-only sparse VIEW filter (the non-declined complement of
-- D10 sparse-checkout — NO working-tree materialization). Ordered include
-- patterns owned solely by internal::sparse::SparseViewStore. The toggle
-- (sparse.enabled) lives in config_kv, mirroring git's core.sparseCheckout +
-- $GIT_DIR/info/sparse-checkout split.
CREATE TABLE IF NOT EXISTS `sparse_view` (
    `id`      INTEGER PRIMARY KEY AUTOINCREMENT,
    `pattern` TEXT NOT NULL,
    `ordinal` INTEGER NOT NULL
);
