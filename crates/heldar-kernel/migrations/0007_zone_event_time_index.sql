-- A plain timestamp index on zone_events, for time-range scans that do not filter by camera (the
-- existing idx_zone_events_cam_time is (camera_id, timestamp) and can't serve a pure-time range).
-- Used by cross-cutting consumers like semantic search and time-windowed analytics rollups.
CREATE INDEX IF NOT EXISTS idx_zone_events_ts ON zone_events(timestamp);
