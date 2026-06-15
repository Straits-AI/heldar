//! Health monitor: downgrades cameras that claim to be recording but have stopped producing
//! segments (a stalled-but-connected stream), emitting an event on the transition.

use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::SqlitePool;
use tokio::process::Command;

use crate::config::Config;
use crate::repo;

/// SMART CLI (smartmontools). Looked up on PATH; a missing binary degrades to a one-time log + skip.
const SMARTCTL_BIN: &str = "smartctl";

pub async fn run(pool: SqlitePool, cfg: Arc<Config>) {
    let mut tick = tokio::time::interval(Duration::from_secs(cfg.health_interval_s.max(5)));
    // Disk-health (SMART/RAID) runs on its own slower cadence inside this same loop, so a busy
    // appliance is not probed for drive health every few seconds.
    let smart_interval = Duration::from_secs(cfg.smart_check_interval_s.max(30));
    let mut last_disk_check: Option<Instant> = None;
    loop {
        tick.tick().await;
        if let Err(e) = check_once(&pool).await {
            tracing::error!(error = %e, "health: check failed");
        }
        let due = last_disk_check
            .map(|t| t.elapsed() >= smart_interval)
            .unwrap_or(true);
        if due {
            last_disk_check = Some(Instant::now());
            check_disk_health(&pool, &cfg).await;
        }
    }
}

/// (camera_id, last_segment_at, last_started_at, segment_seconds)
type StaleRow = (String, Option<DateTime<Utc>>, Option<DateTime<Utc>>, i64);

async fn check_once(pool: &SqlitePool) -> anyhow::Result<()> {
    let rows: Vec<StaleRow> = sqlx::query_as(
        "SELECT cs.camera_id, cs.last_segment_at, cs.last_started_at, c.segment_seconds
         FROM camera_status cs
         JOIN cameras c ON c.id = cs.camera_id
         WHERE cs.state = 'recording'",
    )
    .fetch_all(pool)
    .await?;

    let now = Utc::now();
    for (camera_id, last_seg, last_start, seg_s) in rows {
        let threshold = (seg_s.max(10) * 3).max(30);
        let seg_age = last_seg.map(|t| (now - t).num_seconds());
        let start_age = last_start.map(|t| (now - t).num_seconds());

        let recent_segment = seg_age.map(|a| a <= threshold).unwrap_or(false);
        let recently_started = start_age.map(|a| a <= threshold).unwrap_or(false);
        if recent_segment || recently_started {
            continue;
        }

        let msg = format!("no segments for >{threshold}s while recording");
        let _ = repo::set_state(pool, &camera_id, "error", Some(&msg)).await;
        let _ = repo::log_event(
            pool,
            Some(&camera_id),
            "recorder_error",
            "warning",
            json!({ "reason": "stale", "threshold_seconds": threshold, "last_segment_age_s": seg_age }),
        )
        .await;
        tracing::warn!(%camera_id, threshold, "health: camera stale, marked error");
    }
    Ok(())
}

/// SMART/RAID disk-health pass: opt-in drive self-assessment (`smartctl -H`) and Linux md/RAID
/// array-state monitoring. Both degrade gracefully when their inputs are absent (missing smartctl is
/// logged once then skipped; no `/proc/mdstat` is a no-op), so the build and tests never require them.
async fn check_disk_health(pool: &SqlitePool, cfg: &Config) {
    if cfg.smart_check_enabled {
        if smartctl_available().await {
            for dev in &cfg.smart_devices {
                check_smart_device(pool, dev).await;
            }
        } else if !SMARTCTL_MISSING_WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "health: HELDAR_SMART_CHECK_ENABLED set but `smartctl` is not on PATH; skipping \
                 SMART checks (install smartmontools)"
            );
        }
    }
    #[cfg(target_os = "linux")]
    if cfg.mdstat_check_enabled {
        check_mdstat(pool).await;
    }
}

/// One-shot guard so the "smartctl missing" warning is logged once per process, not every interval.
static SMARTCTL_MISSING_WARNED: AtomicBool = AtomicBool::new(false);

