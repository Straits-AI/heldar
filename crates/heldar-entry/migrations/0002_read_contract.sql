-- Published cross-app READ CONTRACT for entry_events (Apache-2.0, open).
--
-- Other app crates (heldar-movement, heldar-search) read this app's events through THIS VIEW, never the
-- base `entry_events` table directly. The view is the stable contract: it exposes exactly the columns
-- peers may depend on. To rename a base column, add a NEW migration that redefines this view aliasing the
-- new column back to the contract name (e.g. `plate_text AS plate`) — the contract name stays stable and
-- every consumer keeps working with zero consumer-side change. A base rename that forgets to redefine the
-- view is caught by tests/read_contract.rs (SQLite late-binds views, so the contract SELECT fails to
-- prepare) in THIS crate's CI, in the same PR — instead of at runtime in a distant consumer.
--
-- Columns are the union of what movement (reid.rs self-join + plate history, breach.rs) and search
-- (query.rs) read. Note: JSON columns (subject, evidence) are still opaque here — the view guards column
-- IDENTITY, not the shape/meaning of the JSON inside them.
CREATE VIEW IF NOT EXISTS entry_events_read AS
SELECT id, timestamp, camera_id, event_type, plate, subject, auth_status, evidence, direction, track_id
  FROM entry_events;
