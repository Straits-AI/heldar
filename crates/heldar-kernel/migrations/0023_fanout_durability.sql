-- Durable perception fan-out. Consumers (zones / ANPR / movement) are driven AFTER a detection batch
-- commits; a process crash in that window previously dropped the notification entirely. `fanned_out_at`
-- marks a batch whose fan-out completed; a background drainer replays any row still NULL after a crash.
-- `consumer_fanout` makes that replay safe: at-most-once per (consumer, camera_id, frame_id), so a
-- consumer is never invoked twice for the same frame even if the batch is replayed.
ALTER TABLE outbox ADD COLUMN fanned_out_at TEXT;

-- Treat every pre-existing batch as already fanned out, so enabling the drainer never replays history.
UPDATE outbox SET fanned_out_at = created_at WHERE fanned_out_at IS NULL;

CREATE TABLE IF NOT EXISTS consumer_fanout (
    consumer   TEXT NOT NULL,
    camera_id  TEXT NOT NULL,
    frame_id   TEXT NOT NULL,
    fanned_at  TEXT NOT NULL,
    PRIMARY KEY (consumer, camera_id, frame_id)
);

-- The drainer's hot lookup: outbox batches whose fan-out has not completed.
CREATE INDEX IF NOT EXISTS idx_outbox_unfanned ON outbox(seq) WHERE fanned_out_at IS NULL;
