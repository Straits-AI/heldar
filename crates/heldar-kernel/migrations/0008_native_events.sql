-- On-camera smart-event ingestion (issue #46): subscribe to the camera's own event stream
-- (motion / line-crossing / intrusion fired by the device's built-in detection) and drive the
-- kernel's event machinery (event log -> webhooks/email, event-mode recording triggers) from it.
--
-- `native_events_enabled` gates the kernel's per-camera alertStream consumer
-- (services/camera_events.rs). Off by default: the operator arms it per camera from the Device
-- panel once the capability probe has shown which built-in detections the device supports.

ALTER TABLE cameras ADD COLUMN native_events_enabled INTEGER NOT NULL DEFAULT 0;
