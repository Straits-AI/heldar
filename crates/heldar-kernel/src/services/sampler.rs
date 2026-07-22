//! AI frame sampler (Stage 2): for each (camera, stream_profile) that has an enabled AI task, decode
//! that stream at a budgeted frame rate and write the latest frame to `frames/<cam>/latest_<profile>.jpg`
//! (atomic rename, so readers never see a torn JPEG). AI workers pull frames on their own cadence.
//! A global FPS budget is shared across samplers, and the number of concurrent decoders is capped, so
//! adding AI cameras degrades gracefully instead of overloading the host (backpressure). AI workers
//! never touch RTSP directly — they consume sampled frames + post detections back.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use sqlx::SqlitePool;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use crate::camera_url;
use crate::config::Config;
use crate::models::Camera;
use crate::repo;

const STDERR_TAIL_CAP: usize = 8192;
const MIN_FPS: f64 = 0.5;

/// Map a (camera, profile) pair to a stable sampler key + frame filename.
fn sampler_key(camera_id: &str, profile: &str) -> String {
    format!("{camera_id}:{profile}")
}
fn frame_filename(profile: &str) -> String {
    format!("latest_{profile}.jpg")
}

struct SamplerTask {
    stop: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SamplerInfo {
    pub camera_id: String,
    pub stream_profile: String,
    pub state: String,
    pub fps: f64,
    /// Effective decode width — may be below the requested width when the resolution ladder
    /// stepped this camera down to keep it running under budget pressure.
    pub width: i64,
}

/// Owns and supervises the per-(camera,profile) frame samplers.
pub struct SamplerManager {
    pool: SqlitePool,
    cfg: Arc<Config>,
    tasks: Mutex<HashMap<String, SamplerTask>>,
    info: Mutex<HashMap<String, SamplerInfo>>,
    rebalance_lock: Mutex<()>,
}

impl SamplerManager {
    pub fn new(pool: SqlitePool, cfg: Arc<Config>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            cfg,
            tasks: Mutex::new(HashMap::new()),
            info: Mutex::new(HashMap::new()),
            rebalance_lock: Mutex::new(()),
        })
    }

    pub async fn start_all(self: &Arc<Self>) {
        self.rebalance().await;
    }

    /// React to AI-task / camera changes: recompute the budget and (re)start samplers.
    pub async fn reconcile(self: &Arc<Self>) {
        self.rebalance().await;
    }

    /// Per-(camera,profile) sampler status (state + effective fps).
    pub async fn statuses(&self) -> Vec<SamplerInfo> {
        let mut v: Vec<SamplerInfo> = self.info.lock().await.values().cloned().collect();
        v.sort_by(|a, b| {
            (a.camera_id.as_str(), a.stream_profile.as_str())
                .cmp(&(b.camera_id.as_str(), b.stream_profile.as_str()))
        });
        v
    }

    /// Stop, recompute the active set + per-camera fps budget, and restart all samplers. Serialized
    /// by `rebalance_lock` so concurrent AI-task edits cannot race into overlapping ffmpegs.
    async fn rebalance(self: &Arc<Self>) {
        let _guard = self.rebalance_lock.lock().await;

        let ids: Vec<String> = { self.tasks.lock().await.keys().cloned().collect() };
        for id in ids {
            self.stop(&id).await;
        }
        self.info.lock().await.clear();

        if !self.cfg.ai_enabled {
            return;
        }

        // Each (camera, stream_profile) with at least one enabled task, with its max fps + width.
        let rows: Vec<(String, String, f64, i64, i64)> = sqlx::query_as(
            "SELECT c.id, t.stream_profile, MAX(t.fps) AS fps, MAX(t.width) AS width, c.priority
             FROM cameras c JOIN ai_tasks t ON t.camera_id = c.id
             WHERE c.enabled = 1 AND t.enabled = 1
             GROUP BY c.id, t.stream_profile
             ORDER BY c.priority DESC, c.id, t.stream_profile",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        if rows.is_empty() {
            return;
        }

        let budget = self.cfg.ai_max_total_fps.max(1.0);
        // Cap concurrent decoders so total cost cannot exceed the budget even at the ladder floor.
        let max_samplers = (budget / ladder_floor_cost()).floor().max(1.0) as usize;
        // Priority-aware allocation: rows are ordered priority DESC, so high-priority cameras (e.g. an
        // ANPR gate lane) get their requested fps/width first. Under pressure the RESOLUTION LADDER
        // steps lower-priority cameras down (floor fps at reduced width — a cheaper frame) before any
        // camera is shed to 0 — degraded sight beats blindness.
        let want: Vec<(f64, i64)> = rows.iter().map(|r| (r.2, r.3)).collect();
        let alloc = allocate(&want, budget, max_samplers);
        let shed = alloc.iter().filter(|(fps, _)| *fps <= 0.0).count();
        let downgraded = alloc
            .iter()
            .zip(want.iter())
            .filter(|((fps, w), (_, wq))| *fps > 0.0 && w < wq)
            .count();
        if shed > 0 {
            tracing::warn!(
                requested = rows.len(),
                shed,
                "sampler: AI budget exhausted even at the ladder floor; lowest-priority cameras will not be sampled"
            );
        }
        if downgraded > 0 {
            tracing::info!(
                downgraded,
                "sampler: resolution ladder stepped lower-priority cameras down to stay in budget"
            );
        }
        tracing::info!(
            samplers = alloc.iter().filter(|(fps, _)| *fps > 0.0).count(),
            budget,
            "sampler: rebalancing AI frame budget by priority"
        );

        for (i, (cam, profile, _max_fps, _width, _priority)) in rows.into_iter().enumerate() {
            let (fps, width) = alloc[i];
            if fps > 0.0 {
                self.spawn(cam, profile, fps, width).await;
            } else {
                self.set_info(&cam, &profile, "budget_exhausted", 0.0, 0)
                    .await;
            }
        }
    }

    async fn stop(self: &Arc<Self>, key: &str) {
        let task = { self.tasks.lock().await.remove(key) };
        if let Some(task) = task {
            let _ = task.stop.send(true);
            let mut handle = task.handle;
            if tokio::time::timeout(Duration::from_secs(8), &mut handle)
                .await
                .is_err()
            {
                tracing::warn!(key, "sampler: task did not stop within 8s; aborting");
                handle.abort();
                let _ = handle.await;
            }
        }
    }

    pub async fn shutdown(self: &Arc<Self>) {
        // Hold the rebalance lock so an in-flight reconcile cannot re-spawn after we stop.
        let _guard = self.rebalance_lock.lock().await;
        let ids: Vec<String> = { self.tasks.lock().await.keys().cloned().collect() };
        for id in ids {
            self.stop(&id).await;
        }
    }

    async fn spawn(self: &Arc<Self>, camera_id: String, profile: String, fps: f64, width: i64) {
        let key = sampler_key(&camera_id, &profile);
        let (tx, rx) = watch::channel(false);
        let mut tasks = self.tasks.lock().await;
        let me = self.clone();
        let handle = tokio::spawn(async move {
            me.supervise(camera_id, profile, fps, width, rx).await;
        });
        if let Some(old) = tasks.insert(key, SamplerTask { stop: tx, handle }) {
            let _ = old.stop.send(true);
            old.handle.abort();
        }
    }

    async fn set_info(&self, camera_id: &str, profile: &str, state: &str, fps: f64, width: i64) {
        self.info.lock().await.insert(
            sampler_key(camera_id, profile),
            SamplerInfo {
                camera_id: camera_id.to_string(),
                stream_profile: profile.to_string(),
                state: state.to_string(),
                fps,
                width,
            },
        );
    }

    /// Remove this sampler's own task + info entry (on a self-initiated exit).
    async fn cleanup_self(&self, key: &str) {
        self.tasks.lock().await.remove(key);
        self.info.lock().await.remove(key);
    }

    async fn supervise(
        self: Arc<Self>,
        camera_id: String,
        profile: String,
        fps: f64,
        width: i64,
        mut stop: watch::Receiver<bool>,
    ) {
        let key = sampler_key(&camera_id, &profile);
        let mut backoff: u64 = 1;
        loop {
            if *stop.borrow() {
                self.set_info(&camera_id, &profile, "stopped", fps, width)
                    .await;
                return;
            }
            let cam = match sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
                .bind(&camera_id)
                .fetch_optional(&self.pool)
                .await
            {
                Ok(Some(c)) if c.enabled => c,
                Ok(_) => {
                    // Camera deleted or disabled: clean up our own slot and exit.
                    self.cleanup_self(&key).await;
                    return;
                }
                Err(e) => {
                    tracing::error!(%camera_id, error = %e, "sampler: failed to load camera");
                    if sleep_or_stop(&mut stop, 10).await {
                        return;
                    }
                    continue;
                }
            };

            let Some(url) =
                camera_url::stream_url(&cam, &profile).or_else(|| camera_url::record_url(&cam))
            else {
                self.set_info(&camera_id, &profile, "error", fps, width)
                    .await;
                if sleep_or_stop(&mut stop, 30).await {
                    return;
                }
                continue;
            };

            let dir = self.cfg.camera_frames_dir(&camera_id);
            if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                tracing::error!(%camera_id, error = %e, "sampler: cannot create frames dir");
            }
            let latest = dir.join(frame_filename(&profile));
            let vf = format!("fps={fps},scale={width}:-2");
            self.set_info(&camera_id, &profile, "connecting", fps, width)
                .await;
            tracing::info!(%camera_id, %profile, fps, width, url = %camera_url::mask_url(&url), "sampler: starting");

            let mut child = match Command::new(&self.cfg.ffmpeg_bin)
                .kill_on_drop(true)
                .args(["-nostdin", "-hide_banner", "-loglevel", "warning"])
                .args(["-rtsp_transport", "tcp"])
                .args(["-timeout", "15000000"])
                .args(["-i", &url])
                .args(["-an", "-vf", &vf, "-q:v", "5"])
                // atomic_writing makes ffmpeg write to a temp file and rename, so a worker reading
                // the frame never sees a half-written JPEG.
                .args(["-f", "image2", "-update", "1", "-atomic_writing", "1", "-y"])
                .arg(&latest)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(%camera_id, "sampler: spawn ffmpeg failed: {e}");
                    self.set_info(&camera_id, &profile, "error", fps, width)
                        .await;
                    if sleep_or_stop(&mut stop, 15).await {
                        return;
                    }
                    continue;
                }
            };
            self.set_info(&camera_id, &profile, "sampling", fps, width)
                .await;
            let started = Instant::now();

            let stderr = child.stderr.take();
            let stderr_task = tokio::spawn(async move {
                let mut tail: Vec<u8> = Vec::new();
                if let Some(mut s) = stderr {
                    let mut chunk = [0u8; 4096];
                    loop {
                        match s.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                tail.extend_from_slice(&chunk[..n]);
                                if tail.len() > STDERR_TAIL_CAP {
                                    let excess = tail.len() - STDERR_TAIL_CAP;
                                    tail.drain(0..excess);
                                }
                            }
                        }
                    }
                }
                tail
            });

            tokio::select! {
                status = child.wait() => {
                    let tail = String::from_utf8_lossy(&stderr_task.await.unwrap_or_default()).trim().to_string();
                    let masked = camera_url::mask_url(&tail);
                    tracing::warn!(%camera_id, %profile, status = ?status.ok().and_then(|s| s.code()), tail = %masked, "sampler: ffmpeg exited");
                    self.set_info(&camera_id, &profile, "offline", fps, width).await;
                    let _ = repo::log_event(&self.pool, Some(&camera_id), "sampler_offline", "warning",
                        json!({ "profile": profile, "detail": masked })).await;
                    // Reset backoff after a healthy run (>30s); otherwise grow it (exponential up to
                    // 30s) so a persistently-failing camera doesn't hot-loop ffmpeg restarts. Mirrors
                    // the recorder so a camera that flaps then recovers retries promptly.
                    backoff = if started.elapsed().as_secs() > 30 { 1 } else { (backoff * 2).min(30) };
                    if sleep_or_stop(&mut stop, backoff).await {
                        return;
                    }
                }
                _ = stop.changed() => {
                    let _ = child.kill().await;
                    self.set_info(&camera_id, &profile, "stopped", fps, width).await;
                    return;
                }
            }
        }
    }
}

