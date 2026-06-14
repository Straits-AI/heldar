-- Backup policies (scheduled jobs) + the job ledger.
-- A policy ships a camera selection's recent footage to a destination on a fixed interval. The
-- backup scheduler creates a backup_job when a policy is due (last_run_at IS NULL OR
-- last_run_at + schedule_interval_s <= now), executing it under a concurrency semaphore + timeout.
-- `camera_ids` is a JSON array of camera ids ([] = all cameras); `lookback_hours` bounds how far back
-- the run reaches (0 = everything up to now). `incident_lock_only` restricts to evidence_locked
-- segments. backup_jobs also backs on-demand archive exports (kind='on_demand_archive', output_url set).
CREATE TABLE IF NOT EXISTS backup_policies(
    id                 TEXT PRIMARY KEY,
    name               TEXT NOT NULL,
    destination_id     TEXT NOT NULL REFERENCES backup_destinations(id) ON DELETE CASCADE,
    camera_ids         TEXT NOT NULL DEFAULT '[]',
    incident_lock_only INTEGER NOT NULL DEFAULT 0,
    schedule_interval_s INTEGER NOT NULL DEFAULT 86400,
    lookback_hours     INTEGER NOT NULL DEFAULT 0,
    last_run_at        TEXT,
    last_job_id        TEXT,
    enabled            INTEGER NOT NULL DEFAULT 1,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS backup_jobs(
    id                 TEXT PRIMARY KEY,
    policy_id          TEXT REFERENCES backup_policies(id) ON DELETE SET NULL,
    destination_id     TEXT REFERENCES backup_destinations(id) ON DELETE SET NULL,
    kind               TEXT NOT NULL DEFAULT 'policy',
    camera_ids         TEXT NOT NULL DEFAULT '[]',
    from_time          TEXT,
    to_time            TEXT,
    incident_lock_only INTEGER NOT NULL DEFAULT 0,
    status             TEXT NOT NULL DEFAULT 'pending',
    files_total        INTEGER NOT NULL DEFAULT 0,
    files_copied       INTEGER NOT NULL DEFAULT 0,
    bytes_copied       INTEGER NOT NULL DEFAULT 0,
    error              TEXT,
    output_path        TEXT,
    output_url         TEXT,
    started_at         TEXT,
    finished_at        TEXT,
    created_at         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_backup_jobs_policy ON backup_jobs(policy_id, created_at);
CREATE INDEX IF NOT EXISTS idx_backup_jobs_status ON backup_jobs(status, created_at);
