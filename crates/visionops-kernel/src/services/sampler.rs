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
        let rows: Vec<(String, String, f64, i64)> = sqlx::query_as(
            "SELECT c.id, t.stream_profile, MAX(t.fps) AS fps, MAX(t.width) AS width
             FROM cameras c JOIN ai_tasks t ON t.camera_id = c.id
             WHERE c.enabled = 1 AND t.enabled = 1
             GROUP BY c.id, t.stream_profile
             ORDER BY c.id, t.stream_profile",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        if rows.is_empty() {
            return;
        }

        let budget = self.cfg.ai_max_total_fps.max(1.0);
        // Cap concurrent decoders so total fps cannot exceed the budget even at the MIN_FPS floor.
        let max_samplers = (budget / MIN_FPS).floor().max(1.0) as usize;
        let run = rows.len().min(max_samplers);
        let per_camera_cap = budget / run as f64;
        if rows.len() > run {
            tracing::warn!(
                requested = rows.len(),
                running = run,
                "sampler: AI fps budget exhausted; some cameras will not be sampled"
            );
        }
        tracing::info!(
            samplers = run,
            budget,
            per_camera_cap,
            "sampler: rebalancing AI frame budget"
        );

        for (i, (cam, profile, max_fps, width)) in rows.into_iter().enumerate() {
            if i < run {
                let effective = max_fps.min(per_camera_cap).max(MIN_FPS);
                self.spawn(cam, profile, effective, width).await;
            } else {
                self.set_info(&cam, &profile, "budget_exhausted", 0.0).await;
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

    async fn set_info(&self, camera_id: &str, profile: &str, state: &str, fps: f64) {
        self.info.lock().await.insert(
            sampler_key(camera_id, profile),
            SamplerInfo {
                camera_id: camera_id.to_string(),
                stream_profile: profile.to_string(),
                state: state.to_string(),
                fps,
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
                self.set_info(&camera_id, &profile, "stopped", fps).await;
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
                self.set_info(&camera_id, &profile, "error", fps).await;
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
            self.set_info(&camera_id, &profile, "connecting", fps).await;
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
                    self.set_info(&camera_id, &profile, "error", fps).await;
                    if sleep_or_stop(&mut stop, 15).await {
                        return;
                    }
                    continue;
                }
            };
            self.set_info(&camera_id, &profile, "sampling", fps).await;
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
                    self.set_info(&camera_id, &profile, "offline", fps).await;
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
                    self.set_info(&camera_id, &profile, "stopped", fps).await;
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

impl SamplerManager {
    /// Filesystem path of the latest sampled frame for a (camera, profile).
    pub fn frame_path(&self, camera_id: &str, profile: &str) -> std::path::PathBuf {
        self.cfg
            .camera_frames_dir(camera_id)
            .join(frame_filename(profile))
    }
}
