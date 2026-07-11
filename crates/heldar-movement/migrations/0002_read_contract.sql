-- Published cross-app READ CONTRACT for breach_alerts (Apache-2.0, open).
--
-- Cross-app consumers (heldar-search) read this app's breach alerts through THIS VIEW, never the base
-- `breach_alerts` table directly (movement, the owner, still reads its own base table). The view is the
-- stable contract: exactly the columns peers may depend on. To rename a base column, add a NEW migration
-- that redefines this view aliasing the new column back to the contract name — the contract name stays
-- stable and consumers keep working. A rename that forgets to redefine the view is caught by
-- tests/read_contract.rs (SQLite late-binds views) in THIS crate's CI, in the same PR.
--
-- Columns are what search (query.rs breach branch) reads.
CREATE VIEW IF NOT EXISTS breach_alerts_read AS
SELECT id, created_at, camera_id, rule, subject_type, subject, zone_name, severity, evidence_path
  FROM breach_alerts;