async fn sleep_or_stop(stop: &mut watch::Receiver<bool>, secs: u64) -> bool {
    if *stop.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(secs)) => *stop.borrow(),
        _ = stop.changed() => *stop.borrow(),
    }
}

/// The resolution ladder: fractions of the requested width a pressured camera steps down through
/// before being shed. Inference cost scales ~quadratically with linear resolution, so a half-width
/// frame is budgeted at a quarter of a full frame — the budget stays an honest inference-load proxy.
const LADDER_STEPS: &[f64] = &[1.0, 0.75, 0.5];
/// Never scale below this width (detection quality collapses; matches the task width floor).
const MIN_LADDER_WIDTH: i64 = 320;

/// Budget cost of running `fps` at `width` when `width_req` was requested (quadratic in the
/// downscale ratio; full width costs exactly `fps`).
fn frame_cost(fps: f64, width: i64, width_req: i64) -> f64 {
    let ratio = (width as f64 / width_req.max(1) as f64).clamp(0.0, 1.0);
    fps * ratio * ratio
}

/// The cheapest admission the ladder allows (floor fps at the deepest width step) — what the
/// greedy pass reserves per still-unallocated camera so pressure degrades instead of shedding.
fn ladder_floor_cost() -> f64 {
    let last = *LADDER_STEPS.last().unwrap_or(&1.0);
    MIN_FPS * last * last
}

