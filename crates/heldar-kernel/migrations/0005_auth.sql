-- Kernel auth + audit (RBAC is a kernel platform feature: any deployment can require login).
-- Operators, opaque bearer sessions, machine API keys, and an immutable operator/system audit log.
-- Domain-app tables (vehicles, visitor passes, watchlist, entry events) live in their own app crate's
-- schema (e.g. heldar-entry), NOT in the open kernel.

-- Operators. password_hash is an argon2id PHC string. role: admin|manager|guard|viewer|integration
CREATE TABLE IF NOT EXISTS users (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'viewer',
    display_name  TEXT,
    active        INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

-- Opaque bearer sessions. id is the SHA-256 hex of the issued token (token itself never stored).
CREATE TABLE IF NOT EXISTS sessions (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    last_used_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);

-- Machine API keys (worker ingest + external integration). key_hash is SHA-256 hex of the key.
CREATE TABLE IF NOT EXISTS api_keys (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    key_hash     TEXT NOT NULL UNIQUE,
    key_prefix   TEXT NOT NULL,                    -- leading chars, shown for identification
    role         TEXT NOT NULL DEFAULT 'integration',
    active        INTEGER NOT NULL DEFAULT 1,
    last_used_at TEXT,
    created_at   TEXT NOT NULL
);

-- Immutable audit log of operator + system actions (RBAC accountability). Append-only by contract.
-- Kernel-owned: the auth layer writes it; apps may read it for their admin reports.
CREATE TABLE IF NOT EXISTS audit_log (
    id          TEXT PRIMARY KEY,
    actor       TEXT NOT NULL,                      -- user id or 'system'
    actor_name  TEXT,
    role        TEXT,
    action      TEXT NOT NULL,
    target_type TEXT,
    target_id   TEXT,
    detail      TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_log(created_at);
CREATE INDEX IF NOT EXISTS idx_audit_actor ON audit_log(actor);
