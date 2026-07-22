-- Per-camera "keep the live stream warm" flag. A warm camera's H.264 preview publisher (the kernel-owned
-- live transcode, services/live_publisher.rs) runs persistently instead of on-demand, so live view starts
-- instantly — the product replacement for hand-rolled warming scripts on the box. Operator-tunable from
-- the dashboard (principle 4: day-to-day operator settings live in the UI).
ALTER TABLE cameras ADD COLUMN live_warm INTEGER NOT NULL DEFAULT 0;
