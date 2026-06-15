-- The retention sweeper repeatedly runs `SELECT id, path FROM segments WHERE locked = 0
-- ORDER BY end_time ASC LIMIT N`. Without a composite index SQLite must scan + sort. This index
-- serves the filter (locked) and the order (end_time) directly. Also benefits the transient
-- read-lock toggling used by clip/snapshot export.
CREATE INDEX IF NOT EXISTS idx_segments_locked_end ON segments(locked, end_time);