/// Allocate the global AI budget across `want` (each camera's requested `(fps, width)`), which
/// MUST be ordered priority-high-first. Returns granted `(fps, width)` per camera (`fps == 0.0` =
/// shed).
///
/// Two-tier greedy, priority-first:
/// 1. Admission count: at most `k = budget / ladder_floor_cost` cameras run (and never more than
///    `max_samplers`) — anything beyond `k` is shed UPFRONT, so scarcity sheds from the BOTTOM of
///    the priority order (a reserve-based loop could otherwise starve the leader).
/// 2. Each admitted camera takes its full fps at full width when that fits after reserving a
///    FULL-WIDTH floor (`MIN_FPS`) for every admitted camera behind it — identical to the old
///    allocator when the budget covers full-width floors. Only when it doesn't does the camera
///    walk the resolution ladder (floor fps at 100% → 75% → 50% width, never below
///    `MIN_LADDER_WIDTH`) against the cheaper ladder-floor reserve — degraded sight, not
///    blindness. A camera whose deepest affordable rung still doesn't fit is shed.
fn allocate(want: &[(f64, i64)], budget: f64, max_samplers: usize) -> Vec<(f64, i64)> {
    let admissible = (budget / ladder_floor_cost()).floor() as usize;
    let run = want.len().min(max_samplers).min(admissible.max(1));
    let mut out = vec![(0.0, 0); want.len()];
    let mut remaining = budget;
    for (i, &(fps_req, width_req)) in want.iter().enumerate().take(run) {
        let others_after = (run - i - 1) as f64;
        // Tier 1: full width, reserving full-width floors for the rest (the pre-ladder behavior).
        let headroom_full = remaining - MIN_FPS * others_after;
        if headroom_full >= MIN_FPS {
            let grant = fps_req.min(headroom_full).max(MIN_FPS);
            out[i] = (grant, width_req);
            remaining -= grant;
            continue;
        }
        // Tier 2: the ladder, reserving only the cheapest admission for the rest.
        let headroom = (remaining - ladder_floor_cost() * others_after).max(0.0);
        for step in LADDER_STEPS {
            let width = (((width_req as f64) * step).round() as i64).max(MIN_LADDER_WIDTH);
            let cost = frame_cost(MIN_FPS, width, width_req);
            if cost <= headroom + 1e-9 {
                out[i] = (MIN_FPS, width);
                remaining -= cost;
                break;
            }
        }
        // No rung fit: shed (stays (0.0, 0)).
    }
    out
}

