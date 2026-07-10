-- Add retry_count to agents table for tracking retry attempts.
ALTER TABLE agents ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
