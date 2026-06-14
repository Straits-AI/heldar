-- Per-camera recording schedule (time-of-day windows).
-- `record_mode` controls WHEN the recorder runs for a camera:
--   continuous       -- always record (default; the legacy behavior)
--   scheduled        -- record only inside an enabled camera_schedules window (this batch)
--   event            -- record on events only (event triggers wired in a later batch)
--   scheduled_event  -- record inside windows AND on events (the event part wired in a later batch)
-- A camera_schedules row defines a recurring daily window: `days` is a JSON array of weekday ints
-- (0=Mon .. 6=Sun); `time_start`/`time_end` are "HH:MM" 24h in the SERVER's LOCAL timezone
-- (chrono::Local — there is no per-camera/per-schedule timezone). When time_start > time_end the
-- window wraps past midnight (e.g. 22:00 -> 06:00); its early-morning portion is attributed to the
-- day the window started. Timestamps are RFC3339 UTC TEXT; booleans INTEGER.
ALTER TABLE cameras ADD COLUMN record_mode TEXT NOT NULL DEFAULT 'continuous';

CREATE TABLE IF NOT EXISTS camera_schedules(
    id          TEXT PRIMARY KEY,
    camera_id   TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    days        TEXT NOT NULL DEFAULT '[0,1,2,3,4,5,6]',
    time_start  TEXT NOT NULL,
    time_end    TEXT NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_camera_schedules_camera ON camera_schedules(camera_id);
