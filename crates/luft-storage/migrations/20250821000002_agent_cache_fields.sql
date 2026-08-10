-- Add AgentResultCache fields to agents table.
ALTER TABLE agents ADD COLUMN cache_key_hash  TEXT;
ALTER TABLE agents ADD COLUMN description     TEXT;
ALTER TABLE agents ADD COLUMN role            TEXT;
ALTER TABLE agents ADD COLUMN findings_json   TEXT NOT NULL DEFAULT '[]';
ALTER TABLE agents ADD COLUMN completed_at    INTEGER NOT NULL DEFAULT 0;
