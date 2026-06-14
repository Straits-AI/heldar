-- Durable evidence lock, distinct from the TRANSIENT `locked` column (a read-lock held by
-- clip/snapshot export and wiped at every startup by db::clear_segment_read_locks). A segment with
-- evidence_locked = 1 is pinned indefinitely: retention never deletes it (age TTL, size cap, or
-- disk floor), and its bytes count as the protected footprint when computing the deletable budget.
-- Set/cleared only by the incident API; survives restarts. incident_id (added in 0001) tags the
-- segment to a case so evidence can be grouped and reviewed.
ALTER TABLE segments ADD COLUMN evidence_locked INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_segments_evlocked_end ON segments(evidence_locked, end_time);
