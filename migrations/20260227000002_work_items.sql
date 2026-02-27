CREATE TABLE work_items (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    pgmq_msg_id     BIGINT,
    queue_name      TEXT NOT NULL,
    work_type       TEXT NOT NULL,
    dedup_key       TEXT,
    source          TEXT NOT NULL,
    trigger_info    TEXT,
    params          JSONB NOT NULL DEFAULT '{}',
    priority        INTEGER NOT NULL DEFAULT 0,
    state           TEXT NOT NULL DEFAULT 'queued',
    merged_into     UUID REFERENCES work_items(id),
    parent_id       UUID REFERENCES work_items(id),
    attempts        INTEGER NOT NULL DEFAULT 0,
    max_attempts    INTEGER,
    outcome_data    JSONB,
    outcome_error   TEXT,
    outcome_ms      BIGINT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at     TIMESTAMPTZ
);

CREATE INDEX idx_work_dedup ON work_items(work_type, dedup_key)
    WHERE dedup_key IS NOT NULL AND state NOT IN ('completed', 'dead', 'merged');
CREATE INDEX idx_work_state ON work_items(state);
CREATE INDEX idx_work_parent ON work_items(parent_id) WHERE parent_id IS NOT NULL;
