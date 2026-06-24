-- Initial SQLite schema for Maestro structured persistence.
-- See docs/design/sqlite-persistence.md for rationale.

-- Runs: one row per top-level run.
CREATE TABLE runs (
    run_id              BLOB    NOT NULL PRIMARY KEY,
    task                TEXT    NOT NULL,
    status              TEXT    NOT NULL DEFAULT 'running',
    started_ts          TEXT    NOT NULL,
    finished_ts         TEXT,
    elapsed_ms          INTEGER NOT NULL DEFAULT 0,
    input_tokens        INTEGER NOT NULL DEFAULT 0,
    output_tokens       INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens   INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens  INTEGER NOT NULL DEFAULT 0,
    report              TEXT,
    script_path         TEXT,
    created_at          TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_runs_started ON runs(started_ts DESC);
CREATE INDEX idx_runs_status ON runs(status);

-- Phases: per-run phase summaries.
CREATE TABLE phases (
    run_id      BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    phase_id    INTEGER NOT NULL,
    label       TEXT    NOT NULL,
    planned     INTEGER NOT NULL DEFAULT 0,
    ok          INTEGER NOT NULL DEFAULT 0,
    failed      INTEGER NOT NULL DEFAULT 0,
    started_ts  TEXT,
    done_ts     TEXT,
    PRIMARY KEY (run_id, phase_id)
);

-- Agents: one row per (run, agent); written on AgentStarted, updated on AgentDone.
CREATE TABLE agents (
    run_id              BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    agent_id            BLOB    NOT NULL,
    phase_id            INTEGER,
    model               TEXT,
    status              TEXT    NOT NULL DEFAULT 'running',
    prompt_preview      TEXT,
    input_tokens        INTEGER NOT NULL DEFAULT 0,
    output_tokens       INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens   INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens  INTEGER NOT NULL DEFAULT 0,
    output              TEXT,
    started_ts          TEXT    NOT NULL,
    done_ts             TEXT,
    elapsed_ms          INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (run_id, agent_id)
);

CREATE INDEX idx_agents_run ON agents(run_id);
CREATE INDEX idx_agents_phase ON agents(run_id, phase_id);

-- Turns: core table for UI conversation flow. One row per ProgressDelta.
CREATE TABLE turns (
    seq                 INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id              BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    agent_id            BLOB    NOT NULL,
    phase_id            INTEGER,
    ts                  TEXT    NOT NULL,
    kind                TEXT    NOT NULL,
    role                TEXT,
    text                TEXT,
    tool_call_id        TEXT,
    name                TEXT,
    input               TEXT,
    output              TEXT,
    tool_status         TEXT,
    file_path           TEXT,
    file_op             TEXT,
    diff                TEXT,
    input_tokens        INTEGER,
    output_tokens       INTEGER,
    cache_read_tokens   INTEGER,
    cache_write_tokens  INTEGER
);

CREATE INDEX idx_turns_run_agent ON turns(run_id, agent_id, seq);
CREATE INDEX idx_turns_run ON turns(run_id, seq);
CREATE INDEX idx_turns_tool_call ON turns(tool_call_id) WHERE tool_call_id IS NOT NULL;

-- Spans: orchestration primitives (parallel / converge / workflow / pipeline).
CREATE TABLE spans (
    run_id          BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    span_id         INTEGER NOT NULL,
    kind            TEXT    NOT NULL,
    phase_id        INTEGER,
    parent_span_id  INTEGER,
    label           TEXT,
    path            TEXT,
    items           INTEGER,
    max_rounds      INTEGER,
    rounds          INTEGER,
    converged       INTEGER,
    ok              INTEGER,
    failed          INTEGER,
    result          TEXT,
    error           TEXT,
    started_ts      TEXT,
    done_ts         TEXT,
    elapsed_ms      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (run_id, span_id)
);

CREATE INDEX idx_spans_run ON spans(run_id);
CREATE INDEX idx_spans_parent ON spans(run_id, parent_span_id);

-- Findings: structured discoveries reported by agents.
CREATE TABLE findings (
    run_id      BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    agent_id    BLOB,
    phase_id    INTEGER,
    kind        TEXT    NOT NULL,
    severity    TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    detail      TEXT,
    file_path   TEXT,
    line_start  INTEGER,
    line_end    INTEGER,
    evidence    TEXT,
    data        TEXT,
    created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (run_id, agent_id, title)
);

CREATE INDEX idx_findings_run ON findings(run_id);
CREATE INDEX idx_findings_severity ON findings(run_id, severity);

-- Events: complete AgentEvent audit log. Used for replay and forensics.
CREATE TABLE events (
    seq     INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id  BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    ts      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    type    TEXT    NOT NULL,
    payload TEXT    NOT NULL
);

CREATE INDEX idx_events_run ON events(run_id, seq);
CREATE INDEX idx_events_type ON events(run_id, type);