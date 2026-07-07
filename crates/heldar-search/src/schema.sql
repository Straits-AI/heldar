-- Semantic search schema (Apache-2.0, open). Owned by this app crate, applied idempotently against the
-- shared kernel pool. Only a query log (history + accountability) — search itself reads kernel facts.

CREATE TABLE IF NOT EXISTS search_log (
    id           TEXT PRIMARY KEY,
    actor        TEXT,
    mode         TEXT NOT NULL,              -- 'nl' | 'structured'
    query_text   TEXT,                       -- the natural-language question (nl mode)
    plan         TEXT NOT NULL DEFAULT '{}', -- the structured plan that was executed (JSON)
    planner      TEXT,                       -- 'rules' | 'llm' | 'structured'
    result_count INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_search_log_created ON search_log(created_at);
