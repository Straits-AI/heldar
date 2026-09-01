-- Backfill `media_artifacts` for the gate evidence frames this box recorded BEFORE the media guard
-- existed.
--
-- Kernel migration 0013 backfilled the two evidence producers it could see from the kernel side —
-- `zone_events` and `embeddings` — but `entry_events` lives in this app crate and was missed. Both
-- write a byte-identical flat frame into the same `snapshots/` directory, so the result was a pure
-- false deny: on an upgraded box a camera-scoped credential got 403 on its OWN pre-upgrade gate
-- evidence while the zone frame beside it returned 200. Fresh events are attributed at write time by
-- `anpr.rs::copy_evidence`; only history needed carrying across.
--
-- The key MUST be what `media_scope::artifact_key` derives from the served URL — `snapshots/<file>`.
-- Storing the URL itself writes a row the guard looks up under a different key and never finds, which
-- is the same 403 with an extra row to explain it. `substr(...,8)` drops the leading `/media/`,
-- exactly as 0013 does for zone evidence.
--
-- `INSERT OR IGNORE` because 0013's zone/embedding rows share the flat directory and the primary key
-- is (path, camera_id): re-running or overlapping keys must never fail the upgrade.
--
-- Two filters that are not incidental:
-- - `camera_id IS NOT NULL` — a guard-recorded manual check-in has no lane, and `media_artifacts`
--   requires one. Attributing it to anything would be a fabrication.
-- - the `json_valid` test sits inside a CASE rather than beside the others in the WHERE, because AND
--   terms have no guaranteed evaluation order and `json_extract` RAISES on a malformed blob, which
--   would abort the migration and brick the upgrade over a single bad row.
INSERT OR IGNORE INTO media_artifacts (path, camera_id, kind, created_at)
    SELECT substr(snapshot_path, 8), camera_id, 'entry_evidence', created_at
      FROM (
            SELECT camera_id,
                   created_at,
                   CASE WHEN json_valid(evidence)
                        THEN json_extract(evidence, '$.snapshot_path')
                   END AS snapshot_path
              FROM entry_events
             WHERE camera_id IS NOT NULL
              AND camera_id <> ''
           )
     WHERE snapshot_path LIKE '/media/snapshots/%';
