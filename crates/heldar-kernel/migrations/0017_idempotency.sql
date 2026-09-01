-- Replay protection for mutations, so a retried request cannot do the work twice.
--
-- Automation retries after a timeout, and a relay can lose a response to an operation the box already
-- completed. Without this, "create a clip" or "trigger a backup" runs again — the client sees one
-- failure and the box does two units of work, or creates two rows.
--
-- Scoped to the PRINCIPAL as well as the key: a key is a client-chosen string, so without the
-- principal in the identity one caller could replay another's result by guessing it. That is the
-- difference between deduplication and an information leak.
CREATE TABLE IF NOT EXISTS idempotency_keys (
    -- Client-chosen key, unique only within one principal.
    key           TEXT NOT NULL,
    -- Which credential made the original call. Part of the identity, never just a label.
    principal_id  TEXT NOT NULL,
    method        TEXT NOT NULL,
    path          TEXT NOT NULL,
    -- SHA-256 of the request body. Reusing a key with a DIFFERENT body is a client bug worth
    -- reporting (409) rather than silently returning the wrong cached answer.
    request_hash  TEXT NOT NULL,
    -- NULL while the first request is still running: a concurrent duplicate sees the row exists but
    -- has no answer yet, and is told to retry rather than served a half-finished result.
    status_code   INTEGER,
    body          TEXT,
    created_at    TEXT NOT NULL,
    PRIMARY KEY (principal_id, key, method, path)
);

-- The prune scans by age.
CREATE INDEX IF NOT EXISTS idx_idempotency_created ON idempotency_keys(created_at);
