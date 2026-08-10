-- Agent sessions: backend session metadata for resume.
CREATE TABLE agent_sessions (
    run_id               BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    agent_id             BLOB    NOT NULL,
    backend_id           TEXT,
    protocol_session_id  TEXT,
    session_id           TEXT    NOT NULL,
    status               TEXT    NOT NULL,
    updated_at           INTEGER NOT NULL,
    resumable            INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (run_id, agent_id)
);
