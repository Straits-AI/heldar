-- Who ORDERED a backup job, so work that outlives the request can re-check them.
--
-- `services/backup::spawn_job` detaches the transfer: the API answers 202 and the copy keeps running
-- for up to HELDAR_BACKUP_JOB_TIMEOUT_S (default 3600 s), shipping recorded footage to a destination
-- that may be off the box entirely (sftp/ftp/s3 via rclone). Every scope decision about that job was
-- made once, at request time, and the job row carried no way back to the credential that made it —
-- so revoking a compromised key, or narrowing its camera list, did nothing to the bytes already in
-- flight. Revocation is a deliberate operator act meaning "this credential is compromised"; footage
-- continuing to leave the box afterwards is the failure this column exists to stop.
--
-- NULL means "no credential ordered this": the background scheduler holds no principal, and rows
-- written before this migration cannot be attributed. Both are read as authorized, which keeps the
-- scheduler and the upgrade path unchanged.
--
-- `created_by_kind` is 'api_key' | 'user' | 'system' and decides HOW `created_by` is re-checked —
-- an api key by `api_keys.active/revoked_at/expires_at` plus its CURRENT `scope_cameras`, a user by
-- `users.active`, and 'system' (auth disabled) not at all. Storing the kind rather than inferring it
-- from the id prefix means the re-check cannot be fooled by an id that looks like the other kind.
ALTER TABLE backup_jobs ADD COLUMN created_by TEXT;
ALTER TABLE backup_jobs ADD COLUMN created_by_kind TEXT;
