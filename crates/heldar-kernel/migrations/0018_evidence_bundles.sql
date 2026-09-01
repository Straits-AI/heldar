-- Signed evidence bundles (#118).
--
-- A row per exported bundle, so an operator can list what left the appliance and a camera-scoped
-- credential sees only its own. The bundle itself is self-contained and verifiable WITHOUT this
-- table — that is the point of it — so this is an index of exports, not the evidence.
CREATE TABLE evidence_bundles (
    id            TEXT PRIMARY KEY,
    camera_id     TEXT NOT NULL,
    site_id       TEXT,
    incident_id   TEXT,
    filename      TEXT NOT NULL,
    from_time     TEXT NOT NULL,
    to_time       TEXT NOT NULL,
    size_bytes    INTEGER NOT NULL DEFAULT 0,
    -- sha256 of the bundle file as written. An operator who kept only this row can still say
    -- whether a bundle handed back to them is the one that left the box.
    sha256        TEXT NOT NULL,
    -- The manifest's own hash, which is what the signature covers.
    manifest_sha256 TEXT NOT NULL,
    key_id        TEXT NOT NULL,
    exported_by   TEXT NOT NULL,
    audit_id      TEXT,
    request_id    TEXT,
    created_at    TEXT NOT NULL
);
CREATE INDEX idx_evidence_bundles_camera ON evidence_bundles(camera_id, created_at);
