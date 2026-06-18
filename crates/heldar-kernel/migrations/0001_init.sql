-- Heldar kernel — consolidated initial schema.
--
-- This single migration is the authoritative baseline: it collapses the former 24 incremental
-- migrations (0001..0024) into one schema. Done pre-1.0 with no production deployments, so the
-- migration history was reset rather than preserved — a fresh database only (an older DB cannot be
-- upgraded across this collapse; recreate it).
--
-- The vestigial multi-tenant scaffold (the `tenants` table + `sites.tenant_id`) is intentionally
-- dropped: Heldar is single-tenant-per-deployment (each customer runs their own DVR), so a tenant
-- layer was never used. `sites` stays — a single organization with many sites is real.
--
-- Conventions: timestamps are RFC3339 UTC TEXT; booleans are INTEGER 0/1; JSON is TEXT.

CREATE TABLE sites (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    timezone    TEXT NOT NULL DEFAULT 'UTC',
    created_at  TEXT NOT NULL
);
CREATE TABLE cameras (
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
, storage_quota_bytes INTEGER, record_audio INTEGER NOT NULL DEFAULT 0, record_mode TEXT NOT NULL DEFAULT 'continuous', pre_roll_seconds INTEGER NOT NULL DEFAULT 10, post_roll_seconds INTEGER NOT NULL DEFAULT 30, mirror_enabled INTEGER NOT NULL DEFAULT 0, anr_enabled INTEGER NOT NULL DEFAULT 0, anr_replay_url_template TEXT, priority INTEGER NOT NULL DEFAULT 100);
CREATE TABLE segments (
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
, evidence_locked INTEGER NOT NULL DEFAULT 0);
CREATE INDEX idx_segments_cam_time ON segments(camera_id, start_time);
CREATE INDEX idx_segments_end ON segments(end_time);
CREATE TABLE camera_status (
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
CREATE TABLE events (
    id          TEXT PRIMARY KEY,
    camera_id   TEXT,
    site_id     TEXT,
    event_type  TEXT NOT NULL,                  -- camera_online|camera_offline|recorder_error|recording_gap|reconnect|retention_delete|disk_pressure
    severity    TEXT NOT NULL DEFAULT 'info',   -- info|warning|critical
    timestamp   TEXT NOT NULL,
    payload     TEXT NOT NULL DEFAULT '{}',     -- JSON
    created_at  TEXT NOT NULL
);
CREATE INDEX idx_events_time ON events(timestamp);
CREATE INDEX idx_events_cam ON events(camera_id, timestamp);
CREATE INDEX idx_events_severity_created ON events(severity, created_at);
CREATE TABLE ai_tasks (
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
CREATE INDEX idx_ai_tasks_camera ON ai_tasks(camera_id);
CREATE TABLE detections (
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
, frame_id TEXT);
CREATE INDEX idx_detections_cam_time ON detections(camera_id, timestamp);
CREATE INDEX idx_detections_label ON detections(label);
CREATE TABLE zones (
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
CREATE INDEX idx_zones_camera ON zones(camera_id);
CREATE TABLE zone_events (
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
CREATE INDEX idx_zone_events_cam_time ON zone_events(camera_id, timestamp);
CREATE INDEX idx_zone_events_zone ON zone_events(zone_id, timestamp);
CREATE TABLE users (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'viewer',
    display_name  TEXT,
    active        INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE TABLE sessions (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    last_used_at TEXT
);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expires ON sessions(expires_at);
CREATE TABLE api_keys (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    key_hash     TEXT NOT NULL UNIQUE,
    key_prefix   TEXT NOT NULL,                    -- leading chars, shown for identification
    role         TEXT NOT NULL DEFAULT 'integration',
    active        INTEGER NOT NULL DEFAULT 1,
    last_used_at TEXT,
    created_at   TEXT NOT NULL
);
CREATE TABLE audit_log (
    id          TEXT PRIMARY KEY,
    actor       TEXT NOT NULL,                      -- user id or 'system'
    actor_name  TEXT,
    role        TEXT,
    action      TEXT NOT NULL,
    target_type TEXT,
    target_id   TEXT,
    detail      TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL
);
CREATE INDEX idx_audit_ts ON audit_log(created_at);
CREATE INDEX idx_audit_actor ON audit_log(actor);
CREATE TABLE outbox (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    topic           TEXT NOT NULL,              -- 'detections'
    camera_id       TEXT,
    site_id         TEXT,
    frame_id        TEXT,                       -- worker-supplied per-camera idempotency key (nullable)
    task_type       TEXT,
    detection_count INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL
, fanned_out_at TEXT);
CREATE UNIQUE INDEX idx_outbox_dedup
    ON outbox(camera_id, frame_id) WHERE frame_id IS NOT NULL;
CREATE INDEX idx_outbox_created ON outbox(created_at);
CREATE INDEX idx_zone_events_ts ON zone_events(timestamp);
CREATE INDEX idx_segments_locked_end ON segments(locked, end_time);
CREATE INDEX idx_segments_evlocked_end ON segments(evidence_locked, end_time);
CREATE TABLE snapshot_schedules(
    id               TEXT PRIMARY KEY,
    camera_id        TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    interval_seconds INTEGER NOT NULL DEFAULT 300,
    enabled          INTEGER NOT NULL DEFAULT 1,
    last_fired_at    TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);
CREATE INDEX idx_snapshot_schedules_cam ON snapshot_schedules(camera_id);
CREATE TABLE snapshots(
    id          TEXT PRIMARY KEY,
    camera_id   TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    schedule_id TEXT REFERENCES snapshot_schedules(id) ON DELETE SET NULL,
    path        TEXT NOT NULL UNIQUE,
    taken_at    TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
);
CREATE INDEX idx_snapshots_cam_time ON snapshots(camera_id, taken_at);
CREATE INDEX idx_snapshots_schedule ON snapshots(schedule_id);
CREATE TABLE camera_schedules(
    id          TEXT PRIMARY KEY,
    camera_id   TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    days        TEXT NOT NULL DEFAULT '[0,1,2,3,4,5,6]',
    time_start  TEXT NOT NULL,
    time_end    TEXT NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX idx_camera_schedules_camera ON camera_schedules(camera_id);
CREATE TABLE backup_destinations(
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,
    config     TEXT NOT NULL DEFAULT '{}',
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE backup_policies(
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
CREATE TABLE backup_jobs(
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
CREATE INDEX idx_backup_jobs_policy ON backup_jobs(policy_id, created_at);
CREATE INDEX idx_backup_jobs_status ON backup_jobs(status, created_at);
CREATE TABLE camera_onvif(
    camera_id        TEXT PRIMARY KEY REFERENCES cameras(id) ON DELETE CASCADE,
    device_url       TEXT NOT NULL,
    manufacturer     TEXT,
    model            TEXT,
    firmware_version TEXT,
    serial_number    TEXT,
    hardware_id      TEXT,
    scopes           TEXT NOT NULL DEFAULT '[]',
    media_url        TEXT,
    ptz_url          TEXT,
    profile_token    TEXT,
    ptz_node_token   TEXT,
    ptz_enabled      INTEGER NOT NULL DEFAULT 0,
    probed_at        TEXT NOT NULL
);
CREATE TABLE camera_ptz_presets(
    id         TEXT PRIMARY KEY,
    camera_id  TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    token      TEXT NOT NULL,
    name       TEXT,
    fetched_at TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_ptz_presets_cam_token ON camera_ptz_presets(camera_id, token);
CREATE TABLE recording_gaps(
    id              TEXT PRIMARY KEY,
    camera_id       TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    gap_start       TEXT NOT NULL,
    gap_end         TEXT NOT NULL,
    gap_seconds     INTEGER NOT NULL,
    fill_state      TEXT NOT NULL DEFAULT 'pending',
    fill_attempts   INTEGER NOT NULL DEFAULT 0,
    last_attempt_at TEXT,
    filled_at       TEXT,
    created_at      TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_recording_gaps_cam_start ON recording_gaps(camera_id, gap_start);
CREATE INDEX idx_recording_gaps_state ON recording_gaps(fill_state, camera_id);
CREATE TABLE camera_isapi (
    camera_id          TEXT PRIMARY KEY REFERENCES cameras(id) ON DELETE CASCADE,
    device_name        TEXT,
    model              TEXT,
    firmware_version   TEXT,
    serial_number      TEXT,
    onvif_enabled      INTEGER NOT NULL DEFAULT 0,
    onvif_user_created INTEGER NOT NULL DEFAULT 0,
    time_mode          TEXT,
    ntp_server         TEXT,
    fetched_at         TEXT NOT NULL
);
CREATE TABLE webhook_subscriptions(
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    url          TEXT NOT NULL,
    event_types  TEXT NOT NULL DEFAULT '["*"]',  -- JSON array; ["*"] = all types
    min_severity TEXT NOT NULL DEFAULT 'info',    -- info|warning|critical (threshold floor)
    secret       TEXT,                            -- HMAC-SHA256 signing key (nullable); never echoed
    enabled      INTEGER NOT NULL DEFAULT 1,
    cursor_at    TEXT,                            -- delivery cursor (events.created_at); NULL = start at now
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE TABLE webhook_deliveries(
    id              TEXT PRIMARY KEY,
    subscription_id TEXT NOT NULL REFERENCES webhook_subscriptions(id) ON DELETE CASCADE,
    event_id        TEXT,
    event_type      TEXT,
    status          TEXT NOT NULL,               -- delivered|failed
    attempts        INTEGER NOT NULL DEFAULT 0,
    response_code   INTEGER,
    error           TEXT,
    created_at      TEXT NOT NULL,
    delivered_at    TEXT
);
CREATE INDEX idx_webhook_deliveries_sub ON webhook_deliveries(subscription_id, created_at);
CREATE TABLE module_registrations(
    id                TEXT PRIMARY KEY,                     -- stable plugin id; the /m/{id}/ mount + nav key
    name              TEXT NOT NULL,
    version           TEXT NOT NULL DEFAULT '',
    publisher         TEXT NOT NULL DEFAULT '',
    description       TEXT NOT NULL DEFAULT '',
    base_url          TEXT NOT NULL,                        -- sidecar origin the kernel proxies to (http(s))
    nav               TEXT NOT NULL DEFAULT '[]',           -- JSON array of {path,label,icon}
    subscribes        TEXT NOT NULL DEFAULT '["*"]',        -- JSON array of event types the sidecar receives
    role              TEXT NOT NULL DEFAULT 'integration',  -- role of the minted API key (least-priv)
    api_key_id        TEXT,                                 -- minted api_keys.id (revoked on uninstall)
    webhook_id        TEXT,                                 -- minted webhook_subscriptions.id (deleted on uninstall)
    health            TEXT NOT NULL DEFAULT 'unknown',      -- unknown|healthy|unreachable
    health_checked_at TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);
CREATE INDEX idx_segments_incident ON segments(incident_id) WHERE incident_id IS NOT NULL;
CREATE TABLE consumer_fanout (
    consumer   TEXT NOT NULL,
    camera_id  TEXT NOT NULL,
    frame_id   TEXT NOT NULL,
    fanned_at  TEXT NOT NULL,
    PRIMARY KEY (consumer, camera_id, frame_id)
);
CREATE INDEX idx_outbox_unfanned ON outbox(seq) WHERE fanned_out_at IS NULL;
