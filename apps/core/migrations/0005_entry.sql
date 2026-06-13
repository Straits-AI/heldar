-- Stage 4: Campus Entry app (VisionOps Entry).
-- RBAC (users/sessions/api keys), registered vehicles, visitor passes, watchlist,
-- canonical entry/exit events (memo §8.1), and an immutable audit log.

-- ---- RBAC ----------------------------------------------------------------

-- Operators. password_hash is an argon2id PHC string. role: admin|manager|guard|viewer|integration
CREATE TABLE IF NOT EXISTS users (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'viewer',
    display_name  TEXT,
    active        INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

-- Opaque bearer sessions. id is the SHA-256 hex of the issued token (token itself never stored).
CREATE TABLE IF NOT EXISTS sessions (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    last_used_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);

-- Machine API keys (worker ingest + external integration). key_hash is SHA-256 hex of the key.
CREATE TABLE IF NOT EXISTS api_keys (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    key_hash     TEXT NOT NULL UNIQUE,
    key_prefix   TEXT NOT NULL,                    -- leading chars, shown for identification
    role         TEXT NOT NULL DEFAULT 'integration',
    active        INTEGER NOT NULL DEFAULT 1,
    last_used_at TEXT,
    created_at   TEXT NOT NULL
);

-- ---- Registry ------------------------------------------------------------

-- Registered vehicles (the "allow" anchor). plate_norm is uppercased, alphanumeric-only.
CREATE TABLE IF NOT EXISTS vehicles (
    id           TEXT PRIMARY KEY,
    plate        TEXT NOT NULL,                    -- as entered (display)
    plate_norm   TEXT NOT NULL UNIQUE,             -- normalized lookup key
    owner_name   TEXT,
    owner_type   TEXT NOT NULL DEFAULT 'visitor',  -- student|staff|resident|contractor|visitor
    owner_ref    TEXT,                             -- student/staff id
    site_id      TEXT,
    vehicle_type TEXT,
    make         TEXT,
    model        TEXT,
    color        TEXT,
    notes        TEXT,
    active       INTEGER NOT NULL DEFAULT 1,
    valid_from   TEXT,
    valid_until  TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_vehicles_plate ON vehicles(plate_norm);

-- Visitor pre-registration / pass.
CREATE TABLE IF NOT EXISTS visitor_passes (
    id             TEXT PRIMARY KEY,
    code           TEXT NOT NULL UNIQUE,           -- short human pass code
    visitor_name   TEXT NOT NULL,
    phone          TEXT,
    company        TEXT,
    host           TEXT,                           -- person/department being visited
    purpose        TEXT,
    plate          TEXT,
    plate_norm     TEXT,
    vehicle_desc   TEXT,
    site_id        TEXT,
    valid_from     TEXT NOT NULL,
    valid_until    TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'active', -- active|checked_in|checked_out|expired|revoked
    checked_in_at  TEXT,
    checked_out_at TEXT,
    created_by     TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_passes_plate ON visitor_passes(plate_norm);
CREATE INDEX IF NOT EXISTS idx_passes_status ON visitor_passes(status);

-- Watchlist (block / vip / alert plates).
CREATE TABLE IF NOT EXISTS watchlist (
    id          TEXT PRIMARY KEY,
    plate       TEXT NOT NULL,
    plate_norm  TEXT NOT NULL UNIQUE,
    kind        TEXT NOT NULL DEFAULT 'block',     -- block|vip|alert
    reason      TEXT,
    severity    TEXT NOT NULL DEFAULT 'warning',   -- info|warning|critical
    active      INTEGER NOT NULL DEFAULT 1,
    created_by  TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_watchlist_plate ON watchlist(plate_norm);

-- ---- Events --------------------------------------------------------------

-- Canonical entry/exit event (memo §8.1). subject/authorization/evidence/workflow/audit are JSON;
-- plate / auth_status / workflow_status are denormalized columns for fast querying & reports.
-- No camera FK: events outlive a camera deletion for audit integrity (like zone_events).
CREATE TABLE IF NOT EXISTS entry_events (
    id               TEXT PRIMARY KEY,
    site_id          TEXT,
    camera_id        TEXT,
    event_type       TEXT NOT NULL,                  -- vehicle_entry|vehicle_exit|visitor_checkin|visitor_checkout
    timestamp        TEXT NOT NULL,
    direction        TEXT NOT NULL DEFAULT 'unknown',-- inbound|outbound|unknown
    plate            TEXT,                           -- normalized plate (denormalized for query)
    plate_confidence REAL,
    subject          TEXT NOT NULL DEFAULT '{}',     -- JSON: type/plate/plate_confidence/vehicle_type/color/make_model
    authorization    TEXT NOT NULL DEFAULT '{}',     -- JSON: {status, source, pass_id, vehicle_id, ...}
    auth_status      TEXT NOT NULL DEFAULT 'unmatched', -- matched|exception|unmatched|blocked (denormalized)
    evidence         TEXT NOT NULL DEFAULT '{}',     -- JSON: {snapshot_path, clip_id, recording_segment_ids}
    workflow_status  TEXT NOT NULL DEFAULT 'pending',-- pending|confirmed|rejected|auto
    workflow         TEXT NOT NULL DEFAULT '{}',     -- JSON: {assigned_to, resolved_by, resolved_at, note}
    audit            TEXT NOT NULL DEFAULT '{}',     -- JSON: {created_by, model_versions}
    track_id         TEXT,
    created_at       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_entry_events_ts ON entry_events(timestamp);
CREATE INDEX IF NOT EXISTS idx_entry_events_plate ON entry_events(plate);
CREATE INDEX IF NOT EXISTS idx_entry_events_auth ON entry_events(auth_status);
CREATE INDEX IF NOT EXISTS idx_entry_events_wf ON entry_events(workflow_status);

-- Immutable audit log of operator + system actions (RBAC accountability). Append-only by contract.
CREATE TABLE IF NOT EXISTS audit_log (
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
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_log(created_at);
CREATE INDEX IF NOT EXISTS idx_audit_actor ON audit_log(actor);
