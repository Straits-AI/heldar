//! Recorder supervisor: one FFmpeg process per camera, recording the configured stream
//! into time-segmented fragmented-MP4 files with `-c copy` (no decode). Supervises the
//! process, reconnects with backoff on stream loss, and maintains live camera status.

use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
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
    ///   weekday, with overnight wrap, evaluated against the camera's SITE timezone (#125) — or,
    ///   when no zone is configured anywhere, against the SERVER's local zone exactly as before.
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
                // WHICH CLOCK A RECORDING WINDOW FOLLOWS. This is the line, and the fallback is
                // deliberate: an unconfigured box keeps evaluating against the server's local zone,
                // because moving a working site's recording windows on upgrade day — silently, with
                // no error — is the worst thing this change could do. Set a zone and everything
                // moves together, which is the point of setting one.
                //
                // ponytail: one site lookup per scheduled camera per 30s tick. Cache it on
                // RecorderManager and invalidate on a site/timezone update if that ever shows up in
                // a profile; measuring first is cheaper than a cache that goes stale.
                let (tz, _src) = crate::services::tz::site_tz(&self.pool, Some(camera_id)).await;
                match tz {
                    Some(tz) => {
                        let now = Utc::now().with_timezone(&tz);
                        rows.iter().any(|s| schedule_active_at(s, now))
                    }
                    None => {
                        let now = Local::now();
                        rows.iter().any(|s| schedule_active_at(s, now))
                    }
                }
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
                    backoff = next_backoff(backoff, ran);
                    if sleep_or_stop(&mut stop, backoff).await {
                        return;
                    }
                }
                _ = stop.changed() => {
                    tracing::info!(%camera_id, "recorder: stop requested");
                    finish_and_stop(&mut child, &camera_id).await;
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
                // Sleep precisely to the trigger window end (so post-roll stops on time); also re-check
                // at least every schedule tick to notice a scheduled_event window closing.
                let recheck = event_recheck_secs(
                    self.cfg.schedule_check_interval_s.max(5),
                    *trig.borrow(),
                    Utc::now(),
                );
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
                    finish_and_stop(&mut child, &camera_id).await;
                    let _ = repo::set_state(&self.pool, &camera_id, "offline", None).await;
                    return;
                }
                End::WindowClosed => {
                    // The END of an event window is the worst place to truncate: the last seconds of
                    // a triggered recording are its post-roll, which is the part someone triggered
                    // the recording to see.
                    finish_and_stop(&mut child, &camera_id).await;
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
                    backoff = next_backoff(backoff, ran);
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
            // A trigger only extends the window, never shrinks it.
            let next = extend_trigger_window(*cur, end);
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

/// How long FFmpeg gets to close the segment it is writing before we insist (#167).
///
/// Must stay comfortably under [`RecorderManager::stop`]'s 8-second budget: when that timeout
/// expires it ABORTS the task, which drops the `Child`, and `kill_on_drop` then SIGKILLs — undoing
/// the graceful shutdown this exists to perform. Three seconds of headroom.
const FINALIZE_GRACE: Duration = Duration::from_secs(5);

/// Ask FFmpeg to finish the segment it is writing, then insist.
///
/// `Child::kill` is SIGKILL, which takes the process out between fragments and leaves the
/// in-progress segment truncated — the recorder had already captured those seconds and threw them
/// away. Measured on the qualification harness at 3.0–4.4 s of footage per camera per restart
/// (#167), and reproduced directly against these exact muxer arguments: interrupting mid-segment
/// leaves a 28-byte unplayable file under SIGKILL and a valid 8-second segment under SIGTERM.
///
/// SIGTERM rather than `q` on stdin because the command is built with `-nostdin`; ffmpeg handles
/// both and writes out what it has either way.
///
/// Falls back to SIGKILL on timeout and on non-Unix, so a wedged encoder cannot hold shutdown open.
async fn finish_and_stop(child: &mut tokio::process::Child, camera_id: &str) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // SAFETY: `pid` comes from a child this process spawned and has not yet reaped, so it names
        // that child or nothing. SIGTERM to a stale pid is the same risk every process supervisor
        // carries; the window is bounded by us awaiting the same child immediately below.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        match tokio::time::timeout(FINALIZE_GRACE, child.wait()).await {
            Ok(_) => return,
            Err(_) => tracing::warn!(
                %camera_id,
                grace_s = FINALIZE_GRACE.as_secs(),
                "recorder: ffmpeg did not finish its segment in time; killing (the in-progress \
                 segment will be truncated and the indexer will reject it)"
            ),
        }
    }
    let _ = child.kill().await;
}