/// Whether `smartctl` is runnable on PATH (so a missing binary degrades to a skip, not a panic).
async fn smartctl_available() -> bool {
    Command::new(SMARTCTL_BIN)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `smartctl -H <dev>` and emit a `disk_smart_warning` event unless the drive reports healthy
/// (PASSED / OK). A FAILED result or an unreadable/missing device both warn.
async fn check_smart_device(pool: &SqlitePool, dev: &str) {
    let out = Command::new(SMARTCTL_BIN)
        .arg("-H")
        .arg(dev)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;
    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if smart_is_healthy(&stdout) {
                return;
            }
            let detail = stdout
                .lines()
                .find(|l| l.contains("health") || l.contains("Health") || l.contains("SMART"))
                .unwrap_or("")
                .trim()
                .to_string();
            let _ = repo::log_event(
                pool,
                None,
                "disk_smart_warning",
                "warning",
                json!({ "device": dev, "detail": detail, "exit_ok": o.status.success() }),
            )
            .await;
            tracing::warn!(device = %dev, "health: SMART self-assessment did not report PASSED");
        }
        Err(e) => {
            let _ = repo::log_event(
                pool,
                None,
                "disk_smart_warning",
                "warning",
                json!({ "device": dev, "detail": format!("smartctl could not run: {e}") }),
            )
            .await;
            tracing::warn!(device = %dev, error = %e, "health: smartctl invocation failed");
        }
    }
}

/// A SMART health summary is healthy only when it positively reports PASSED/OK and never FAILED.
fn smart_is_healthy(stdout: &str) -> bool {
    !stdout.contains("FAILED") && (stdout.contains("PASSED") || stdout.contains("OK"))
}

/// Read `/proc/mdstat` and emit a `raid_degraded` event for each array showing a down member.
#[cfg(target_os = "linux")]
async fn check_mdstat(pool: &SqlitePool) {
    let contents = match tokio::fs::read_to_string("/proc/mdstat").await {
        Ok(c) => c,
        // No md subsystem on this host — nothing to monitor.
        Err(_) => return,
    };
    for name in mdstat_degraded(&contents) {
        let _ = repo::log_event(
            pool,
            None,
            "raid_degraded",
            "critical",
            json!({ "array": name, "source": "/proc/mdstat" }),
        )
        .await;
        tracing::warn!(array = %name, "health: RAID array degraded");
    }
}

/// Parse `/proc/mdstat` and return the names of arrays with a down member. An array's per-disk state
/// map is a bracketed token of only `U`/`_` on the status line (e.g. `[U_]`); any `_` means degraded.
fn mdstat_degraded(contents: &str) -> Vec<String> {
    let mut degraded = Vec::new();
    let mut current: Option<String> = None;
    for line in contents.lines() {
        // Array header lines start at column 0 with the device name, e.g. "md0 : active raid1 ...".
        if line.starts_with("md") {
            current = line.split([' ', ':']).next().map(|s| s.to_string());
            continue;
        }
        if let Some(name) = &current {
            if line_has_down_member(line) {
                degraded.push(name.clone());
                current = None; // one verdict per array
            }
        }
    }
    degraded
}

/// Whether a status line carries a `[U.._..]` map (only `U`/`_`) with at least one down (`_`) member.
fn line_has_down_member(line: &str) -> bool {
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        if let Some(close) = after.find(']') {
            let inner = &after[..close];
            if !inner.is_empty()
                && inner.chars().all(|c| c == 'U' || c == '_')
                && inner.contains('_')
            {
                return true;
            }
            rest = &after[close + 1..];
        } else {
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_health_parsing() {
        assert!(smart_is_healthy(
            "SMART overall-health self-assessment test result: PASSED"
        ));
        assert!(smart_is_healthy("SMART Health Status: OK"));
        assert!(!smart_is_healthy(
            "SMART overall-health self-assessment test result: FAILED!"
        ));
        // Missing/unreadable device output has no positive health line.
        assert!(!smart_is_healthy("Smartctl open device: /dev/sdz failed"));
    }

    #[test]
    fn mdstat_flags_degraded_arrays_only() {
        let healthy = "\
Personalities : [raid1]
md0 : active raid1 sdb1[1] sda1[0]
      976630336 blocks super 1.2 [2/2] [UU]

unused devices: <none>
";
        assert!(mdstat_degraded(healthy).is_empty());

        let degraded = "\
Personalities : [raid1] [raid6]
md0 : active raid1 sdb1[1] sda1[0]
      976630336 blocks super 1.2 [2/1] [U_]
md1 : active raid6 sdc1[0] sdd1[1] sde1[2] sdf1[3]
      3906248704 blocks super 1.2 level 6, 512k chunk, algorithm 2 [4/4] [UUUU]

unused devices: <none>
";
        assert_eq!(mdstat_degraded(degraded), vec!["md0".to_string()]);
    }

    #[test]
    fn down_member_detection_ignores_disk_index_brackets() {
        // The header line's [0]/[1] disk-index brackets must not be read as a state map.
        assert!(!line_has_down_member("md0 : active raid1 sdb1[1] sda1[0]"));
        assert!(line_has_down_member(
            "      976630336 blocks super 1.2 [2/1] [U_]"
        ));
        assert!(!line_has_down_member(
            "      976630336 blocks super 1.2 [2/2] [UU]"
        ));
    }
}
