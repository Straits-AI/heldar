-- Stage 1 observability: durable key/value state (alert notifier cursor) + an index for the
-- notifier's event scan.

CREATE TABLE IF NOT EXISTS app_state (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_severity_created ON events(severity, created_at);
