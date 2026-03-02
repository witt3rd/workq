-- Rename work_type to faculty, add skill field.
--
-- Work items now specify the target faculty directly (no routing table)
-- and carry a skill name that determines the engage methodology.

-- Rename work_type → faculty
ALTER TABLE work_items RENAME COLUMN work_type TO faculty;

-- Add skill column (nullable — not all work items need a skill)
ALTER TABLE work_items ADD COLUMN skill TEXT;

-- Recreate the dedup index on (faculty, dedup_key)
DROP INDEX IF EXISTS idx_work_dedup;
CREATE UNIQUE INDEX idx_work_dedup ON work_items(faculty, dedup_key)
    WHERE dedup_key IS NOT NULL AND state NOT IN ('completed', 'dead', 'merged');
