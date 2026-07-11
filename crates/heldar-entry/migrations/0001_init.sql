-- Access-control schema (Apache-2.0, open). Owned by this app crate, applied idempotently on startup against
-- the shared kernel pool (single-tenant-per-deployment). No camera FK: events outlive a camera delete
-- for audit integrity (like zone_events). Plate is the primary anchor; vehicle attributes are
-- secondary verification only.

-- Registered vehicles (the "allow" anchor). plate_norm is uppercased, alphanumeric-only.
CREATE TABLE IF NOT EXISTS vehicles (
    id           TEXT PRIMARY KEY,
    plate        TEXT NOT NULL,
    plate_norm   TEXT NOT NULL UNIQUE,
    owner_name   TEXT,
    owner_type   TEXT NOT NULL DEFAULT 'visitor',
    owner_ref    TEXT,
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
    code           TEXT NOT NULL UNIQUE,
    visitor_name   TEXT NOT NULL,
    phone          TEXT,
    company        TEXT,
    host           TEXT,
    purpose        TEXT,
    plate          TEXT,
    plate_norm     TEXT,
    vehicle_desc   TEXT,
    site_id        TEXT,
    valid_from     TEXT NOT NULL,
    valid_until    TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'active',
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
    kind        TEXT NOT NULL DEFAULT 'block',
    reason      TEXT,
    severity    TEXT NOT NULL DEFAULT 'warning',
    active      INTEGER NOT NULL DEFAULT 1,
    created_by  TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_watchlist_plate ON watchlist(plate_norm);

-- Canonical entry/exit event. subject/authorization/evidence/workflow/audit are JSON;
-- plate / auth_status / workflow_status are denormalized columns for fast querying & reports.
CREATE TABLE IF NOT EXISTS entry_events (
    id               TEXT PRIMARY KEY,
    site_id          TEXT,
    camera_id        TEXT,
    event_type       TEXT NOT NULL,
    timestamp        TEXT NOT NULL,
    direction        TEXT NOT NULL DEFAULT 'unknown',
    plate            TEXT,
    plate_confidence REAL,
    subject          TEXT NOT NULL DEFAULT '{}',
    authorization    TEXT NOT NULL DEFAULT '{}',
    auth_status      TEXT NOT NULL DEFAULT 'unmatched',
    evidence         TEXT NOT NULL DEFAULT '{}',
    workflow_status  TEXT NOT NULL DEFAULT 'pending',
    workflow         TEXT NOT NULL DEFAULT '{}',
    audit            TEXT NOT NULL DEFAULT '{}',
    track_id         TEXT,
    created_at       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_entry_events_ts ON entry_events(timestamp);
CREATE INDEX IF NOT EXISTS idx_entry_events_plate ON entry_events(plate);
CREATE INDEX IF NOT EXISTS idx_entry_events_auth ON entry_events(auth_status);
CREATE INDEX IF NOT EXISTS idx_entry_events_wf ON entry_events(workflow_status);
