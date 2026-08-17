-- The camera an audit row is ABOUT, promoted from free-form JSON into a FILTERABLE COLUMN.
--
-- Camera scope on `GET /api/v1/audit` could only ever be expressed against `target_id`, so it masked
-- the rows whose `target_type` is 'camera' and let every other row through untouched. But the owning
-- camera is routinely carried in the free-form `detail` blob under some OTHER target_type — zones,
-- ai_task, camera_schedule, snapshot_schedule and recording_gap all write `detail.camera_id` — so
-- `GET /api/v1/audit?limit=5000` handed a camera-scoped manager the fleet roster plus which cameras
-- carry zones, AI tasks and schedules. `detail` is `Json<Value>` with no schema and new call sites
-- add keys freely: it cannot be a scope boundary. A column can, and an index makes it cheap.
--
-- NULL means "fleet-level, or about no camera at all" and is HIDDEN from a camera-scoped reader.
-- Audit is a manager+ surface where a missing row costs an accountability question and an extra row
-- costs the roster, so the fail-closed default is the correct one.
--
-- `crate::auth::audit` is the single writer and now derives this on every insert, so no call site has
-- to remember and future call sites get it for free.
ALTER TABLE audit_log ADD COLUMN subject_camera_id TEXT;

-- The scoped read adds `subject_camera_id IN (…)` to a query already ordered by `created_at DESC`;
-- without this the 5000-row page degrades to a scan of the whole log.
CREATE INDEX idx_audit_subject_camera ON audit_log(subject_camera_id);

-- Backfill 1: rows that already named their camera in `target_id` (gate policy edits, manual gate
-- opens, camera registry writes). Their subject was never in doubt — only the filter was.
--
-- `'*'` is excluded to match `auth::subject_camera`: the bulk device-config write files a fleet-wide
-- act under target_type 'camera' with `'*'` for the target, and taken literally that is a camera
-- named `*` whose holder would inherit every bulk row ever written. History has to classify the same
-- way the writer now does, or the same row means two things either side of the upgrade.
UPDATE audit_log
   SET subject_camera_id = target_id
 WHERE target_type = 'camera'
   AND target_id IS NOT NULL
   AND target_id <> ''
   AND target_id <> '*';

-- Backfill 2: the leak itself — one camera named in `detail` under a non-camera target_type. Only a
-- STRING `camera_id` counts, which is what every such writer emits.
--
-- Deliberately NOT backfilled: multi-camera rows (`detail.camera_ids` on an archive export,
-- `detail.scope_cameras` on an API key mint). They stay NULL and are read as fleet-level, because an
-- export spanning four lanes is a fleet-level act — picking its first element would both mislabel the
-- row and disclose the other three to whoever happens to hold that one lane.
--
-- The `json_valid` test sits inside a CASE rather than beside the others in the WHERE: AND terms have
-- no guaranteed evaluation order, and `json_type` on a malformed blob RAISES, which would abort the
-- migration and brick the upgrade over one hand-edited row. CASE evaluates its branch lazily.
--
-- `<> ''` matters: json_type('{"camera_id":""}') is 'text', so without it a pre-upgrade row would
-- store '' where the live writer (`auth::subject_camera`, which filters empties) stores NULL. Since
-- '' IS NOT NULL passes the read gate, history would classify one way and new rows another — the
-- exact property this migration's header promises to preserve.
UPDATE audit_log
   SET subject_camera_id = json_extract(detail, '$.camera_id')
 WHERE subject_camera_id IS NULL
   AND CASE WHEN json_valid(detail) THEN json_type(detail, '$.camera_id') END = 'text'
   AND json_extract(detail, '$.camera_id') <> '';
