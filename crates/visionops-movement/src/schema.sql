-- Movement intelligence (proprietary) schema. Owned by this app crate, applied idempotently against
-- the shared kernel pool. Correlation/candidate data only — no legal-identity records.

-- Operator-configured directed camera adjacency: a subject leaving `from_camera` may appear at
-- `to_camera` within ~transit_seconds. Scopes cross-camera candidate matching.
CREATE TABLE IF NOT EXISTS camera_links (
    id              TEXT PRIMARY KEY,
    from_camera     TEXT NOT NULL,
    to_camera       TEXT NOT NULL,
    transit_seconds INTEGER NOT NULL DEFAULT 120,
    bidirectional   INTEGER NOT NULL DEFAULT 0,
    note            TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE(from_camera, to_camera)
);

-- Cross-camera candidate links (ReID = probabilistic correlation, NOT identity). Each links two
-- appearances (entry events or detections) with a fused score + per-signal evidence; human-reviewed.
CREATE TABLE IF NOT EXISTS movement_candidates (
    id            TEXT PRIMARY KEY,
    subject_type  TEXT NOT NULL,                  -- vehicle | person
    anchor        TEXT,                           -- normalized plate (vehicle) or '' (person)
    from_camera   TEXT, from_ref TEXT, from_time TEXT,
    to_camera     TEXT, to_ref TEXT, to_time TEXT,
    transit_seconds REAL,
    score         REAL NOT NULL DEFAULT 0,        -- 0..1 fused confidence
    signals       TEXT NOT NULL DEFAULT '{}',     -- JSON: which signals agreed + weights
    status        TEXT NOT NULL DEFAULT 'pending',-- pending | confirmed | rejected
    reviewed_by   TEXT, reviewed_at TEXT,
    created_at    TEXT NOT NULL,
    UNIQUE(subject_type, from_ref, to_ref)
);
CREATE INDEX IF NOT EXISTS idx_move_cand_status ON movement_candidates(status, score);
CREATE INDEX IF NOT EXISTS idx_move_cand_anchor ON movement_candidates(anchor);

-- Red-zone breach alerts (rule engine). One per triggering zone event; enriched with any correlated
-- subject. Worked by an operator (open → acknowledged → resolved).
CREATE TABLE IF NOT EXISTS breach_alerts (
    id            TEXT PRIMARY KEY,
    camera_id     TEXT,
    zone_id       TEXT,
    zone_name     TEXT,
    zone_event_id TEXT UNIQUE,                    -- the source zone_event (dedup key)
    rule          TEXT NOT NULL,                  -- red_zone_entry | red_zone_dwell | ...
    subject_type  TEXT,                           -- vehicle | person | unknown
    subject       TEXT,                           -- correlated plate, if any
    track_id      TEXT,
    severity      TEXT NOT NULL DEFAULT 'warning',
    status        TEXT NOT NULL DEFAULT 'open',   -- open | acknowledged | resolved
    detail        TEXT NOT NULL DEFAULT '{}',
    evidence_path TEXT,
    created_at    TEXT NOT NULL,
    resolved_by   TEXT, resolved_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_breach_status ON breach_alerts(status, created_at);
-- Plain created_at index for time-range scans (e.g. semantic search) that don't filter by status.
CREATE INDEX IF NOT EXISTS idx_breach_created ON breach_alerts(created_at);
