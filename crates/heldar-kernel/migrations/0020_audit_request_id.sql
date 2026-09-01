-- Join an audit entry to the request that caused it (#169, part of #121).
--
-- `request_id` already reaches the response header, the tracing span and the evidence manifest. It
-- did not reach here, so the join only worked in one direction: an evidence bundle could point at an
-- audit row, and the audit row could not point back at the request. An operator holding a request id
-- from a client's bug report had no way to find what the box actually did.
--
-- Nullable, and stays nullable. NULL means "no request carried an id into this row": every row
-- already in the table gets it, because the column is added to a live one and an id that was never
-- recorded cannot be backfilled. It would mean the same for an act with no request behind it, though
-- nothing background audits today — and if that changes, inventing an id there would be worse than
-- the gap.
ALTER TABLE audit_log ADD COLUMN request_id TEXT;

-- The question this column exists to answer is "what did request X do", which is a lookup, not a
-- scan. Partial so the index holds only rows that have one.
CREATE INDEX IF NOT EXISTS idx_audit_request ON audit_log(request_id) WHERE request_id IS NOT NULL;
