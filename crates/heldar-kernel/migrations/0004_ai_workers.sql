-- Liveness heartbeats for AI worker processes, so MULTIPLE workers on one node can SHARD the task set
-- (each worker analyzes only its slice) instead of every worker redoing every task and burning N× GPU for
-- 1× throughput. A worker upserts its row on each `/ai/tasks` poll (the poll IS the heartbeat); the kernel
-- treats a worker whose `last_seen` is older than the liveness TTL as gone and reassigns its tasks on the
-- next poll. Absent a `worker_id` (a single legacy worker), the kernel returns ALL tasks and this table is
-- unused — so the change is backward-compatible in both directions.
CREATE TABLE IF NOT EXISTS ai_workers (
    worker_id TEXT PRIMARY KEY,
    last_seen TEXT NOT NULL
);
