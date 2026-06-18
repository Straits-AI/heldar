-- Dual / mirror recording + ANR (Automatic Network Replenishment) edge re-fill.
--
-- mirror_enabled: when HELDAR_MIRROR_RECORDINGS_DIR is set, run a SECOND ffmpeg pipeline writing
-- byte-identical segments to the mirror dir (a redundant DVR copy on a separate volume). See
-- crates/heldar-kernel/src/services/mirror.rs.
--
-- anr_enabled + anr_replay_url_template: when ANR is enabled (HELDAR_ANR_ENABLED), the background
-- ANR loop re-fetches missed footage from the camera's ONBOARD storage to fill recording gaps, by
-- recording the camera's replay stream. anr_replay_url_template is an optional per-camera replay URL
-- with {start}/{end} placeholders (Hikvision time format, e.g. 20260613T120500Z); NULL falls back to
-- the default Hikvision RTSP playback endpoint built from the camera's address+credentials. Re-fill is
-- best-effort and camera-dependent — see crates/heldar-kernel/src/services/anr.rs.
ALTER TABLE cameras ADD COLUMN mirror_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cameras ADD COLUMN anr_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cameras ADD COLUMN anr_replay_url_template TEXT;

-- A recording gap detected by the indexer (a hole > 3s between consecutive segments). The ANR loop
-- picks pending gaps and tries to fill them; fill_state is 'pending' | 'filled' | 'failed'. gap_start
-- / gap_end / last_attempt_at / filled_at / created_at are RFC3339 UTC TEXT. One row per
-- (camera_id, gap_start) — the indexer upserts ignore-on-conflict, so re-scans never duplicate a gap.
CREATE TABLE IF NOT EXISTS recording_gaps(
    id              TEXT PRIMARY KEY,
    camera_id       TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    gap_start       TEXT NOT NULL,
    gap_end         TEXT NOT NULL,
    gap_seconds     INTEGER NOT NULL,
    fill_state      TEXT NOT NULL DEFAULT 'pending',
    fill_attempts   INTEGER NOT NULL DEFAULT 0,
    last_attempt_at TEXT,
    filled_at       TEXT,
    created_at      TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_recording_gaps_cam_start ON recording_gaps(camera_id, gap_start);
CREATE INDEX IF NOT EXISTS idx_recording_gaps_state ON recording_gaps(fill_state, camera_id);
