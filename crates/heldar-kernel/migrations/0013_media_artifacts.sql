-- Attribution sidecar for the FLAT media subtrees (clips, playback sessions, evidence snapshots,
-- backup archives). Those artifacts carry no camera in their filename, which is why /media/clips,
-- /media/playback, /media/snapshots and /media/archives could only ever be gated by capability —
-- any credential holding that capability read every camera's exports.
--
-- Keyed by the artifact's PATH RELATIVE TO THE MEDIA ROOT, so artifacts keep their current names and
-- locations and nothing on disk moves. For a DIRECTORY artifact the key is the directory
-- ("playback/pbs_x" covers index.m3u8, init.mp4 and every seg_*.m4s), so a scrub through a session
-- shares one row. The primary key is composite because one archive spans several cameras.
CREATE TABLE IF NOT EXISTS media_artifacts (
    path       TEXT NOT NULL,  -- "clips/clip_x.mp4" | "playback/pbs_x" | "snapshots/zoneevt_x.jpg"
    camera_id  TEXT NOT NULL,  -- | "archives/bkp_x.zip"
    kind       TEXT NOT NULL,  -- clip | playback_session | zone_evidence | embed_thumb
    created_at TEXT NOT NULL,  -- | entry_evidence | archive
    PRIMARY KEY (path, camera_id)
);

-- Retention prunes clips by mtime and cannot forget them by name, so it sweeps by (kind, created_at).
CREATE INDEX IF NOT EXISTS idx_media_artifacts_kind_created ON media_artifacts(kind, created_at);

-- Backfill what the database already knows, so existing evidence frames stay readable for scoped
-- credentials instead of falling off a cliff. substr(...,8) strips the leading "/media/" from the
-- stored URL (zones.rs and embeddings.rs both write "/media/snapshots/<file>").
INSERT OR IGNORE INTO media_artifacts (path, camera_id, kind, created_at)
    SELECT substr(evidence_path, 8), camera_id, 'zone_evidence', created_at
      FROM zone_events
     WHERE evidence_path LIKE '/media/snapshots/%';

INSERT OR IGNORE INTO media_artifacts (path, camera_id, kind, created_at)
    SELECT substr(evidence_path, 8), camera_id, 'embed_thumb', created_at
      FROM embeddings
     WHERE evidence_path LIKE '/media/snapshots/%';

-- NOT backfillable: clips. There is no clips table and no camera in clip_<uuid>.mp4 — the owning
-- camera only ever existed in the ClipResult already returned to the exporter. Pre-migration clips
-- are therefore unattributed: 403 for camera-scoped credentials, unchanged (200) for every unscoped
-- credential and every human role. Playback sessions self-heal, because they TTL out.
