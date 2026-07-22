-- Barrier/gate actuation (issue #44): per-lane policy for pulsing the camera's alarm/relay output
-- on a matched entry decision, plus a global kill-switch.
--
-- Actuation flows through the kernel's camera_control pulse primitive (ISAPI output ports). The
-- policy is dashboard-managed (manager+); the manual guard-open endpoint (guard+) uses the same
-- per-camera port/pulse configuration. Every actuation — auto or manual — is written to the
-- kernel event log, and manual opens additionally to the immutable audit log.

CREATE TABLE IF NOT EXISTS gate_policies (
    camera_id   TEXT PRIMARY KEY,
    -- Auto-open on a `matched` entry event (manual guard-open works regardless).
    enabled     INTEGER NOT NULL DEFAULT 0,
    -- Alarm/relay output port on the camera driving the barrier.
    output_port INTEGER NOT NULL DEFAULT 1,
    -- Relay pulse width in milliseconds.
    pulse_ms    INTEGER NOT NULL DEFAULT 1000,
    updated_at  TEXT NOT NULL
);

-- Single-row global gate settings (the kill-switch halts ALL actuation, auto and manual).
CREATE TABLE IF NOT EXISTS gate_settings (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    kill_switch INTEGER NOT NULL DEFAULT 0,
    updated_at  TEXT NOT NULL
);
INSERT OR IGNORE INTO gate_settings (id, kill_switch, updated_at)
VALUES (1, 0, '1970-01-01T00:00:00Z');
