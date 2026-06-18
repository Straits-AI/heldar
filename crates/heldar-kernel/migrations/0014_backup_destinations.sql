-- Backup subsystem: where archived footage is shipped to.
-- A destination is a named transfer target. `kind` selects the transport and `config` is a
-- kind-specific JSON blob (credentials live here, masked on read by BackupDestinationView):
--   local           -> {"path": "/mnt/nas/heldar"}
--   sftp | ftp      -> {"host","port","user","pass","path"}
--   s3              -> {"bucket","prefix","access_key","secret_key","endpoint","region"}
-- Local destinations copy via std fs (NAS mounts); the rest shell out to rclone (HELDAR_RCLONE_BIN),
-- which degrades to a job error if rclone is not installed. Timestamps are RFC3339 UTC TEXT; bool INTEGER.
CREATE TABLE IF NOT EXISTS backup_destinations(
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,
    config     TEXT NOT NULL DEFAULT '{}',
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
