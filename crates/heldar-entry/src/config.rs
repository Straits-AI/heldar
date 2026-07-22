//! The access-control app's own configuration, loaded from the environment by the composing server. The open
//! kernel does not carry any entry-app tuning knobs.

use heldar_kernel::env::parse_or;

#[derive(Clone, Debug)]
pub struct EntryConfig {
    /// Minimum reads agreeing on a track's winning plate before the ANPR engine commits an entry
    /// event (temporal voting). Lower = faster but noisier; higher = more accurate but more latency.
    pub anpr_min_votes: u32,
    /// How long entry events (+ their evidence frames) are kept before this app's retention prunes them.
    pub entry_retention_days: i64,
}

impl EntryConfig {
    pub fn from_env() -> Self {
        EntryConfig {
            anpr_min_votes: parse_or::<u32>("HELDAR_ANPR_MIN_VOTES", 3).clamp(1, 50),
            entry_retention_days: parse_or("HELDAR_ENTRY_RETENTION_DAYS", 365),
        }
    }
}
