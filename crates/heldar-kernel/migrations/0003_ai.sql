-- Stage 2: AI tasks (what perception to run per camera) and detections (results posted by workers).

CREATE TABLE IF NOT EXISTS ai_tasks (
    id             TEXT PRIMARY KEY,
    camera_id      TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    task_type      TEXT NOT NULL,                 -- detection|anpr|tracking|... (free-form)
    enabled        INTEGER NOT NULL DEFAULT 1,
    stream_profile TEXT NOT NULL DEFAULT 'sub',   -- sub|main (which stream to sample)
    fps            REAL NOT NULL DEFAULT 5,        -- requested sample rate (budget may reduce it)
    width          INTEGER NOT NULL DEFAULT 1280,  -- target sample width (height keeps aspect)
    config         TEXT NOT NULL DEFAULT '{}',     -- JSON: model params, zones, thresholds
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ai_tasks_camera ON ai_tasks(camera_id);

CREATE TABLE IF NOT EXISTS detections (
    id          TEXT PRIMARY KEY,
    camera_id   TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    task_type   TEXT NOT NULL,
    timestamp   TEXT NOT NULL,
    label       TEXT,
    confidence  REAL,
    bbox        TEXT,                              -- JSON [x,y,w,h], normalized 0..1
    track_id    TEXT,
    attributes  TEXT NOT NULL DEFAULT '{}',        -- JSON
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_detections_cam_time ON detections(camera_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_detections_label ON detections(label);
