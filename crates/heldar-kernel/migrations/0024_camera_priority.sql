-- Per-camera AI decode priority (higher = more important; default 100). Under fps-budget pressure the
-- frame sampler now favors high-priority cameras (e.g. an ANPR gate lane) and sheds low-priority ones
-- first, instead of splitting the budget evenly and blinding cameras in arbitrary (alphabetical) order.
ALTER TABLE cameras ADD COLUMN priority INTEGER NOT NULL DEFAULT 100;
