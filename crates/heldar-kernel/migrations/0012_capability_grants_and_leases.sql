-- Capability-scoped machine credentials (Stage A) plus the storage Stage B needs (leases, embed-query
-- claims, provenance forensics). Both stages share ONE migration deliberately: Stage B adds no
-- migration, so the two can never race on a version number.
--
-- EVERY `ALTER TABLE ... ADD COLUMN` below uses a CONSTANT default (or none), so SQLite applies it as an
-- O(1) metadata-only change. That matters: `detections` is the largest table on a live box and a
-- rewriting ALTER would stall the recorder behind SQLite's single writer for minutes.
--
-- There is NO backfill, and that is the back-compat contract:
--   * api_keys.capabilities  NULL      = "legacy key" -> role expansion (see auth::legacy_caps).
--   * api_keys.scope_kind    'all'     = unrestricted, exactly today's reach.
--   * detections.provenance  'client'  = every pre-existing row is UNTRUSTED. Never 'camera_native'.

-- ---------------------------------------------------------------------------------------------------
-- 1. Capability grants + scope + lifecycle on api_keys.
-- ---------------------------------------------------------------------------------------------------
-- JSON array of capability slugs, e.g. ["ai:ingest","ai:frames"]. NULL = legacy role expansion.
ALTER TABLE api_keys ADD COLUMN capabilities TEXT;
-- 'all' | 'cameras'. Constant default keeps this O(1) and keeps every existing key unrestricted.
ALTER TABLE api_keys ADD COLUMN scope_kind TEXT NOT NULL DEFAULT 'all';
-- JSON array of camera ids, honoured only when scope_kind = 'cameras'.
ALTER TABLE api_keys ADD COLUMN scope_cameras TEXT;
-- RFC3339. NULL = never expires (today's behaviour).
ALTER TABLE api_keys ADD COLUMN expires_at TEXT;
-- RFC3339. NULL = live. SOFT revoke: the row survives so audit_log entries keep resolving, which a
-- hard DELETE (the only revocation available before this) destroys.
ALTER TABLE api_keys ADD COLUMN revoked_at TEXT;

-- ---------------------------------------------------------------------------------------------------
-- 2. Per-TASK worker leases (Stage B). One row per task, renewed ~once a minute — NOT per frame.
-- ---------------------------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ai_task_leases (
    task_id     TEXT PRIMARY KEY REFERENCES ai_tasks(id) ON DELETE CASCADE,
    lease_id    TEXT NOT NULL,
    api_key_id  TEXT NOT NULL,
    worker_id   TEXT NOT NULL,
    camera_id   TEXT NOT NULL,
    task_type   TEXT NOT NULL,
    acquired_at TEXT NOT NULL,
    renewed_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL
);
-- Expiry is a PREDICATE at claim time, not a reaper task: nothing new competes with the recorder for
-- SQLite's single writer.
CREATE INDEX IF NOT EXISTS idx_ai_task_leases_expiry ON ai_task_leases(expires_at);
CREATE INDEX IF NOT EXISTS idx_ai_task_leases_holder ON ai_task_leases(api_key_id, worker_id);

-- ---------------------------------------------------------------------------------------------------
-- 3. Embed-query claim ownership + lease expiry (Stage B).
-- ---------------------------------------------------------------------------------------------------
-- Which API key claimed the query. Result submission is checked against it, so worker B cannot poison
-- the vector worker A was asked for.
ALTER TABLE embed_queries ADD COLUMN claimed_by_key TEXT;
-- Without this, nothing ever flips 'claimed' back and a crashed claimant wedges the row until the
-- waiting search 503s.
ALTER TABLE embed_queries ADD COLUMN lease_expires_at TEXT;

-- ---------------------------------------------------------------------------------------------------
-- 4. Bind a heartbeating worker id to the credential that registered it (graft G4).
-- ---------------------------------------------------------------------------------------------------
-- NULL = registered before this migration; the conditional upsert treats it as adoptable once.
ALTER TABLE ai_workers ADD COLUMN api_key_id TEXT;

-- ---------------------------------------------------------------------------------------------------
-- 5. SQL-queryable provenance forensics (graft G5).
-- ---------------------------------------------------------------------------------------------------
-- 'client' (pre-existing / untrusted), 'worker', or 'kernel:<producer>'. The authoritative value that
-- consumers read still rides in the rewritten `attributes` blob; these columns exist so an incident
-- responder can answer "which credential produced this?" in SQL instead of JSON-scraping.
ALTER TABLE detections ADD COLUMN provenance TEXT NOT NULL DEFAULT 'client';
ALTER TABLE outbox ADD COLUMN provenance TEXT NOT NULL DEFAULT 'client';
-- api_key id (or 'system' / a kernel producer name) that produced the batch. NULL for legacy rows.
ALTER TABLE outbox ADD COLUMN produced_by TEXT;
