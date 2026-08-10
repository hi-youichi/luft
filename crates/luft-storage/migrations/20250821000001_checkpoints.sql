-- Checkpoints: one row per run, mirrors RunCheckpoint fields.
CREATE TABLE checkpoints (
    run_id              BLOB    NOT NULL PRIMARY KEY REFERENCES runs(run_id) ON DELETE CASCADE,
    status              TEXT    NOT NULL DEFAULT 'running',
    current_phase       INTEGER NOT NULL DEFAULT 0,
    total_tokens        INTEGER NOT NULL DEFAULT 0,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    workflow_meta       TEXT,
    started_agent_ids   TEXT    NOT NULL DEFAULT '[]'
);

CREATE INDEX idx_checkpoints_status ON checkpoints(status);
