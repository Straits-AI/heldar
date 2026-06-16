-- Webhook subscriptions: the generic event-delivery substrate that SUPERSEDES the single-URL
-- alerting notifier. Each subscription is an independent at-least-once deliverer with its own cursor,
-- event-type/severity filter, optional HMAC-SHA256 secret, and a per-delivery attempt ledger.
--
-- An `event_types` value of ["*"] matches every type; otherwise it is an exact-membership set. The
-- legacy single-URL alerting webhook (app_state / HELDAR_ALERT_WEBHOOK_URL) is migrated into a
-- "Default alerts" subscription on first run of the webhooks service, so it keeps working unchanged.

CREATE TABLE IF NOT EXISTS webhook_subscriptions(
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    url          TEXT NOT NULL,
    event_types  TEXT NOT NULL DEFAULT '["*"]',  -- JSON array; ["*"] = all types
    min_severity TEXT NOT NULL DEFAULT 'info',    -- info|warning|critical (threshold floor)
    secret       TEXT,                            -- HMAC-SHA256 signing key (nullable); never echoed
    enabled      INTEGER NOT NULL DEFAULT 1,
    cursor_at    TEXT,                            -- delivery cursor (events.created_at); NULL = start at now
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS webhook_deliveries(
    id              TEXT PRIMARY KEY,
    subscription_id TEXT NOT NULL REFERENCES webhook_subscriptions(id) ON DELETE CASCADE,
    event_id        TEXT,
    event_type      TEXT,
    status          TEXT NOT NULL,               -- delivered|failed
    attempts        INTEGER NOT NULL DEFAULT 0,
    response_code   INTEGER,
    error           TEXT,
    created_at      TEXT NOT NULL,
    delivered_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_sub ON webhook_deliveries(subscription_id, created_at);
