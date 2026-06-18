-- Heldar Core — Stage 0 media kernel schema.
-- Timestamps are RFC3339 UTC TEXT. Booleans are INTEGER 0/1. JSON is TEXT.

CREATE TABLE IF NOT EXISTS tenants (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sites (
    id          TEXT PRIMARY KEY,
    tenant_id   TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    timezone    TEXT NOT NULL DEFAULT 'UTC',
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS cameras (
    id               TEXT PRIMARY KEY,                       -- slug, e.g. gate_a_01
    site_id          TEXT REFERENCES sites(id) ON DELETE SET NULL,
    name             TEXT NOT NULL,
    vendor           TEXT NOT NULL DEFAULT 'generic',        -- hikvision|dahua|onvif|generic
    model            TEXT,
    address          TEXT,                                   -- host/ip
    rtsp_port        INTEGER NOT NULL DEFAULT 554,
    username         TEXT,
    password         TEXT,                                   -- Stage 0: plaintext; move to secret store later
    main_stream_url  TEXT,                                   -- explicit override; else built from vendor template
    sub_stream_url   TEXT,
    record_stream    TEXT NOT NULL DEFAULT 'main',           -- main|sub
    codec            TEXT,
    resolution_main  TEXT,
    resolution_sub   TEXT,
    fps_main         INTEGER,
    fps_sub          INTEGER,
    capabilities     TEXT NOT NULL DEFAULT '{}',             -- JSON
    record_enabled   INTEGER NOT NULL DEFAULT 1,
    segment_seconds  INTEGER NOT NULL DEFAULT 60,
    retention_hours  INTEGER NOT NULL DEFAULT 24,
    enabled          INTEGER NOT NULL DEFAULT 1,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);

-- Timeline index: one row per recorded segment file.
CREATE TABLE IF NOT EXISTS segments (
    id            TEXT PRIMARY KEY,
    camera_id     TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    path          TEXT NOT NULL UNIQUE,
    start_time    TEXT NOT NULL,
    end_time      TEXT NOT NULL,
    duration_s    REAL NOT NULL,
    codec         TEXT,
    width         INTEGER,
    height        INTEGER,
    size_bytes    INTEGER NOT NULL DEFAULT 0,
    container     TEXT NOT NULL DEFAULT 'mp4',
    locked        INTEGER NOT NULL DEFAULT 0,
    incident_id   TEXT,
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_segments_cam_time ON segments(camera_id, start_time);
CREATE INDEX IF NOT EXISTS idx_segments_end ON segments(end_time);

-- Current live status per camera (single row per camera, upserted).
CREATE TABLE IF NOT EXISTS camera_status (
    camera_id        TEXT PRIMARY KEY REFERENCES cameras(id) ON DELETE CASCADE,
    state            TEXT NOT NULL DEFAULT 'unknown',   -- disabled|connecting|recording|offline|error|unknown
    last_segment_at  TEXT,
    last_started_at  TEXT,
    reconnect_count  INTEGER NOT NULL DEFAULT 0,
    segments_written INTEGER NOT NULL DEFAULT 0,
    fps_observed     REAL,
    bitrate_kbps     REAL,
    last_error       TEXT,
    recorder_pid     INTEGER,
    updated_at       TEXT NOT NULL
);

-- Generic event log: camera lifecycle now, AI/intelligence events later.
CREATE TABLE IF NOT EXISTS events (
    id          TEXT PRIMARY KEY,
    camera_id   TEXT,
    site_id     TEXT,
    event_type  TEXT NOT NULL,                  -- camera_online|camera_offline|recorder_error|recording_gap|reconnect|retention_delete|disk_pressure
    severity    TEXT NOT NULL DEFAULT 'info',   -- info|warning|critical
    timestamp   TEXT NOT NULL,
    payload     TEXT NOT NULL DEFAULT '{}',     -- JSON
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_time ON events(timestamp);
CREATE INDEX IF NOT EXISTS idx_events_cam ON events(camera_id, timestamp);
