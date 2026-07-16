-- Semantic retrieval substrate (issue #38): CLIP embeddings of detection crops, one row per
-- (camera, track, stride bucket), posted by the AI worker's `embedding` task. `vec` is the raw
-- embedding as little-endian f32 bytes (`dim` entries); search is brute-force cosine in Rust
-- (no vector DB — consistent with ADR 0004). Rows follow the detections retention TTL and are
-- shed by the DB size cap before detections (derived data goes first).

CREATE TABLE IF NOT EXISTS embeddings (
    id            TEXT PRIMARY KEY,
    camera_id     TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    detection_id  TEXT,
    track_id      TEXT,
    label         TEXT,
    ts            TEXT NOT NULL,
    model         TEXT NOT NULL,
    dim           INTEGER NOT NULL,
    vec           BLOB NOT NULL,
    bbox          TEXT,
    frame_id      TEXT,
    evidence_path TEXT,
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_embeddings_cam_ts ON embeddings(camera_id, ts);
CREATE INDEX IF NOT EXISTS idx_embeddings_created ON embeddings(created_at);
-- At-least-once redelivery dedup: a retried batch re-sends the same (frame_id, track_id) rows;
-- the conflicting re-inserts are no-ops (mirrors the outbox (camera_id, frame_id) idempotency).
CREATE UNIQUE INDEX IF NOT EXISTS idx_embeddings_dedup
    ON embeddings(camera_id, frame_id, track_id)
    WHERE frame_id IS NOT NULL AND track_id IS NOT NULL;

-- Query-embedding job queue (issue #38): workers are pull-only, so a semantic search enqueues its
-- text/image query here, a worker claims it on its fast poll, computes the CLIP embedding, and
-- POSTs the vector back; the search request polls the row until done or its ~3s budget expires.
-- Rows are short-lived (pruned by retention within the hour).
CREATE TABLE IF NOT EXISTS embed_queries (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    payload     TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending',
    vec         BLOB,
    model       TEXT,
    dim         INTEGER,
    error       TEXT,
    created_at  TEXT NOT NULL,
    claimed_at  TEXT,
    claimed_by  TEXT,
    finished_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_embed_queries_status ON embed_queries(status, created_at);