/// Reconnect backoff after an ffmpeg child exits. Resets to 1s if the child ran a healthy while
/// (`> 30s`), otherwise doubles up to a 30s cap. This is the guard that keeps a dead/flapping camera
/// from becoming a hot spawn loop (hammering ffmpeg + the DB), while recovering promptly after a
/// transient blip. Shared by both supervisors so the two cadences can't drift.
fn next_backoff(prev: u64, ran_seconds: i64) -> u64 {
    if ran_seconds > 30 {
        1
    } else {
        (prev * 2).min(30)
    }
}

/// The end of an event recording window after a trigger: a trigger only ever EXTENDS the window, never
/// shrinks it, so a burst of events can't cut a prior trigger's post-roll short. Returns the later of
/// the current window end and the new one.
fn extend_trigger_window(
    current: Option<chrono::DateTime<Utc>>,
    new_end: chrono::DateTime<Utc>,
) -> chrono::DateTime<Utc> {
    match current {
        Some(existing) if existing > new_end => existing,
        _ => new_end,
    }
}

/// How long the event supervisor waits before re-checking, so post-roll stops ON TIME: at most the
/// schedule tick (`base_tick`), but shorter when a trigger window closes sooner (remaining + 1s).
/// Always `>= 1` so it never busy-spins on `child.wait()`.
fn event_recheck_secs(
    base_tick: u64,
    window_end: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
) -> u64 {
    let mut recheck = base_tick;
    if let Some(w_end) = window_end {
        if w_end > now {
            let remaining = (w_end - now).num_seconds().max(0) as u64 + 1;
            recheck = recheck.min(remaining);
        }
    }
    recheck.max(1)
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

/// Whether a single schedule row is active at wall-clock instant `now`. Malformed times never match.
///
/// Generic over the zone so the caller decides which clock a window follows. The body needs no
/// awareness of it: `weekday()`, `hour()` and `minute()` already read the wall clock of whatever
/// zone the value carries.
///
/// DAYLIGHT SAVING FALLS OUT WITH NO SPECIAL CASE, but only because the caller converts an *instant*
/// into a zone (`Utc::now().with_timezone(&tz)`), which is total — every instant is exactly one wall
/// clock. Do NOT rewrite this to build a local time and convert the other way; that direction has
/// skipped and repeated readings, and it is where an hour of footage goes missing one Sunday a year.
/// The 30s poll is what makes the total direction sufficient: nothing precomputes a boundary instant
/// that could go stale across an offset change.
fn schedule_active_at<T: TimeZone>(s: &RecordSchedule, now: DateTime<T>) -> bool {
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

    /// Build a schedule row: `days` as JSON ints (0=Mon), "HH:MM" bounds.
    fn sched(days: &str, start: &str, end: &str) -> RecordSchedule {
        RecordSchedule {
            id: "sch_1".into(),
            camera_id: "cam_a".into(),
            days: sqlx::types::Json(serde_json::from_str(days).unwrap()),
            time_start: start.into(),
            time_end: end.into(),
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// THE POINT OF #125, as an assertion.
    ///
    /// A site in Kuala Lumpur schedules 18:00–22:00. At 12:00 UTC it is 20:00 there and the window
    /// is open; the server's own clock — UTC on a default container — reads 12:00 and would say it
    /// is shut. Evaluating a Malaysian operator's evening schedule on the container's clock is how a
    /// recorder records the wrong four hours of the day, every day, silently.
    #[test]
    fn a_window_follows_the_sites_wall_clock_not_the_servers() {
        let s = sched("[0,1,2,3,4,5,6]", "18:00", "22:00");
        let noon_utc = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();

        assert!(
            schedule_active_at(
                &s,
                noon_utc.with_timezone(&crate::services::tz::Tz::Asia__Kuala_Lumpur)
            ),
            "20:00 in Kuala Lumpur is inside an 18:00-22:00 window"
        );
        assert!(
            !schedule_active_at(&s, noon_utc.with_timezone(&crate::services::tz::Tz::UTC)),
            "the same instant read on a UTC box is 12:00 and must NOT be inside it — if this \
             passes, the zone is not reaching the comparison and the test proves nothing"
        );
    }

    /// Daylight saving, on the two Sundays a year that decide whether a recorder has a gap.
    ///
    /// The window is a WALL-CLOCK rule, so it opens whenever the local clock says so, however many
    /// real hours that turns out to be. This works with no special-case code only because the caller
    /// converts an instant INTO a zone, which is total — see `schedule_active_at`'s docs.
    #[test]
    fn daylight_saving_transitions_keep_a_wall_clock_window_honest() {
        use crate::services::tz::Tz;
        // Europe/London springs forward 2026-03-29: 01:00 -> 02:00, so 01:30 local never happens.
        // A 01:00-05:00 window must still be open right after the jump.
        let s = sched("[0,1,2,3,4,5,6]", "01:00", "05:00");
        let just_after_jump = Utc.with_ymd_and_hms(2026, 3, 29, 1, 5, 0).unwrap();
        let local = just_after_jump.with_timezone(&Tz::Europe__London);
        assert_eq!(local.hour(), 2, "the clock has jumped to 02:0x local");
        assert!(
            schedule_active_at(&s, local),
            "a window whose start hour was skipped must still be open afterwards, or the box \
             records nothing until 05:00"
        );

        // Autumn back 2026-10-25: 02:00 -> 01:00, so 01:30 local happens twice. A 01:00-03:00
        // window must be open for BOTH, i.e. three real hours.
        let s = sched("[0,1,2,3,4,5,6]", "01:00", "03:00");
        for utc_hour in [0, 1, 2] {
            let at = Utc.with_ymd_and_hms(2026, 10, 25, utc_hour, 30, 0).unwrap();
            assert!(
                schedule_active_at(&s, at.with_timezone(&Tz::Europe__London)),
                "{utc_hour:02}:30Z falls inside the repeated local hour and must record"
            );
        }
    }

    /// A window that wraps midnight has to wrap the SITE's midnight.
    #[test]
    fn an_overnight_window_wraps_the_sites_midnight() {
        use crate::services::tz::Tz;
        // Sunday 22:00 -> Monday 06:00, KL. 2026-06-01 is a Monday.
        let s = sched("[6]", "22:00", "06:00");
        // 16:00Z Sunday = 00:00 Monday in KL — inside the tail of Sunday's window.
        let tail = Utc.with_ymd_and_hms(2026, 5, 31, 16, 30, 0).unwrap();
        assert!(
            schedule_active_at(&s, tail.with_timezone(&Tz::Asia__Kuala_Lumpur)),
            "00:30 Monday KL belongs to the window that started Sunday evening"
        );
        // The same instant in UTC is 16:30 Sunday — outside it.
        assert!(!schedule_active_at(&s, tail.with_timezone(&Tz::UTC)));
    }

    /// END-TO-END, and the reason it exists: the three tests above call `schedule_active_at`
    /// DIRECTLY, so they prove the function honours whatever zone it is handed — and prove exactly
    /// nothing about whether `eval_schedule` ever hands it one. Ripping the site lookup out of
    /// `eval_schedule` leaves all three green. This one goes through the real path, so it fails.
    ///
    /// The window is built around the CURRENT wall clock in Kuala Lumpur, so the assertion holds at
    /// any time of day: KL is +08:00 with no daylight saving, so the same instant can never fall in
    /// the same one-hour window in both zones.
    #[tokio::test]
    async fn eval_schedule_actually_consults_the_cameras_site_zone() {
        use crate::services::tz::Tz;

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let cfg = Arc::new(Config::from_env());
        let mgr = RecorderManager::new(pool.clone(), cfg);

        let now = Utc::now();
        let kl_hour = now.with_timezone(&Tz::Asia__Kuala_Lumpur).hour();
        // The whole current KL hour. Wrapping past midnight is fine — `window_active`'s overnight
        // branch is exercised for free when kl_hour is 23.
        let start = format!("{kl_hour:02}:00");
        let end = format!("{:02}:00", (kl_hour + 1) % 24);

        for (site, tz, cam) in [
            ("site_kl", "Asia/Kuala_Lumpur", "cam_kl"),
            ("site_utc", "UTC", "cam_utc"),
        ] {
            sqlx::query("INSERT INTO sites (id, name, timezone, created_at) VALUES (?,?,?,?)")
                .bind(site)
                .bind(site)
                .bind(tz)
                .bind(now)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO cameras (id, site_id, name, record_mode, created_at, updated_at)
                 VALUES (?,?,?,'scheduled',?,?)",
            )
            .bind(cam)
            .bind(site)
            .bind(cam)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO camera_schedules
                   (id, camera_id, days, time_start, time_end, enabled, created_at, updated_at)
                 VALUES (?,?,'[0,1,2,3,4,5,6]',?,?,1,?,?)",
            )
            .bind(format!("sch_{cam}"))
            .bind(cam)
            .bind(&start)
            .bind(&end)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }

        assert!(
            mgr.eval_schedule("cam_kl").await,
            "a camera on a Kuala Lumpur site must be recording during the current KL hour \
             ({start}-{end} local)"
        );
        assert!(
            !mgr.eval_schedule("cam_utc").await,
            "the SAME window on a UTC site must be shut at this instant — KL is +08:00, so one \
             instant cannot be inside the same one-hour window in both zones. If this fails, \
             eval_schedule is not reading the camera's site at all and the other schedule tests \
             are proving nothing about the code path that actually runs."
        );
    }

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

    // ---- supervision decision logic (extracted from run_supervise / run_event_supervise) ----------

    #[test]
    fn next_backoff_caps_at_30_and_resets_after_a_healthy_run() {
        // A flapping camera (each attempt dies quickly): 1 -> 2 -> 4 -> 8 -> 16 -> 30 (saturates).
        let mut b = 1;
        let expected = [2, 4, 8, 16, 30, 30, 30];
        for want in expected {
            b = next_backoff(b, 5); // ran only 5s -> unhealthy -> keep doubling toward the cap
            assert_eq!(b, want, "backoff should double toward the 30s cap");
        }
        // A child that ran a healthy while (> 30s) resets the backoff to 1s, so a recovered camera
        // reconnects promptly rather than staying stuck at the cap.
        assert_eq!(next_backoff(30, 31), 1);
        assert_eq!(next_backoff(16, 45), 1);
        // Exactly 30s ran is NOT healthy (strict `>`), so it keeps backing off.
        assert_eq!(next_backoff(4, 30), 8);
    }

    #[test]
    fn extend_trigger_window_only_extends() {
        let t0 = Utc::now();
        let near = t0 + chrono::Duration::seconds(10);
        let far = t0 + chrono::Duration::seconds(60);
        // First trigger sets the window.
        assert_eq!(extend_trigger_window(None, near), near);
        // A later, further trigger extends it.
        assert_eq!(extend_trigger_window(Some(near), far), far);
        // A nearer trigger while a further window is open never SHRINKS it (post-roll is preserved).
        assert_eq!(extend_trigger_window(Some(far), near), far);
    }

    #[test]
    fn event_recheck_secs_wakes_at_window_end_and_never_busy_spins() {
        let now = Utc::now();
        // No open window -> wait the full base tick.
        assert_eq!(event_recheck_secs(30, None, now), 30);
        // Window closes in 5s -> wake at remaining+1 (6s), sooner than the base tick.
        assert_eq!(
            event_recheck_secs(30, Some(now + chrono::Duration::seconds(5)), now),
            6
        );
        // Window far in the future (100s) -> the base tick still bounds it.
        assert_eq!(
            event_recheck_secs(30, Some(now + chrono::Duration::seconds(100)), now),
            30
        );
        // Window already elapsed -> base tick (and never 0 -> no busy-spin on child.wait()).
        assert_eq!(
            event_recheck_secs(30, Some(now - chrono::Duration::seconds(5)), now),
            30
        );
        // A tiny base tick still floors at 1.
        assert_eq!(event_recheck_secs(0, None, now), 1);
    }

    // ---- ffmpeg command construction (pins the recording pipeline + credential/injection safety) ---

    fn test_camera() -> Camera {
        Camera {
            id: "cam1".into(),
            site_id: None,
            name: "Cam 1".into(),
            vendor: "hikvision".into(),
            model: None,
            address: Some("192.168.0.2".into()),
            rtsp_port: 554,
            username: Some("admin".into()),
            password: Some("secret".into()),
            main_stream_url: None,
            sub_stream_url: None,
            record_stream: "main".into(),
            codec: None,
            resolution_main: None,
            resolution_sub: None,
            fps_main: None,
            fps_sub: None,
            capabilities: sqlx::types::Json(json!({})),
            record_enabled: true,
            segment_seconds: 60,
            retention_hours: 24,
            storage_quota_bytes: None,
            record_audio: false,
            record_mode: "continuous".into(),
            pre_roll_seconds: 10,
            post_roll_seconds: 30,
            mirror_enabled: false,
            anr_enabled: false,
            anr_replay_url_template: None,
            native_anpr_enabled: false,
            native_events_enabled: false,
            enabled: true,
            priority: 100,
            live_warm: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn command_args(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// Two adjacent args (`flag` immediately followed by `value`) appear in order.
    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    #[test]
    fn build_record_command_uses_stream_copy_segmenting_and_a_single_url_arg() {
        let mut cfg = Config::from_env();
        cfg.ffmpeg_bin = "/opt/heldar/ffmpeg".into();
        let cam = test_camera();
        let dir = std::path::Path::new("/data/recordings/cam1");
        // A URL with a space would inject extra ffmpeg args IF it were split — assert it stays ONE arg.
        let url = "rtsp://admin:secret@192.168.0.2:554/Streaming/Channels/101 -f mpegts /evil";
        let cmd = build_record_command(&cfg, &cam, url, dir);

        assert_eq!(
            cmd.as_std().get_program().to_string_lossy(),
            "/opt/heldar/ffmpeg"
        );
        let args = command_args(&cmd);
        // Stream-copy (no decode), RTSP-over-TCP, fragmented-MP4 segmenting at the camera's interval.
        assert!(has_pair(&args, "-c", "copy"), "must stream-copy: {args:?}");
        assert!(has_pair(&args, "-rtsp_transport", "tcp"));
        assert!(has_pair(&args, "-f", "segment"));
        assert!(has_pair(&args, "-segment_time", "60")); // segment_seconds
        assert!(args.iter().any(|a| a.contains("frag_keyframe")));
        // The URL is passed as a SINGLE argv element right after `-i` — the injection-safety guarantee
        // (the whitespace in `url` is NOT split into extra ffmpeg arguments).
        assert!(
            has_pair(&args, "-i", url),
            "the whole URL must be one arg after -i: {args:?}"
        );
        // Video-only when record_audio is false.
        assert!(args.iter().any(|a| a == "-an"));
        assert!(!has_pair(&args, "-c:a", "copy"));
        // The segment output pattern lands under `dir`.
        assert!(args.last().unwrap().ends_with("%Y%m%d_%H%M%S.mp4"));
        // Segments are timestamped in UTC regardless of the host timezone.
        let tz = cmd
            .as_std()
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("TZ"))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned());
        assert_eq!(tz.as_deref(), Some("UTC"));
    }

    #[test]
    fn build_record_command_passes_audio_through_when_enabled() {
        let cfg = Config::from_env();
        let mut cam = test_camera();
        cam.record_audio = true;
        let cmd = build_record_command(&cfg, &cam, "rtsp://x/s", std::path::Path::new("/d"));
        let args = command_args(&cmd);
        assert!(
            has_pair(&args, "-c:a", "copy"),
            "audio pass-through: {args:?}"
        );
        assert!(!args.iter().any(|a| a == "-an"));
    }

    // ---- supervision LIFECYCLE (drives run_supervise with a fake ffmpeg; the plumbing the pure -----
    // ---- decision-logic tests above don't cover: spawn -> status write, crash -> reconnect bookkeeping)

    async fn mem_pool_migrated() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    /// A hikvision camera with an address so `record_url` is Some (the loop spawns ffmpeg). The
    /// address is TEST-NET-1 (192.0.2.0/24, RFC5737) and unreachable — but the fake ffmpeg never
    /// connects, so that doesn't matter. Other columns default (enabled/record_enabled=1, continuous).
    async fn insert_recordable_camera(pool: &SqlitePool, id: &str) {
        sqlx::query(
            "INSERT INTO cameras (id, name, vendor, address, created_at, updated_at)
             VALUES (?, ?, 'hikvision', '192.0.2.10', ?, ?)",
        )
        .bind(id)
        .bind(format!("Cam {id}"))
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(pool)
        .await
        .unwrap();
    }

    fn tmp_recordings_dir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("heldar-rec-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Poll `f` until it returns true or ~4s elapses (deterministic events happen in ms; the budget is
    /// generous slack, and assertions are monotonic so a re-spawn between polls can't flake them).
    async fn poll_until<F, Fut>(mut f: F) -> bool
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        for _ in 0..160 {
            if f().await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        false
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_supervise_records_error_state_when_ffmpeg_is_missing() {
        let pool = mem_pool_migrated().await;
        insert_recordable_camera(&pool, "cam_err").await;
        let mut cfg = Config::from_env();
        cfg.ffmpeg_bin = "/nonexistent/heldar-ffmpeg-should-not-exist".into();
        cfg.recordings_dir = tmp_recordings_dir("err");
        let mgr = RecorderManager::new(pool.clone(), std::sync::Arc::new(cfg));

        mgr.spawn("cam_err".to_string()).await;
        // A failed spawn must land camera_status.state='error' (not panic the task, not leave it blank).
        let p = pool.clone();
        let reached_error = poll_until(|| {
            let p = p.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT state FROM camera_status WHERE camera_id = 'cam_err'",
                )
                .fetch_optional(&p)
                .await
                .ok()
                .flatten()
                .as_deref()
                    == Some("error")
            }
        })
        .await;
        mgr.stop("cam_err").await;

        assert!(
            reached_error,
            "a missing ffmpeg must set camera_status.state='error'"
        );
        let err: Option<String> =
            sqlx::query_scalar("SELECT last_error FROM camera_status WHERE camera_id = 'cam_err'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            err.unwrap_or_default().contains("spawn ffmpeg failed"),
            "last_error should explain the spawn failure"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_supervise_bumps_reconnect_and_logs_offline_on_child_crash() {
        let pool = mem_pool_migrated().await;
        insert_recordable_camera(&pool, "cam_crash").await;
        let mut cfg = Config::from_env();
        // `false` spawns (via PATH) and exits non-zero immediately — a crashing ffmpeg child.
        cfg.ffmpeg_bin = "false".into();
        cfg.recordings_dir = tmp_recordings_dir("crash");
        let mgr = RecorderManager::new(pool.clone(), std::sync::Arc::new(cfg));

        mgr.spawn("cam_crash".to_string()).await;
        // On the child exiting, the supervisor bumps reconnect_count (state 'offline') and logs a
        // 'camera_offline' event — the observability an operator relies on to see a flapping camera.
        let p = pool.clone();
        let reconnected = poll_until(|| {
            let p = p.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT reconnect_count FROM camera_status WHERE camera_id = 'cam_crash'",
                )
                .fetch_optional(&p)
                .await
                .ok()
                .flatten()
                .unwrap_or(0)
                    >= 1
            }
        })
        .await;
        mgr.stop("cam_crash").await;

        assert!(
            reconnected,
            "a crashing ffmpeg child must bump reconnect_count"
        );
        let offline_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE camera_id = 'cam_crash' AND event_type = 'camera_offline'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            offline_events >= 1,
            "a crash must log a camera_offline event"
        );
    }

    /// Spawn a real FFmpeg writing the SHIPPED segment arguments, and interrupt it mid-segment.
    ///
    /// `lavfi` rather than RTSP: the muxer behaviour under interruption is what is being tested, and
    /// standing up MediaMTX would add a second thing that can fail without testing anything more.
    /// The `-f segment` / `-segment_format mp4` / `movflags` triple is copied from
    /// `build_record_command`, and `the_segment_arguments_match_the_shipped_ones` below fails if
    /// they drift apart.
    #[cfg(unix)]
    async fn write_then_interrupt(dir: &std::path::Path, graceful: bool) -> Vec<(u64, bool)> {
        let pattern = dir.join("%Y%m%d_%H%M%S.mp4");
        let mut child = Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i", "testsrc=size=320x180:rate=15"])
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-g",
                "30",
                "-pix_fmt",
                "yuv420p",
            ])
            .args(["-f", "segment"])
            .args(["-segment_time", "4"])
            .args(["-segment_format", "mp4"])
            .args([
                "-segment_format_options",
                "movflags=+frag_keyframe+empty_moov+default_base_moof",
            ])
            .args(["-reset_timestamps", "1", "-strftime", "1"])
            .arg(&pattern)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ffmpeg");

        // Wait until a second segment has been opened, so one is complete and one is mid-flight.
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0) >= 2 {
                break;
            }
        }
        if graceful {
            finish_and_stop(&mut child, "cam_test").await;
        } else {
            let _ = child.kill().await;
        }

        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        files.sort();
        files
            .iter()
            .map(|f| {
                let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
                let playable = std::process::Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-show_entries",
                        "format=duration",
                        "-of",
                        "csv=p=0",
                    ])
                    .arg(f)
                    .output()
                    .map(|o| o.status.success() && !o.stdout.is_empty())
                    .unwrap_or(false);
                (size, playable)
            })
            .collect()
    }

    /// THE BUG (#167): SIGKILL leaves the segment being written truncated and unplayable, so the
    /// seconds already captured in it are lost. `finish_and_stop` asks FFmpeg to close it first.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_graceful_stop_keeps_the_segment_being_written() {
        // Skipped rather than failed where ffmpeg is absent — CI installs it, a laptop may not.
        let have = |bin: &str| {
            std::process::Command::new(bin)
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        if !have("ffmpeg") || !have("ffprobe") {
            eprintln!("skipping a_graceful_stop_keeps_the_segment_being_written: ffmpeg absent");
            return;
        }
        let base =
            std::env::temp_dir().join(format!("heldar_sig_{}", uuid::Uuid::new_v4().simple()));
        let (graceful_dir, killed_dir) = (base.join("graceful"), base.join("killed"));
        std::fs::create_dir_all(&graceful_dir).unwrap();
        std::fs::create_dir_all(&killed_dir).unwrap();

        let graceful = write_then_interrupt(&graceful_dir, true).await;
        let killed = write_then_interrupt(&killed_dir, false).await;
        let _ = std::fs::remove_dir_all(&base);

        assert!(
            graceful.len() >= 2 && killed.len() >= 2,
            "the probe did not produce a complete segment plus one in flight \
             (graceful={graceful:?} killed={killed:?})"
        );

        // NEGATIVE CONTROL, in the same test: without it, a change that made every segment playable
        // for some unrelated reason would leave the assertion below passing while proving nothing.
        let (killed_size, killed_playable) = *killed.last().unwrap();
        assert!(
            !killed_playable,
            "SIGKILL left a PLAYABLE final segment ({killed_size} bytes) — the bug this guards \
             against no longer reproduces, so the assertion below is no longer evidence"
        );

        let (size, playable) = *graceful.last().unwrap();
        assert!(
            playable,
            "the segment FFmpeg was writing when it was asked to stop is unplayable ({size} bytes); \
             those seconds were captured and thrown away (#167)"
        );
    }

    /// The probe above copies the muxer arguments; this fails if the shipped ones move.
    #[test]
    fn the_segment_arguments_match_the_shipped_ones() {
        let cfg = Config::from_env();
        let mut cam = test_camera();
        cam.segment_seconds = 4;
        let cmd =
            build_record_command(&cfg, &cam, "rtsp://example/x", std::path::Path::new("/tmp"));
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for needed in [
            "segment",
            "mp4",
            "movflags=+frag_keyframe+empty_moov+default_base_moof",
        ] {
            assert!(
                args.iter().any(|a| a == needed),
                "the shipped recorder no longer passes {needed:?}; the interruption probe in this \
                 file is testing a muxer configuration the product does not use: {args:?}"
            );
        }
    }
}
