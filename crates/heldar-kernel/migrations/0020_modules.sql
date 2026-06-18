-- Runtime-registered sidecar plugins (Phase B of the plugin platform).
--
-- A sidecar is an out-of-process service (a separate process/container) that extends Heldar without
-- being compiled into the binary. On register the kernel does three things and records their ids here
-- so uninstall can reverse them exactly:
--   1. mints a scoped API key (api_keys) the sidecar uses to call kernel APIs back,
--   2. creates a webhook subscription (webhook_subscriptions) that feeds it events it subscribes to,
--   3. reverse-proxies /m/{id}/* to the sidecar's base_url (its own UI + API, mounted single-origin).
--
-- Compiled-in modules (entry/movement/search/...) are NOT stored here — they live in code and are
-- served from AppState.modules. GET /api/v1/modules merges both so the dashboard sees one list.
CREATE TABLE IF NOT EXISTS module_registrations(
    id                TEXT PRIMARY KEY,                     -- stable plugin id; the /m/{id}/ mount + nav key
    name              TEXT NOT NULL,
    version           TEXT NOT NULL DEFAULT '',
    publisher         TEXT NOT NULL DEFAULT '',
    description       TEXT NOT NULL DEFAULT '',
    base_url          TEXT NOT NULL,                        -- sidecar origin the kernel proxies to (http(s))
    nav               TEXT NOT NULL DEFAULT '[]',           -- JSON array of {path,label,icon}
    subscribes        TEXT NOT NULL DEFAULT '["*"]',        -- JSON array of event types the sidecar receives
    role              TEXT NOT NULL DEFAULT 'integration',  -- role of the minted API key (least-priv)
    api_key_id        TEXT,                                 -- minted api_keys.id (revoked on uninstall)
    webhook_id        TEXT,                                 -- minted webhook_subscriptions.id (deleted on uninstall)
    health            TEXT NOT NULL DEFAULT 'unknown',      -- unknown|healthy|unreachable
    health_checked_at TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);
