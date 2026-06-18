-- Index incident-tagged segments so the incidents roll-up (GROUP BY incident_id) and the
-- per-incident segment lookup are index-driven instead of a full `segments` scan. Partial index:
-- only rows actually under an incident hold are indexed, so it stays tiny.
CREATE INDEX IF NOT EXISTS idx_segments_incident ON segments(incident_id) WHERE incident_id IS NOT NULL;
