-- Event/motion-triggered recording: pre-roll / post-roll for `event` and `scheduled_event` cameras.
-- post_roll_seconds is the trigger window: a trigger (zone event or manual API) keeps the recorder
-- writing until now + post_roll_seconds; repeated triggers extend the window. pre_roll_seconds is the
-- footage desired BEFORE the trigger; the kernel keeps no always-on ring buffer for idle event
-- cameras, so it is honored only from recent completed segments that already exist (e.g. a
-- scheduled_event window in progress) — see crates/heldar-kernel/src/services/recorder.rs. Triggers
-- are evaluated against the SERVER's wall clock. Both are non-negative; clamped in the API handlers
-- (pre 0..300, post 0..3600).
ALTER TABLE cameras ADD COLUMN pre_roll_seconds INTEGER NOT NULL DEFAULT 10;
ALTER TABLE cameras ADD COLUMN post_roll_seconds INTEGER NOT NULL DEFAULT 30;
