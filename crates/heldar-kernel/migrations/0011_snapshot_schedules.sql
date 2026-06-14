-- Scheduled interval snapshots + their captured frames.
-- A snapshot_schedule fires a live JPEG capture for its camera every `interval_seconds`; the
-- background scheduler advances `last_fired_at` after each fire. Captured frames are recorded in
-- `snapshots` (one row per file under snapshots_dir/{camera_id}/) and pruned by the retention
-- sweeper past HELDAR_SNAPSHOT_RETENTION_HOURS. Timestamps are RFC3339 UTC TEXT; booleans INTEGER.
CREATE TABLE IF NOT EXISTS snapshot_schedules(
    id               TEXT PRIMARY KEY,
    camera_id        TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    interval_seconds INTEGER NOT NULL DEFAULT 300,
    enabled          INTEGER NOT NULL DEFAULT 1,
    last_fired_at    TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snapshot_schedules_cam ON snapshot_schedules(camera_id);

CREATE TABLE IF NOT EXISTS snapshots(
    id          TEXT PRIMARY KEY,
    camera_id   TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    schedule_id TEXT REFERENCES snapshot_schedules(id) ON DELETE SET NULL,
    path        TEXT NOT NULL UNIQUE,
    taken_at    TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snapshots_cam_time ON snapshots(camera_id, taken_at);
CREATE INDEX IF NOT EXISTS idx_snapshots_schedule ON snapshots(schedule_id);
