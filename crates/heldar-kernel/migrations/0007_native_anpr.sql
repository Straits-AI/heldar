-- Camera-native ANPR ingestion (issue #43): let a camera's on-board plate-recognition engine feed
-- the perception pipeline instead of (or alongside) the AI worker's server-side OCR.
--
-- `native_anpr_enabled` gates the kernel's per-camera ISAPI plate poller. The poller keeps a
-- durable per-camera cursor (the device's verbatim captureTime format) so a restart resumes where
-- it left off; replays are deduped by the ingest outbox (camera_id, frame_id) idempotency.

ALTER TABLE cameras ADD COLUMN native_anpr_enabled INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS camera_native_anpr_state (
    camera_id      TEXT PRIMARY KEY REFERENCES cameras(id) ON DELETE CASCADE,
    -- Device-format captureTime of the newest ingested plate read (the poll cursor).
    cursor_time    TEXT NOT NULL DEFAULT '',
    last_event_at  TEXT,
    last_error     TEXT,
    updated_at     TEXT NOT NULL
);
