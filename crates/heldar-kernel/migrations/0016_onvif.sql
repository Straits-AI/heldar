-- ONVIF (Profile S MVP): per-camera device profile discovered by probing the camera's ONVIF
-- service, plus the PTZ presets fetched from it.
--
-- `camera_onvif` holds one row per camera (the result of GetDeviceInformation + GetCapabilities/
-- GetServices + GetProfiles). `device_url` is the ONVIF device service endpoint; `media_url` /
-- `ptz_url` are the discovered sub-service endpoints. `profile_token` is the media profile used for
-- streaming + PTZ; `ptz_node_token` is the PTZ node bound to that profile's PTZConfiguration.
-- `scopes` is a JSON array of ONVIF scope URIs (from WS-Discovery; '[]' when probed directly).
-- `ptz_enabled` is 1 when the device exposes a PTZ service AND the chosen profile carries a
-- PTZConfiguration. Timestamps are RFC3339 UTC TEXT; bools are INTEGER 0/1.
CREATE TABLE IF NOT EXISTS camera_onvif(
    camera_id        TEXT PRIMARY KEY REFERENCES cameras(id) ON DELETE CASCADE,
    device_url       TEXT NOT NULL,
    manufacturer     TEXT,
    model            TEXT,
    firmware_version TEXT,
    serial_number    TEXT,
    hardware_id      TEXT,
    scopes           TEXT NOT NULL DEFAULT '[]',
    media_url        TEXT,
    ptz_url          TEXT,
    profile_token    TEXT,
    ptz_node_token   TEXT,
    ptz_enabled      INTEGER NOT NULL DEFAULT 0,
    probed_at        TEXT NOT NULL
);

-- PTZ presets fetched from a camera's PTZ service (GetPresets). `token` is the device's preset
-- token; `name` is the human label (may be absent). One row per (camera, token).
CREATE TABLE IF NOT EXISTS camera_ptz_presets(
    id         TEXT PRIMARY KEY,
    camera_id  TEXT NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    token      TEXT NOT NULL,
    name       TEXT,
    fetched_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_ptz_presets_cam_token ON camera_ptz_presets(camera_id, token);
