-- Add run_dir column to runs table for directory-name-based lookups.
ALTER TABLE runs ADD COLUMN run_dir TEXT;
CREATE INDEX IF NOT EXISTS idx_runs_run_dir ON runs(run_dir);
