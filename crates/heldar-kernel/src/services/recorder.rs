//! Recorder supervisor: one FFmpeg process per camera, recording the configured stream
//! into time-segmented fragmented-MP4 files with `-c copy` (no decode). Supervises the
//! process, reconnects with backoff on stream loss, and maintains live camera status.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
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

/// Keep at most this many bytes of an FFmpeg run's stderr (the tail is what matters).
const STDERR_TAIL_CAP: usize = 8192;

struct CameraTask {
    stop: watch::Sender<bool>,
    handle: JoinHandle<()>,
    /// Monotonic id distinguishing this task from any later task for the same camera.
    generation: u64,
}

/// Owns and supervises the per-camera recorder tasks.
pub struct RecorderManager {
    pool: SqlitePool,
    cfg: Arc<Config>,
    tasks: Mutex<HashMap<String, CameraTask>>,
    next_generation: AtomicU64,
}

impl RecorderManager {
    pub fn new(pool: SqlitePool, cfg: Arc<Config>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            cfg,
            tasks: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(1),
        })
    }

    /// Start recorders for all cameras that should record.
    pub async fn start_all(self: &Arc<Self>) -> anyhow::Result<()> {
        if !self.cfg.recorder_enabled {
            tracing::warn!("recorder globally disabled (HELDAR_RECORDER_ENABLED=false)");
            return Ok(());
        }
        let cams: Vec<Camera> = sqlx::query_as::<_, Camera>(
            "SELECT * FROM cameras WHERE enabled = 1 AND record_enabled = 1",
        )
        .fetch_all(&self.pool)
        .await?;
        tracing::info!(count = cams.len(), "recorder: starting cameras");
        for cam in cams {
            self.spawn(cam.id).await;
        }
        Ok(())
    }

    /// Reconcile a single camera's recorder against its current DB state.
    pub async fn reconcile(self: &Arc<Self>, camera_id: &str) {
        self.stop(camera_id).await;
        if !self.cfg.recorder_enabled {
            return;
        }
        let cam = sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
            .bind(camera_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten();
        match cam {
            Some(cam) if cam.should_record() => self.spawn(camera_id.to_string()).await,
            Some(_) => {
                let _ = repo::set_state(&self.pool, camera_id, "disabled", None).await;
            }
            None => {}
        }
    }

    /// Stop a camera's recorder task, killing its FFmpeg process. Returns only once the task is
    /// actually gone (aborting it if it does not stop promptly).
    pub async fn stop(self: &Arc<Self>, camera_id: &str) {
        let task = { self.tasks.lock().await.remove(camera_id) };
        if let Some(task) = task {
            let _ = task.stop.send(true);
            let mut handle = task.handle;
            if tokio::time::timeout(Duration::from_secs(8), &mut handle)
                .await
                .is_err()
            {
                // The task did not honor the stop signal in time. Abort it: dropping its frame
                // drops the FFmpeg Child, and kill_on_drop terminates the process.
                tracing::warn!(%camera_id, "recorder: task did not stop within 8s; aborting");
                handle.abort();
                let _ = handle.await;
            }
        }
    }

    /// Stop all recorder tasks (graceful shutdown).
    pub async fn shutdown(self: &Arc<Self>) {
        let ids: Vec<String> = { self.tasks.lock().await.keys().cloned().collect() };
        tracing::info!(count = ids.len(), "recorder: shutting down");
        for id in ids {
            self.stop(&id).await;
        }
    }

    /// Camera ids currently being supervised.
    pub async fn active_ids(&self) -> Vec<String> {
        self.tasks.lock().await.keys().cloned().collect()
    }

    async fn spawn(self: &Arc<Self>, camera_id: String) {
        let (tx, rx) = watch::channel(false);
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);

        // Hold the map lock across spawn+insert so a concurrent stop()/delete can never observe a
        // gap where the task is running but not yet registered (which would let it slip through).
        let mut tasks = self.tasks.lock().await;
        let me = self.clone();
        let id_for_task = camera_id.clone();
        let handle = tokio::spawn(async move {
            me.supervise(id_for_task, generation, rx).await;
        });
        if let Some(old) = tasks.insert(
            camera_id,
            CameraTask {
                stop: tx,
                handle,
                generation,
            },
        ) {
            // Displaced a previous task: signal AND abort it so two FFmpegs never overlap.
            let _ = old.stop.send(true);
            old.handle.abort();
        }
    }

    async fn supervise(
        self: Arc<Self>,
        camera_id: String,
        generation: u64,
        stop: watch::Receiver<bool>,
    ) {
        self.run_supervise(camera_id.clone(), stop).await;
        // Self-exit cleanup: remove our own entry, but only if it is still ours (a concurrent
        // spawn may have installed a newer task for this camera).
        let mut tasks = self.tasks.lock().await;
        if tasks.get(&camera_id).map(|t| t.generation) == Some(generation) {
            tasks.remove(&camera_id);
            tracing::debug!(%camera_id, "recorder: task removed itself from map on exit");
        }
    }

    async fn run_supervise(&self, camera_id: String, mut stop: watch::Receiver<bool>) {
        let mut backoff: u64 = 1;
        loop {
            if *stop.borrow() {
                return;
            }

            let cam = match sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
                .bind(&camera_id)
                .fetch_optional(&self.pool)
                .await
            {
                Ok(Some(c)) => c,
                Ok(None) => return, // camera deleted
                Err(e) => {
                    tracing::error!(%camera_id, error = %e, "recorder: failed to load camera");
                    if sleep_or_stop(&mut stop, 10).await {
                        return;
                    }
                    continue;
                }
            };
            if !cam.should_record() {
                let _ = repo::set_state(&self.pool, &camera_id, "disabled", None).await;
                return;
            }

            let Some(url) = camera_url::record_url(&cam) else {
                let msg = "no RTSP URL: set address+credentials or an explicit stream URL";
                let _ = repo::set_state(&self.pool, &camera_id, "error", Some(msg)).await;
                let _ = repo::log_event(
                    &self.pool,
                    Some(&camera_id),
                    "recorder_error",
                    "warning",
                    json!({ "reason": msg }),
                )
                .await;
                if sleep_or_stop(&mut stop, 30).await {
                    return;
                }
                continue;
            };

            let dir = self.cfg.camera_recordings_dir(&camera_id);
            if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                tracing::error!(%camera_id, error = %e, "recorder: cannot create recordings dir");
            }
            let seg = cam.segment_seconds.max(2);
            let pattern = dir.join("%Y%m%d_%H%M%S.mp4");
            let masked = camera_url::mask_url(&url);

            let _ = repo::set_state(&self.pool, &camera_id, "connecting", None).await;
            tracing::info!(%camera_id, url = %masked, segment_s = seg, "recorder: starting ffmpeg");

            let mut child = match Command::new(&self.cfg.ffmpeg_bin)
                .kill_on_drop(true)
                .env("TZ", "UTC")
                .args(["-nostdin", "-hide_banner", "-loglevel", "warning"])
                .args(["-rtsp_transport", "tcp"])
                .args(["-timeout", "15000000"]) // 15s RTSP socket I/O timeout -> exit on stall
                .args(["-i", &url])
                .args(["-c", "copy", "-an"]) // copy video; drop audio in Stage 0
                .args(["-f", "segment"])
                .args(["-segment_time", &seg.to_string()])
                .args(["-segment_format", "mp4"])
                .args([
                    "-segment_format_options",
                    "movflags=+frag_keyframe+empty_moov+default_base_moof",
                ])
                .args(["-reset_timestamps", "1"])
                .args(["-strftime", "1"])
                .arg(&pattern)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let msg = format!("spawn ffmpeg failed: {e}");
                    tracing::error!(%camera_id, "{msg}");
                    let _ = repo::set_state(&self.pool, &camera_id, "error", Some(&msg)).await;
                    if sleep_or_stop(&mut stop, 15).await {
                        return;
                    }
                    continue;
                }
            };

            let pid = child.id().map(|p| p as i64);
            let _ = repo::set_running(&self.pool, &camera_id, "recording", pid).await;

            // Drain stderr concurrently (so the pipe never blocks ffmpeg), keeping only a bounded
            // tail so a chatty/long-lived recorder cannot grow this buffer without bound.
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

            let started = Utc::now();
            tokio::select! {
                status = child.wait() => {
                    let raw = String::from_utf8_lossy(&stderr_task.await.unwrap_or_default())
                        .trim().to_string();
                    // Mask any credentials FFmpeg echoes back in the RTSP URL before persisting/logging.
                    let err_tail = camera_url::mask_url(&raw);
                    let ran = (Utc::now() - started).num_seconds();
                    match status {
                        Ok(s) if s.success() =>
                            tracing::warn!(%camera_id, ran_s = ran, "ffmpeg exited (stream ended)"),
                        Ok(s) =>
                            tracing::warn!(%camera_id, ran_s = ran, code = ?s.code(), tail = %err_tail, "ffmpeg exited with error"),
                        Err(e) =>
                            tracing::error!(%camera_id, error = %e, "ffmpeg wait failed"),
                    }
                    let _ = repo::bump_reconnect(&self.pool, &camera_id, &err_tail).await;
                    let _ = repo::log_event(&self.pool, Some(&camera_id), "camera_offline", "warning",
                        json!({ "ran_seconds": ran, "detail": err_tail })).await;
                    // Reset backoff if it ran a healthy while; otherwise exponential up to 30s.
                    backoff = if ran > 30 { 1 } else { (backoff * 2).min(30) };
                    if sleep_or_stop(&mut stop, backoff).await {
                        return;
                    }
                }
                _ = stop.changed() => {
                    tracing::info!(%camera_id, "recorder: stop requested");
                    let _ = child.kill().await;
                    let _ = repo::set_state(&self.pool, &camera_id, "offline", None).await;
                    return;
                }
            }
        }
    }
}

/// Sleep for `secs`, returning `true` if a stop was signaled during the wait.
async fn sleep_or_stop(stop: &mut watch::Receiver<bool>, secs: u64) -> bool {
    if *stop.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(secs)) => *stop.borrow(),
        _ = stop.changed() => *stop.borrow(),
    }
}
