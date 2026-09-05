-- Join a backup job to the request that started it (part of #121).
--
-- 0020 did this for audit_log and 0021 for events. Jobs were the third of the same question and the
-- one an operator is most likely to ask it about: "who started this transfer, and why is it running
-- now" is a question about a long-lived, side-effectful thing that outlives the call which asked for
-- it. `created_by` (migration 0015) already records WHICH CREDENTIAL ordered it, so revocation can
-- bite mid-flight; this records WHICH CALL, so the job joins to the audit row and the events it
-- emitted.
--
-- Nullable, and stays nullable — and here NULL carries real meaning rather than being a gap. A
-- policy-driven job is created by the scheduler, which holds no principal and serves no request, so
-- it records NULL exactly as it records no `created_by`. An on-demand archive runs inline inside its
-- request (services::backup::create_archive is awaited by the caller, never spawned), so it records
-- the id. The two are distinguishable in the table, which is the useful part: a non-NULL row was
-- asked for by somebody, a NULL row was the schedule doing its job.
ALTER TABLE backup_jobs ADD COLUMN request_id TEXT;

-- "What did request X start" is a lookup, not a scan. Partial: most rows are scheduler-created and
-- will never have one.
CREATE INDEX IF NOT EXISTS idx_backup_jobs_request ON backup_jobs(request_id)
    WHERE request_id IS NOT NULL;
