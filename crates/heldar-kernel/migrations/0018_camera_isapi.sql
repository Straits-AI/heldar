-- Camera configuration (HikVision ISAPI): per-camera device + integration state captured by the
-- camera-config service (GET /ISAPI/System/deviceInfo, /System/Network/Integrate, /System/time).
--
-- One row per camera. `onvif_enabled` mirrors the device's ISAPI<ONVIF><enable> toggle; the kernel
-- flips it on so the camera answers the ONVIF probe. `onvif_user_created` records whether the kernel
-- has provisioned a dedicated ONVIF user on the device. `time_mode` is the clock source (manual|NTP)
-- and `ntp_server` the configured NTP host. Timestamps are RFC3339 UTC TEXT; bools are INTEGER 0/1.
CREATE TABLE IF NOT EXISTS camera_isapi (
    camera_id          TEXT PRIMARY KEY REFERENCES cameras(id) ON DELETE CASCADE,
    device_name        TEXT,
    model              TEXT,
    firmware_version   TEXT,
    serial_number      TEXT,
    onvif_enabled      INTEGER NOT NULL DEFAULT 0,
    onvif_user_created INTEGER NOT NULL DEFAULT 0,
    time_mode          TEXT,
    ntp_server         TEXT,
    fetched_at         TEXT NOT NULL
);
