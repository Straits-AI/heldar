-- Zone occupancy (issue #40): the zone engine maintains a live per-zone count of tracks currently
-- inside (server-side aggregate), upserted only when the count changes. `updated_at` lets readers
-- judge staleness (in-memory state resets on restart and repopulates as tracks are re-observed).

CREATE TABLE IF NOT EXISTS zone_occupancy (
    zone_id    TEXT PRIMARY KEY REFERENCES zones(id) ON DELETE CASCADE,
    camera_id  TEXT NOT NULL,
    count      INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_zone_occupancy_camera ON zone_occupancy(camera_id);
