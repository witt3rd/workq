-- Replace the non-unique partial index with a UNIQUE partial index
-- to enforce dedup at the database level.
DROP INDEX IF EXISTS idx_work_dedup;

CREATE UNIQUE INDEX idx_work_dedup ON work_items(work_type, dedup_key)
    WHERE dedup_key IS NOT NULL AND state NOT IN ('completed', 'dead', 'merged');
