//! Dual / mirror recording: a SECOND, supervised ffmpeg pipeline per `mirror_enabled` camera that
//! writes byte-identical segments to `HELDAR_MIRROR_RECORDINGS_DIR/{camera_id}/` (a redundant DVR
//! copy on a separate volume). It reuses the recorder's [`build_record_command`] with the output dir
//! swapped, so the mirror files are indistinguishable from the primaries (same names, same codecs).
//!
//! The manager is created (and held as `Option` on [`AppState`]) only when the mirror dir is
//! configured. It is a SHADOW of the primary recorder: it never writes camera_status (the primary owns
//! that) and only mirrors continuously while a camera `should_record()` AND has `mirror_enabled`. The
//! mirror dir is NOT indexed (it is a cold redundant copy); restore/index is an operational step.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;
use tokio::io::AsyncReadExt;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use crate::camera_url;
use crate::config::Config;
use crate::models::Camera;
use crate::services::recorder::build_record_command;

/// Keep at most this many bytes of a mirror ffmpeg run's stderr tail (matches the primary recorder).
const STDERR_TAIL_CAP: usize = 8192;

struct MirrorTask {
    stop: watch::Sender<bool>,
    handle: JoinHandle<()>,
    /// Monotonic id distinguishing this task from any later task for the same camera.
    generation: u64,
}

/// Owns and supervises the per-camera mirror recorder tasks.
pub struct MirrorRecorderManager {
    pool: SqlitePool,
    cfg: Arc<Config>,
    /// Mirror recordings root (HELDAR_MIRROR_RECORDINGS_DIR); each camera mirrors into a subdir.
    mirror_root: PathBuf,
    tasks: Mutex<HashMap<String, MirrorTask>>,
    next_generation: AtomicU64,
}

