-- Open-core seam (Stage 0): transactional outbox + per-frame idempotency.
--
-- The outbox is the durable, ordered (seq), replayable event log written ATOMICALLY in the same
-- transaction as each ingested detection batch. It is the appliance-side foundation for the future
-- edge→cloud uplink (L5) and out-of-process app fan-out (L4) WITHOUT running a message broker on the
-- box (the DB is the log). UNIQUE(camera_id, frame_id) makes ingest idempotent: when the AI worker
-- supplies a per-camera monotonic frame_id, a duplicate redelivery is a no-op — it neither double-
-- inserts detections nor re-fires the consumer side effects (ANPR votes, zone state) that an
-- at-least-once transport would otherwise corrupt.

ALTER TABLE detections ADD COLUMN frame_id TEXT;

CREATE TABLE IF NOT EXISTS outbox (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    topic           TEXT NOT NULL,              -- 'detections'
    camera_id       TEXT,
    site_id         TEXT,
    frame_id        TEXT,                       -- worker-supplied per-camera idempotency key (nullable)
    task_type       TEXT,
    detection_count INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL
);

-- Partial unique index: dedups (camera_id, frame_id) when a frame_id is present; multiple NULLs are
-- allowed (no dedup when the worker does not supply a frame_id — e.g. the dependency-light client).
CREATE UNIQUE INDEX IF NOT EXISTS idx_outbox_dedup
    ON outbox(camera_id, frame_id) WHERE frame_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_outbox_created ON outbox(created_at);
