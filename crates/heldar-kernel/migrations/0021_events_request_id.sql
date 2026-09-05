-- Join an event to the request that caused it (part of #121).
--
-- 0020 did this for `audit_log`. Events were the other half of the same question and were left out,
-- so the chain broke at the interesting point: an operator holding a request id could see what the
-- box RECORDED it did, but not what it EMITTED as a result — which is what alerting, the dashboard
-- timeline and every webhook subscriber actually consume.
--
-- This also reaches webhook deliveries without a column of their own. `webhook_deliveries.event_id`
-- already points here, so "which request caused this webhook to fire" becomes a join rather than a
-- third copy of the same id kept in sync by hand. Denormalising it onto the delivery row would be a
-- second place for it to be wrong.
--
-- Nullable, and stays nullable. NULL means no request carried an id into this row — which is the
-- common and CORRECT case for events: a camera going offline, a disk warning, a retention sweep and
-- a recorder gap are all things the box noticed on its own. The task-local deliberately does not
-- cross `tokio::spawn` (see request_id.rs), so a background emitter records NULL rather than
-- inheriting whichever request happened to be in flight. That distinction is the useful part: a
-- non-NULL row means an operator or an integration asked for this, and a NULL row means the box did
-- it by itself.
ALTER TABLE events ADD COLUMN request_id TEXT;

-- "What did request X cause" is a lookup, not a scan. Partial so the index holds only the rows that
-- have one — most will not, and an index over a mostly-NULL column is cost without a reader.
CREATE INDEX IF NOT EXISTS idx_events_request ON events(request_id) WHERE request_id IS NOT NULL;