impl MirrorRecorderManager {
    pub fn new(pool: SqlitePool, cfg: Arc<Config>, mirror_root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            pool,
            cfg,
            mirror_root,
            tasks: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(1),
        })
    }

    /// Per-camera mirror output directory under the mirror root.
    fn camera_dir(&self, camera_id: &str) -> PathBuf {
        self.mirror_root.join(camera_id)
    }

    /// Whether a camera should have a mirror pipeline: recording-enabled AND opted into mirroring.
    fn should_mirror(cam: &Camera) -> bool {
        cam.should_record() && cam.mirror_enabled
    }

    /// Start mirror recorders for every camera that should mirror.
    pub async fn start_all(self: &Arc<Self>) -> anyhow::Result<()> {
        if !self.cfg.recorder_enabled {
            return Ok(());
        }
        let cams: Vec<Camera> = sqlx::query_as::<_, Camera>(
            "SELECT * FROM cameras WHERE enabled = 1 AND record_enabled = 1 AND mirror_enabled = 1",
        )
        .fetch_all(&self.pool)
        .await?;
        tracing::info!(count = cams.len(), root = %self.mirror_root.display(), "mirror: starting cameras");
        for cam in cams {
            self.spawn(cam.id).await;
        }
        Ok(())
    }

    /// Reconcile a single camera's mirror recorder against its current DB state (stop, then restart
    /// when it should mirror). Mirroring is continuous and independent of the recording schedule, so
    /// this only depends on `should_record()` + `mirror_enabled`.
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
        if let Some(cam) = cam {
            if Self::should_mirror(&cam) {
                self.spawn(camera_id.to_string()).await;
            }
        }
    }

    /// Stop a camera's mirror task, killing its ffmpeg process (aborting if it does not stop promptly).
    pub async fn stop(self: &Arc<Self>, camera_id: &str) {
        let task = { self.tasks.lock().await.remove(camera_id) };
        if let Some(task) = task {
            let _ = task.stop.send(true);
            let mut handle = task.handle;
            if tokio::time::timeout(Duration::from_secs(8), &mut handle)
                .await
                .is_err()
            {
                tracing::warn!(%camera_id, "mirror: task did not stop within 8s; aborting");
                handle.abort();
                let _ = handle.await;
            }
        }
    }

    /// Stop all mirror tasks (graceful shutdown).
    pub async fn shutdown(self: &Arc<Self>) {
        let ids: Vec<String> = { self.tasks.lock().await.keys().cloned().collect() };
        tracing::info!(count = ids.len(), "mirror: shutting down");
        for id in ids {
            self.stop(&id).await;
        }
    }

    async fn spawn(self: &Arc<Self>, camera_id: String) {
        let (tx, rx) = watch::channel(false);
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        // Hold the map lock across spawn+insert so a concurrent stop()/delete can never observe a gap
        // where the task is running but not yet registered.
        let mut tasks = self.tasks.lock().await;
        let me = self.clone();
        let id_for_task = camera_id.clone();
        let handle = tokio::spawn(async move {
            me.supervise(id_for_task, generation, rx).await;
        });
        if let Some(old) = tasks.insert(
            camera_id,
            MirrorTask {
                stop: tx,
                handle,
                generation,
            },
        ) {
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
        self.run_mirror(camera_id.clone(), stop).await;
        // Self-exit cleanup: remove our own entry only if it is still ours.
        let mut tasks = self.tasks.lock().await;
        if tasks.get(&camera_id).map(|t| t.generation) == Some(generation) {
            tasks.remove(&camera_id);
        }
    }

    /// Mirror loop: keep a continuous ffmpeg writing identical segments to the mirror dir, reconnecting
    /// with backoff on stream loss. Self-exits when the camera is deleted / disabled / un-mirrored.
    async fn run_mirror(&self, camera_id: String, mut stop: watch::Receiver<bool>) {
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
                    tracing::error!(%camera_id, error = %e, "mirror: failed to load camera");
                    if sleep_or_stop(&mut stop, 10).await {
                        return;
                    }
                    continue;
                }
            };
            if !Self::should_mirror(&cam) {
                // Disabled / un-mirrored out from under us; reconcile() will respawn if needed.
                return;
            }

            let Some(url) = camera_url::record_url(&cam) else {
                tracing::warn!(%camera_id, "mirror: no RTSP URL; retrying");
                if sleep_or_stop(&mut stop, 30).await {
                    return;
                }
                continue;
            };

            let dir = self.camera_dir(&camera_id);
            if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                tracing::error!(%camera_id, error = %e, "mirror: cannot create mirror dir");
            }
            let masked = camera_url::mask_url(&url);
            tracing::info!(%camera_id, url = %masked, dir = %dir.display(), "mirror: starting ffmpeg");

            let mut child = match build_record_command(&self.cfg, &cam, &url, &dir).spawn() {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(%camera_id, "mirror: spawn ffmpeg failed: {e}");
                    if sleep_or_stop(&mut stop, 15).await {
                        return;
                    }
                    continue;
                }
            };

            // Drain stderr concurrently with a bounded tail (so the pipe never blocks ffmpeg).
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
                    let err_tail = camera_url::mask_url(&raw);
                    let ran = (Utc::now() - started).num_seconds();
                    match status {
                        Ok(s) if s.success() =>
                            tracing::warn!(%camera_id, ran_s = ran, "mirror: ffmpeg exited (stream ended)"),
                        Ok(s) =>
                            tracing::warn!(%camera_id, ran_s = ran, code = ?s.code(), tail = %err_tail, "mirror: ffmpeg exited with error"),
                        Err(e) =>
                            tracing::error!(%camera_id, error = %e, "mirror: ffmpeg wait failed"),
                    }
                    backoff = if ran > 30 { 1 } else { (backoff * 2).min(30) };
                    if sleep_or_stop(&mut stop, backoff).await {
                        return;
                    }
                }
                _ = stop.changed() => {
                    tracing::info!(%camera_id, "mirror: stop requested");
                    let _ = child.kill().await;
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
