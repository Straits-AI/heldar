//! Movement-intelligence configuration (loaded from env by the composing server).

use heldar_kernel::env::parse_or;

#[derive(Clone, Debug)]
pub struct MovementConfig {
    /// How often the candidate proposer + breach engines run (seconds).
    pub engine_interval_s: u64,
    /// Lookback window each engine tick scans for new events (seconds). Enforced ≥ 2× the interval so
    /// consecutive ticks always overlap (no events dropped between ticks), and bounded above.
    pub scan_window_s: i64,
    /// Minimum fused score (0..1) at which a candidate is auto-proposed for review.
    pub min_candidate_score: f64,
    /// Zone `kind` values treated as red/breach zones by the rule engine (comma-separated).
    pub red_zone_kinds: Vec<String>,
    /// How long candidates + breach alerts are kept before retention prunes resolved/old ones.
    pub retention_days: i64,
}

impl MovementConfig {
    pub fn from_env() -> Self {
        let red = std::env::var("HELDAR_MOVEMENT_RED_ZONE_KINDS")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "restricted,red".to_string());
        let engine_interval_s = parse_or::<u64>("HELDAR_MOVEMENT_INTERVAL_S", 60).clamp(15, 600);
        // Guarantee tick overlap (≥ 2× interval) and bound the window so the TimeDelta math + scans
        // never blow up on a pathological env value.
        let scan_window_s = parse_or::<i64>("HELDAR_MOVEMENT_SCAN_WINDOW_S", 900)
            .clamp(engine_interval_s as i64 * 2, 86_400);
        MovementConfig {
            engine_interval_s,
            scan_window_s,
            min_candidate_score: parse_or::<f64>("HELDAR_MOVEMENT_MIN_SCORE", 0.5).clamp(0.0, 1.0),
            red_zone_kinds: red
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            retention_days: parse_or("HELDAR_MOVEMENT_RETENTION_DAYS", 365).clamp(1, 3650),
        }
    }
}
