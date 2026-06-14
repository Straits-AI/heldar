-- Per-camera storage quota + optional audio recording.
-- storage_quota_bytes is nullable: NULL means no per-camera cap (only the global size cap and
-- disk-free floor apply). When set, the retention sweeper keeps that camera's deletable footprint
-- within the quota, pruning its oldest unlocked segments (evidence-locked footage is protected).
-- record_audio toggles audio pass-through in the recorder's ffmpeg command (default off: video only).
ALTER TABLE cameras ADD COLUMN storage_quota_bytes INTEGER;            -- nullable, NULL = no per-camera cap
ALTER TABLE cameras ADD COLUMN record_audio INTEGER NOT NULL DEFAULT 0;
