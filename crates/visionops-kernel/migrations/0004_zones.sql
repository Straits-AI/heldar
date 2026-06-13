-- Stage 3: zones (polygon regions per camera) and zone events (enter/exit/dwell of tracked objects).

CREATE TABLE IF NOT EXISTS zones (
    id            TEXT PRIMARY KEY,
    camera_id     TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL DEFAULT 'region',  -- region | restricted | count | ...
    polygon       TEXT NOT NULL,                    -- JSON [[x,y],...] normalized 0..1
    dwell_seconds REAL NOT NULL DEFAULT 0,          -- >0 => emit a dwell event past this threshold
    labels        TEXT NOT NULL DEFAULT '[]',       -- JSON: detection labels that count (empty = all)
    severity      TEXT NOT NULL DEFAULT 'info',     -- severity of emitted events (info|warning|critical)
    config        TEXT NOT NULL DEFAULT '{}',
    enabled       INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_zones_camera ON zones(camera_id);

CREATE TABLE IF NOT EXISTS zone_events (
    id            TEXT PRIMARY KEY,
    camera_id     TEXT NOT NULL,
    zone_id       TEXT NOT NULL,
    zone_name     TEXT NOT NULL,
    track_id      TEXT,
    event_type    TEXT NOT NULL,                    -- enter | exit | dwell
    label         TEXT,                             -- detection label that triggered it
    timestamp     TEXT NOT NULL,
    dwell_seconds REAL,
    evidence_path TEXT,                             -- copied sampled frame, served under /media/snapshots
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_zone_events_cam_time ON zone_events(camera_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_zone_events_zone ON zone_events(zone_id, timestamp);
