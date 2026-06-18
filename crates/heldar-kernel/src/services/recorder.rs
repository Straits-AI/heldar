//! Recorder supervisor: one FFmpeg process per camera, recording the configured stream
//! into time-segmented fragmented-MP4 files with `-c copy` (no decode). Supervises the
//! process, reconnects with backoff on stream loss, and maintains live camera status.

use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use crate::camera_url;
use crate::config::Config;
use crate::models::{Camera, RecordSchedule};
use crate::repo;

/// Keep at most this many bytes of an FFmpeg run's stderr (the tail is what matters).
const STDERR_TAIL_CAP: usize = 8192;

struct CameraTask {
    stop: watch::Sender<bool>,
    /// Event-trigger channel for `event` / `scheduled_event` cameras: holds the current trigger
    /// window end (`None` = no active trigger window). [`RecorderManager::trigger`] extends it; the
    /// event supervisor records while it (or a schedule window) is active. Unused for
    /// `continuous` / `scheduled` tasks.
    trigger: watch::Sender<Option<DateTime<Utc>>>,
    handle: JoinHandle<()>,
    /// Monotonic id distinguishing this task from any later task for the same camera.
    generation: u64,
}

/// Whether a record mode is event-capable (records on triggers): `event` or `scheduled_event`.
fn event_capable(mode: &str) -> bool {
    matches!(mode, "event" | "scheduled_event")
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
            // Honor the recording schedule at boot: a `scheduled` camera outside its window is left
            // idle (the schedule watcher will start it when the window opens). Continuous cameras
            // always start. Event-capable cameras (`event` / `scheduled_event`) always spawn ARMED:
            // their supervisor sits idle until a trigger (or, for scheduled_event, a window) makes it
            // record.
            if event_capable(&cam.record_mode) || self.eval_schedule(&cam.id).await {
                self.spawn(cam.id).await;
            } else {
                let _ = repo::set_state(&self.pool, &cam.id, "disabled", None).await;
            }
        }
        Ok(())
    }

    /// Reconcile a single camera's recorder against its current DB state. Starts a recorder when the
    /// camera should record AND its schedule says it should be recording now; otherwise stops it and
    /// marks it idle. Always restarts a running recorder (config may have changed) — callers that
    /// must not churn an actively-recording camera should use [`Self::reconcile_schedules`].
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
            Some(cam) if cam.should_record() => {
                if event_capable(&cam.record_mode) || self.eval_schedule(camera_id).await {
                    // Continuous / in-window scheduled: records immediately. Event-capable: spawns
                    // ARMED — the event supervisor decides record-vs-idle from triggers + schedule.
                    self.spawn(camera_id.to_string()).await;
                } else {
                    // Enabled but a `scheduled` camera outside its window: intentionally not
                    // recording right now (the schedule watcher will start it when the window opens).
                    let _ = repo::set_state(&self.pool, camera_id, "disabled", None).await;
                }
            }
            Some(_) => {
                let _ = repo::set_state(&self.pool, camera_id, "disabled", None).await;
            }
            None => {}
        }
    }

    /// Whether `camera_id` should be recording at this instant per its `record_mode` + schedule,
    /// IGNORING event triggers (those are handled by the event supervisor / [`Self::trigger`]):
    /// - `continuous` is always on.
    /// - `scheduled` / `scheduled_event` are on only inside an enabled time-of-day window for today's
    ///   weekday, evaluated against the SERVER's LOCAL timezone (chrono::Local), with overnight wrap.
    /// - `event` (and any unknown mode) has no time-based recording, so it is off here; it records
    ///   only while a trigger window is active.
    pub async fn eval_schedule(&self, camera_id: &str) -> bool {
        let mode: Option<String> =
            sqlx::query_scalar("SELECT record_mode FROM cameras WHERE id = ?")
                .bind(camera_id)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        match mode.as_deref().unwrap_or("continuous") {
            "continuous" => true,
            "scheduled" | "scheduled_event" => {
                let rows = sqlx::query_as::<_, RecordSchedule>(
                    "SELECT * FROM camera_schedules WHERE camera_id = ? AND enabled = 1",
                )
                .bind(camera_id)
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default();
                let now = Local::now();
                rows.iter().any(|s| schedule_active_at(s, now))
            }
            _ => false,
        }
    }

    /// Reconcile only the pure `scheduled` cameras whose recording state must change because their
    /// window just opened or closed. Called periodically by the schedule watcher. Cameras already in
    /// the correct state are left untouched, so an actively-recording camera is never restarted
    /// mid-window. `scheduled_event` is deliberately excluded: those tasks are always ARMED and the
    /// event supervisor opens/closes their window itself (so the watcher must not churn them).
    pub async fn reconcile_schedules(self: &Arc<Self>) {
        if !self.cfg.recorder_enabled {
            return;
        }
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM cameras
             WHERE enabled = 1 AND record_enabled = 1
               AND record_mode = 'scheduled'",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        if ids.is_empty() {
            return;
        }
        let active: HashSet<String> = self.active_ids().await.into_iter().collect();
        for id in ids {
            let want = self.eval_schedule(&id).await;
            let running = active.contains(&id);
            if want != running {
                self.reconcile(&id).await;
            }
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
        // Trigger window channel (event / scheduled_event). Starts with no active window.
        let (trig_tx, trig_rx) = watch::channel(None::<DateTime<Utc>>);
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);

        // Hold the map lock across spawn+insert so a concurrent stop()/delete can never observe a
        // gap where the task is running but not yet registered (which would let it slip through).
        let mut tasks = self.tasks.lock().await;
        let me = self.clone();
        let id_for_task = camera_id.clone();
        let handle = tokio::spawn(async move {
            me.supervise(id_for_task, generation, rx, trig_rx).await;
        });
        if let Some(old) = tasks.insert(
            camera_id,
            CameraTask {
                stop: tx,
                trigger: trig_tx,
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
        trigger: watch::Receiver<Option<DateTime<Utc>>>,
    ) {
        // Choose the supervisor by record mode at task start. A mode change always goes through
        // `reconcile()` (stop + respawn), so picking the path here is sufficient; both paths also
        // self-exit if the camera is later deleted / disabled / its mode no longer matches.
        let mode: Option<String> =
            sqlx::query_scalar("SELECT record_mode FROM cameras WHERE id = ?")
                .bind(&camera_id)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        if event_capable(mode.as_deref().unwrap_or("continuous")) {
            self.run_event_supervise(camera_id.clone(), stop, trigger)
                .await;
        } else {
            self.run_supervise(camera_id.clone(), stop).await;
        }
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
            let masked = camera_url::mask_url(&url);

            let _ = repo::set_state(&self.pool, &camera_id, "connecting", None).await;
            tracing::info!(%camera_id, url = %masked, segment_s = seg, "recorder: starting ffmpeg");

            let mut child = match self.build_record_command(&cam, &url, &dir).spawn() {
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

    /// Build the segmenting FFmpeg command for a camera's recorded stream. Delegates to the shared
    /// [`build_record_command`] free fn so the continuous / event supervisors AND the mirror recorder
    /// all produce byte-identical recordings.
    fn build_record_command(&self, cam: &Camera, url: &str, dir: &std::path::Path) -> Command {
        build_record_command(&self.cfg, cam, url, dir)
    }

    /// Supervise an EVENT-capable camera (`event` / `scheduled_event`). The task is always ARMED: it
    /// sits idle (status `disabled`) until either a trigger window is active — a [`Self::trigger`] set
    /// `window_end = now + post_roll_seconds` — or, for `scheduled_event`, a recording window is open.
    /// While either holds it records continuously (segmenting like the main recorder), reconnecting
    /// with backoff on stream loss, and stops once the trigger window has elapsed AND no schedule
    /// window is open.
    ///
    /// PRE-ROLL is best-effort: the kernel keeps no always-on ring buffer for idle event cameras, so
    /// recording begins at the trigger. `pre_roll_seconds` is honored only from recent completed
    /// segments that already exist on disk (e.g. a `scheduled_event` window already in progress, or a
    /// still-active prior trigger) — assembled at clip/evidence-export time. Frame-accurate pre-roll
    /// for an idle camera would require continuous buffering (a future enhancement).
    async fn run_event_supervise(
        &self,
        camera_id: String,
        mut stop: watch::Receiver<bool>,
        mut trig: watch::Receiver<Option<DateTime<Utc>>>,
    ) {
        // Reasons the inner ffmpeg session ended.
        enum End {
            Stop,
            WindowClosed,
            Exited(std::io::Result<std::process::ExitStatus>),
        }

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
                    tracing::error!(%camera_id, error = %e, "recorder(event): failed to load camera");
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
            if !event_capable(&cam.record_mode) {
                // Mode changed out from under us; let reconcile() respawn the right supervisor.
                return;
            }

            // Should we be recording right now? A trigger window OR (for scheduled_event) a schedule
            // window. eval_schedule() returns false for pure `event`, so triggers are its only source.
            let now = Utc::now();
            let trigger_active = matches!(*trig.borrow(), Some(end) if now <= end);
            let schedule_active = self.eval_schedule(&camera_id).await;
            if !(trigger_active || schedule_active) {
                // Idle / armed: wait for a trigger, a periodic re-check (a scheduled_event window may
                // open), or a stop. Status mirrors the legacy "event camera not recording" state.
                let _ = repo::set_state(&self.pool, &camera_id, "disabled", None).await;
                let idle_tick = self.cfg.schedule_check_interval_s.max(5);
                tokio::select! {
                    _ = stop.changed() => return,
                    _ = trig.changed() => {}
                    _ = tokio::time::sleep(Duration::from_secs(idle_tick)) => {}
                }
                continue;
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
                tracing::error!(%camera_id, error = %e, "recorder(event): cannot create recordings dir");
            }
            let masked = camera_url::mask_url(&url);
            let _ = repo::set_state(&self.pool, &camera_id, "connecting", None).await;
            tracing::info!(%camera_id, url = %masked, "recorder(event): trigger/window active; starting ffmpeg");

            let mut child = match self.build_record_command(&cam, &url, &dir).spawn() {
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

            // Drain stderr concurrently, keeping a bounded tail (same as the main recorder).
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
            // Inner loop: keep THIS ffmpeg child alive until stop, process exit, or until we should no
            // longer be recording (trigger window elapsed AND no schedule window open). Reacts
            // immediately to an extended/new trigger via `trig.changed()`.
            let end = loop {
                let now = Utc::now();
                // Sleep precisely to the trigger window end (so post-roll stops on time); also re-check
                // at least every schedule tick to notice a scheduled_event window closing.
                let mut recheck = self.cfg.schedule_check_interval_s.max(5);
                if let Some(w_end) = *trig.borrow() {
                    if w_end > now {
                        let remaining = (w_end - now).num_seconds().max(0) as u64 + 1;
                        recheck = recheck.min(remaining);
                    }
                }
                let recheck = recheck.max(1);
                tokio::select! {
                    status = child.wait() => break End::Exited(status),
                    _ = stop.changed() => break End::Stop,
                    _ = trig.changed() => { /* window extended/changed; recompute deadline */ }
                    _ = tokio::time::sleep(Duration::from_secs(recheck)) => {
                        let now = Utc::now();
                        let trig_on = matches!(*trig.borrow(), Some(e) if now <= e);
                        let sched_on = self.eval_schedule(&camera_id).await;
                        if !(trig_on || sched_on) {
                            break End::WindowClosed;
                        }
                    }
                }
            };

            match end {
                End::Stop => {
                    tracing::info!(%camera_id, "recorder(event): stop requested");
                    let _ = child.kill().await;
                    let _ = repo::set_state(&self.pool, &camera_id, "offline", None).await;
                    return;
                }
                End::WindowClosed => {
                    let _ = child.kill().await;
                    let _ = repo::set_state(&self.pool, &camera_id, "disabled", None).await;
                    tracing::info!(%camera_id, "recorder(event): trigger window elapsed; stopping ffmpeg");
                    backoff = 1;
                    // Back to the top: re-evaluate (will idle until the next trigger/window).
                }
                End::Exited(status) => {
                    let raw = String::from_utf8_lossy(&stderr_task.await.unwrap_or_default())
                        .trim()
                        .to_string();
                    let err_tail = camera_url::mask_url(&raw);
                    let ran = (Utc::now() - started).num_seconds();
                    match status {
                        Ok(s) if s.success() => {
                            tracing::warn!(%camera_id, ran_s = ran, "ffmpeg exited (stream ended)")
                        }
                        Ok(s) => {
                            tracing::warn!(%camera_id, ran_s = ran, code = ?s.code(), tail = %err_tail, "ffmpeg exited with error")
                        }
                        Err(e) => tracing::error!(%camera_id, error = %e, "ffmpeg wait failed"),
                    }
                    let _ = repo::bump_reconnect(&self.pool, &camera_id, &err_tail).await;
                    let _ = repo::log_event(
                        &self.pool,
                        Some(&camera_id),
                        "camera_offline",
                        "warning",
                        json!({ "ran_seconds": ran, "detail": err_tail }),
                    )
                    .await;
                    backoff = if ran > 30 { 1 } else { (backoff * 2).min(30) };
                    if sleep_or_stop(&mut stop, backoff).await {
                        return;
                    }
                    // Back to the top: if still inside the window, re-spawns ffmpeg (reconnect).
                }
            }
        }
    }

    /// Fire an event recording trigger for a camera: extend its trigger window to
    /// `now + post_roll_seconds` (repeated triggers keep the later end). No-op (returns `None`) for a
    /// camera that is not `event` / `scheduled_event`, is not recording-enabled, or has no armed task
    /// (e.g. the recorder is globally disabled). Returns the resulting window end. Cheap and
    /// idempotent — safe to call on every zone/breach event.
    pub async fn trigger(&self, camera_id: &str, reason: &str) -> Option<DateTime<Utc>> {
        let cam = sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
            .bind(camera_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()?;
        if !cam.should_record() || !event_capable(&cam.record_mode) {
            return None;
        }
        let post = cam.post_roll_seconds.clamp(0, 3600);
        let end = Utc::now() + chrono::Duration::seconds(post);

        let tasks = self.tasks.lock().await;
        let task = tasks.get(camera_id)?;
        let mut window_end = end;
        task.trigger.send_modify(|cur| {
            // Keep the later of the existing window and this one (a trigger only extends).
            let next = match *cur {
                Some(existing) if existing > end => existing,
                _ => end,
            };
            *cur = Some(next);
            window_end = next;
        });
        tracing::info!(%camera_id, %reason, window_end = %window_end, "recorder: event trigger");
        Some(window_end)
    }
}

/// Build the segmenting FFmpeg command for a camera's recorded stream (stream-copy, fragmented-MP4
/// segments, UTC strftime names). Shared verbatim by the continuous + event supervisors and the
/// mirror recorder ([`crate::services::mirror`]) so every pipeline writes byte-identical segments.
/// Video is always `-c copy`; audio is passed through only when the camera opts in. `dir` is the
/// output directory (the primary recordings dir, or the mirror dir for the mirror recorder).
pub(crate) fn build_record_command(
    cfg: &Config,
    cam: &Camera,
    url: &str,
    dir: &std::path::Path,
) -> Command {
    let seg = cam.segment_seconds.max(2);
    let pattern = dir.join("%Y%m%d_%H%M%S.mp4");
    let audio_args: &[&str] = if cam.record_audio {
        &["-c:a", "copy"]
    } else {
        &["-an"]
    };
    let mut cmd = Command::new(&cfg.ffmpeg_bin);
    cmd.kill_on_drop(true)
        .env("TZ", "UTC")
        .args(["-nostdin", "-hide_banner", "-loglevel", "warning"])
        .args(["-rtsp_transport", "tcp"])
        .args(["-timeout", "15000000"]) // 15s RTSP socket I/O timeout -> exit on stall
        .args(["-i", url])
        .args(["-c", "copy"]) // stream-copy (no decode)
        .args(audio_args) // audio: pass-through when record_audio, else dropped
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
        .stderr(Stdio::piped());
    cmd
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

/// Parse "HH:MM" 24h into minutes-since-midnight (0..=1439). Tolerates non-zero-padded hours/minutes.
fn parse_hhmm(s: &str) -> Option<i32> {
    let (h, m) = s.split_once(':')?;
    let h: i32 = h.trim().parse().ok()?;
    let m: i32 = m.trim().parse().ok()?;
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

/// Parse a JSON array of weekday ints into a list, keeping only valid 0..6 (0=Mon..6=Sun) values.
fn parse_days(v: &Value) -> Vec<i64> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|d| d.as_i64())
                .filter(|d| (0..7).contains(d))
                .collect()
        })
        .unwrap_or_default()
}

/// Is a window active at weekday `wd` (0=Mon..6=Sun) and minute-of-day `minute`? A same-day window
/// (`start` <= `end`) is `[start, end)` on a scheduled day. An overnight window (`start` > `end`)
/// wraps midnight: its evening part is on the start day; its early-morning part (before `end`)
/// belongs to the window that STARTED the previous day.
fn window_active(days: &[i64], start: i32, end: i32, wd: i64, minute: i32) -> bool {
    if start <= end {
        days.contains(&wd) && minute >= start && minute < end
    } else {
        let prev = (wd + 6) % 7; // yesterday's weekday
        (days.contains(&wd) && minute >= start) || (days.contains(&prev) && minute < end)
    }
}

/// Whether a single schedule row is active at local instant `now`. Malformed times never match.
fn schedule_active_at(s: &RecordSchedule, now: DateTime<Local>) -> bool {
    let (Some(start), Some(end)) = (parse_hhmm(&s.time_start), parse_hhmm(&s.time_end)) else {
        return false;
    };
    let days = parse_days(&s.days.0);
    let wd = now.weekday().num_days_from_monday() as i64; // 0=Mon..6=Sun
    let minute = now.hour() as i32 * 60 + now.minute() as i32;
    window_active(&days, start, end, wd, minute)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_capable_modes() {
        assert!(event_capable("event"));
        assert!(event_capable("scheduled_event"));
        assert!(!event_capable("continuous"));
        assert!(!event_capable("scheduled"));
        assert!(!event_capable("nonsense"));
    }

    #[test]
    fn parse_hhmm_valid_and_invalid() {
        assert_eq!(parse_hhmm("00:00"), Some(0));
        assert_eq!(parse_hhmm("9:30"), Some(570));
        assert_eq!(parse_hhmm("23:59"), Some(1439));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("x:y"), None);
        assert_eq!(parse_hhmm("1230"), None);
    }

    #[test]
    fn parse_days_filters_out_of_range() {
        assert_eq!(parse_days(&json!([0, 1, 6])), vec![0, 1, 6]);
        assert_eq!(parse_days(&json!([7, -1, 3])), vec![3]);
        assert_eq!(parse_days(&json!("nope")), Vec::<i64>::new());
    }

    #[test]
    fn window_same_day() {
        let days = vec![0, 1, 2, 3, 4]; // Mon..Fri, 09:00..17:00
        assert!(window_active(&days, 540, 1020, 0, 600)); // Mon 10:00 -> in
        assert!(!window_active(&days, 540, 1020, 0, 480)); // Mon 08:00 -> before
        assert!(!window_active(&days, 540, 1020, 0, 1020)); // end is exclusive
        assert!(!window_active(&days, 540, 1020, 5, 600)); // Sat -> not scheduled
    }

    #[test]
    fn window_overnight_wrap() {
        let days = vec![0]; // Monday window 22:00..06:00
        let (start, end) = (1320, 360);
        assert!(window_active(&days, start, end, 0, 1380)); // Mon 23:00 -> evening part
        assert!(window_active(&days, start, end, 1, 120)); // Tue 02:00 -> Monday's carryover
        assert!(!window_active(&days, start, end, 1, 400)); // Tue 06:40 -> after end
        assert!(!window_active(&days, start, end, 0, 300)); // Mon 05:00 -> would be Sunday's window
    }
}