impl SamplerManager {
    /// Filesystem path of the latest sampled frame for a (camera, profile).
    pub fn frame_path(&self, camera_id: &str, profile: &str) -> std::path::PathBuf {
        self.cfg
            .camera_frames_dir(camera_id)
            .join(frame_filename(profile))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Total budget cost of an allocation, in the allocator's own currency.
    fn total_cost(alloc: &[(f64, i64)], want: &[(f64, i64)]) -> f64 {
        alloc
            .iter()
            .zip(want)
            .map(|((fps, w), (_, wq))| frame_cost(*fps, *w, *wq))
            .sum()
    }

    #[test]
    fn allocate_favors_priority_then_floors_the_rest() {
        // Priority-ordered requests against a budget of 10: plenty of headroom for the leader,
        // the rest floored but RUNNING at full width (no ladder needed at this pressure).
        let want = [(10.0, 640), (5.0, 640), (5.0, 640)];
        let got = allocate(&want, 10.0, 10);
        assert_eq!(got.len(), 3);
        assert!(
            got[0].0 > got[1].0 && got[0].0 > got[2].0,
            "highest-priority camera gets the most fps: {got:?}"
        );
        assert!(
            got.iter().all(|(fps, w)| *fps >= MIN_FPS && *w == 640),
            "all three run, none downgraded at this pressure: {got:?}"
        );
        assert!(total_cost(&got, &want) <= 10.0 + 1e-9);
    }

    /// The heart of #39: pressure that previously SHED cameras now steps them down the resolution
    /// ladder — floor fps at reduced width — so every camera keeps sight. Budget 1.4 cannot cover
    /// four full-width floors (4 × MIN_FPS = 2.0); the old allocator would shed the tail.
    #[test]
    fn allocate_downgrades_resolution_before_shedding() {
        let want = [(2.0, 640), (2.0, 640), (2.0, 640), (2.0, 640)];
        let got = allocate(&want, 1.4, 16);
        assert!(
            got.iter().all(|(fps, _)| *fps > 0.0),
            "no camera is shed while the ladder can pay for it: {got:?}"
        );
        assert!(
            got.iter().any(|(_, w)| *w < 640),
            "at least one camera stepped down the ladder: {got:?}"
        );
        assert!(
            total_cost(&got, &want) <= 1.4 + 1e-9,
            "ladder admissions stay inside the budget: {got:?}"
        );
        // Priority order respected: earlier cameras never worse off than later ones.
        assert!(got[0].0 >= got[3].0 && got[0].1 >= got[3].1, "{got:?}");
    }

    /// Extreme scarcity sheds from the BOTTOM of the priority order (the admission count keeps the
    /// leader alive), and the decoder cap sheds regardless of budget.
    #[test]
    fn allocate_sheds_lowest_priority_under_extreme_scarcity() {
        let want = [(2.0, 640), (2.0, 640), (2.0, 640), (2.0, 640)];
        // Budget 0.3 affords two deepest-rung admissions (2 × 0.125) and nothing more.
        let got = allocate(&want, 0.3, 16);
        assert!(got[0].0 > 0.0 && got[1].0 > 0.0, "top two survive: {got:?}");
        assert_eq!(got[2], (0.0, 0), "{got:?}");
        assert_eq!(got[3], (0.0, 0), "{got:?}");
        assert!(total_cost(&got, &want) <= 0.3 + 1e-9);
        // Decoder cap still sheds regardless of budget.
        let capped = allocate(&want, 100.0, 2);
        assert_eq!(capped[2], (0.0, 0));
        assert_eq!(capped[3], (0.0, 0));
    }

    #[test]
    fn ladder_width_never_below_floor_and_cost_is_quadratic() {
        // Force the second camera onto a deep rung: floors won't fit, ladder must.
        let want = [(2.0, 480), (2.0, 480)];
        let got = allocate(&want, MIN_FPS + 0.23, 16);
        assert!(got[1].0 > 0.0, "ladder admission: {got:?}");
        assert!(got[1].1 >= MIN_LADDER_WIDTH, "width floored: {got:?}");
        assert!(got[1].1 < 480, "and genuinely downgraded: {got:?}");
        // Cost model: half width = quarter cost.
        assert!((frame_cost(2.0, 320, 640) - 0.5).abs() < 1e-9);
        assert!((frame_cost(2.0, 640, 640) - 2.0).abs() < 1e-9);
    }
}
