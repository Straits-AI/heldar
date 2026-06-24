-- Runtime-tunable key/value settings: operator policy (e.g. recording disk limits) that should take
-- effect without an env change + restart. Readers fall back to the static env config when a key is unset.
CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
