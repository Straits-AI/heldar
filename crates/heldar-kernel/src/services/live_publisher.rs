//! Kernel-owned live preview publishers: the HEVC→H.264 transcode ffmpeg that feeds each camera's
//! MediaMTX `cam_<id>` path is spawned and supervised HERE, next to the recorder/sampler ffmpegs —
//! never by MediaMTX.
//!
//! Why the kernel owns this process (and MediaMTX's `runOnDemand` is deliberately NOT used): the
//! kernel already requires host ffmpeg for recording and AI sampling, so the dependency is proven
//! wherever the kernel runs. MediaMTX's exec environment is NOT ours to assume — the recommended
//! docker-compose deployment runs the official `bluenviron/mediamtx` image, which ships no ffmpeg,
//! so a `runOnDemand` command dies instantly inside that container and live view silently never
//! starts. Owning the process also gives real supervision (backoff, restart-on-config-change,
//! teardown on disable/delete) instead of fire-and-forget shell strings.
//!
//! Lifecycle per camera:
//! - **On demand** (default): a viewer's `ensure_live` calls [`LivePublisherManager::demand`]; the
//!   reconcile loop reaps the publisher once MediaMTX reports no readers and no demand has been
//!   seen for `live_idle_close_secs` (the reap re-checks demand under the lock, so a viewer who
//!   arrives mid-tick is never cut).
//! - **Warm** (`cameras.live_warm`): the publisher runs persistently (instant live view — the
//!   product replacement for hand-rolled warming scripts); the reconcile loop (re)starts it.
//! - A camera PATCH/DELETE calls [`LivePublisherManager::reconcile`]; the periodic loop is the
//!   self-healing backstop (MediaMTX restarts, engine changes, missed nudges) per principle 3.
//!   All mutators — the hook, the loop's per-camera step, and a viewer's `demand` — are SERIALIZED
//!   per camera: each acquires that camera's async lock and re-reads the row inside it, so
//!   decisions are linearized on fresh state and a hook can never race the loop into resurrecting
//!   a disabled/deleted camera (there is no unsynchronized read-then-act window at all).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::SqlitePool;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use crate::camera_url;
use crate::config::Config;
use crate::models::Camera;
use crate::services::mediamtx;

/// Bounded stderr tail kept per publisher (same cap as the recorder).
const STDERR_TAIL_CAP: usize = 8192;

struct PublisherTask {
    stop: watch::Sender<bool>,
    handle: JoinHandle<()>,
    /// The exact argv this publisher runs (display form) — compared on reconcile so an engine,
    /// credential, or source change restarts the process with fresh config.
    command: String,
    /// Last time a viewer demanded this camera. Warm cameras ignore it.
    last_demand: Arc<std::sync::Mutex<Instant>>,
}

/// Owns and supervises the per-camera live-transcode publisher processes.
pub struct LivePublisherManager {
    pool: SqlitePool,
    cfg: Arc<Config>,
    http: reqwest::Client,
    tasks: Mutex<HashMap<String, PublisherTask>>,
    /// Per-camera serialization locks: every mutator (hook reconcile, loop step, viewer demand)
    /// holds the camera's lock across its fresh-row read AND the resulting action, linearizing
    /// decisions. Lock order is always camera lock → `tasks` lock, never nested camera locks.
    /// Entries are never removed — the map is bounded by the set of camera ids ever seen, and a
    /// stable `Arc` per id is what makes the serialization airtight.
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Set by [`Self::shutdown`]: no new publisher may spawn afterwards, so a reconcile tick or an
    /// in-flight `ensure_live` racing graceful shutdown cannot orphan an ffmpeg.
    shutting_down: AtomicBool,
}

/// Build the publisher argv: pull the camera's stream and republish H.264 to MediaMTX. Returned as
/// discrete args (never passed through a shell); `display` joins them for comparison/logging.
fn publish_args(
    source: &str,
    audio_args: &str,
    codec_args: &str,
    publish_url: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-nostdin".into(),
        "-rtsp_transport".into(),
        "tcp".into(),
        "-timeout".into(),
        "10000000".into(),
        "-i".into(),
        source.into(),
    ];
    args.extend(audio_args.split_whitespace().map(String::from));
    args.extend(codec_args.split_whitespace().map(String::from));
    args.extend(["-f".into(), "rtsp".into(), publish_url.into()]);
    args
}

/// The RTSP base the KERNEL publishes to. `mediamtx_rtsp_base` is the CLIENT-facing base and may be
/// operator-overridden to an external name (a CDN, a reverse proxy) — but the publisher must reach
/// the actual MediaMTX instance the kernel manages, which is the host of `mediamtx_api_url`. Take
/// the host from the API URL and the port from the RTSP base (default 8554). MediaMTX's publish
/// auth is loopback-only, so publishing to an external client base would be rejected anyway.
fn publish_base(api_url: &str, rtsp_base: &str) -> String {
    fn authority(url: &str) -> Option<&str> {
        let rest = url.split_once("://")?.1;
        Some(rest.split('/').next().unwrap_or(rest))
    }
    fn port_of(auth: &str) -> Option<&str> {
        if let Some(rest) = auth.strip_prefix('[') {
            return rest.split_once(']')?.1.strip_prefix(':');
        }
        auth.rsplit_once(':').map(|(_, p)| p)
    }
    fn host_of(auth: &str) -> &str {
        if auth.starts_with('[') {
            if let Some(end) = auth.find(']') {
                return &auth[..=end];
            }
        }
        auth.rsplit_once(':').map_or(auth, |(h, _)| h)
    }
    let api_host = authority(api_url).map(host_of).unwrap_or("127.0.0.1");
    let rtsp_port = authority(rtsp_base).and_then(port_of).unwrap_or("8554");
    format!("rtsp://{api_host}:{rtsp_port}")
}

/// Restart backoff: doubling capped at 30s; a run that survived ≥60s resets to 1s.
fn next_backoff(prev: u64, ran_seconds: i64) -> u64 {
    if ran_seconds >= 60 {
        1
    } else {
        (prev * 2).clamp(2, 30)
    }
}

impl LivePublisherManager {
    pub fn new(pool: SqlitePool, cfg: Arc<Config>, http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            pool,
            cfg,
            http,
            tasks: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            shutting_down: AtomicBool::new(false),
        })
    }

    /// The desired publisher argv for a camera under the CURRENT effective transcode engine, or
    /// `None` when the camera has no usable stream URL. Prefers the sub stream (preview-quality).
    async fn desired_args(&self, cam: &Camera) -> Option<Vec<String>> {
        let source = camera_url::stream_url(cam, "sub").or_else(|| camera_url::record_url(cam))?;
        let audio = if cam.record_audio {
            "-c:a aac -b:a 96k"
        } else {
            "-an"
        };
        let codec = mediamtx::effective_codec_args(&self.pool, &self.cfg).await;
        let publish_url = format!(
            "{}/cam_{}",
            publish_base(&self.cfg.mediamtx_api_url, &self.cfg.mediamtx_rtsp_base),
            cam.id
        );
        Some(publish_args(&source, audio, &codec, &publish_url))
    }

    /// The camera's serialization lock (created on first use, stable thereafter).
    async fn camera_lock(&self, camera_id: &str) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .await
            .entry(camera_id.to_string())
            .or_default()
            .clone()
    }

    /// A viewer wants this camera: record demand and ensure a publisher is running with current
    /// config. Called from `ensure_live`. Serialized per camera and re-reads the row inside the
    /// lock, so a PATCH/DELETE landing mid-request can never be raced into starting a publisher
    /// for a camera that is no longer enabled.
    pub async fn demand(self: &Arc<Self>, camera_id: &str) {
        let lock = self.camera_lock(camera_id).await;
        let _g = lock.lock().await;
        let cam: Option<Camera> = sqlx::query_as("SELECT * FROM cameras WHERE id = ?")
            .bind(camera_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten();
        match cam {
            Some(cam) if cam.enabled => self.ensure_running_inner(&cam, true).await,
            _ => {}
        }
    }

    /// Ensure a publisher is running with current config WITHOUT recording viewer demand (the
    /// reconcile loop's form — recording demand here would reset the idle clock every tick and
    /// on-demand publishers would never reap). Restarts the publisher when its config drifted
    /// (engine/credential/source change).
    pub async fn ensure_running(self: &Arc<Self>, cam: &Camera) {
        self.ensure_running_inner(cam, false).await;
    }

    async fn ensure_running_inner(self: &Arc<Self>, cam: &Camera, touch_demand: bool) {
        if self.shutting_down.load(Ordering::Relaxed) {
            return;
        }
        let Some(args) = self.desired_args(cam).await else {
            tracing::warn!(camera_id = %cam.id, "live: camera has no stream URL; not publishing");
            return;
        };
        let display = format!("{} {}", self.cfg.ffmpeg_bin, args.join(" "));
        {
            let tasks = self.tasks.lock().await;
            if let Some(t) = tasks.get(&cam.id) {
                if t.command == display && !t.handle.is_finished() {
                    if touch_demand {
                        *t.last_demand.lock().unwrap() = Instant::now();
                    }
                    return;
                }
            }
        }
        // (Re)start: config changed, task died, or never started.
        self.stop(&cam.id).await;
        self.spawn(cam.id.clone(), args, display).await;
    }

    /// Reconcile one camera after a create/update/delete. Missing or disabled → stop the publisher
    /// and remove the MediaMTX path (a disabled camera has no live surface, so viewers are cut).
    /// Warm → (re)started immediately. Enabled on-demand → restarted only if it was already running
    /// (keeps current viewers on fresh config; otherwise stays down until demanded).
    pub async fn reconcile(self: &Arc<Self>, camera_id: &str) {
        let lock = self.camera_lock(camera_id).await;
        let _g = lock.lock().await;
        self.reconcile_locked(camera_id).await;
    }

    /// The hook-reconcile body; caller MUST hold the camera's serialization lock.
    async fn reconcile_locked(self: &Arc<Self>, camera_id: &str) {
        let cam: Option<Camera> = sqlx::query_as("SELECT * FROM cameras WHERE id = ?")
            .bind(camera_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten();
        match cam {
            None => {
                self.stop(camera_id).await;
                let api = self.cfg.mediamtx_api_url.trim_end_matches('/');
                mediamtx::delete_path(&self.http, api, &format!("cam_{camera_id}")).await;
            }
            Some(cam) if !cam.enabled => {
                self.stop(&cam.id).await;
                let api = self.cfg.mediamtx_api_url.trim_end_matches('/');
                mediamtx::delete_path(&self.http, api, &format!("cam_{}", cam.id)).await;
            }
            Some(cam) if cam.should_warm() => self.ensure_running(&cam).await,
            Some(cam) => {
                let was_running = { self.tasks.lock().await.contains_key(&cam.id) };
                if was_running {
                    self.ensure_running(&cam).await;
                }
            }
        }
    }

    /// The supervised reconcile loop: starts warm publishers, restarts drifted configs, reaps idle
    /// on-demand publishers (only when MediaMTX confirms zero readers), and stops publishers whose
    /// camera vanished or was disabled. First tick fires immediately, so warm cameras come up at boot.
    pub async fn run(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if self.shutting_down.load(Ordering::Relaxed) {
                return;
            }
            self.reconcile_all().await;
        }
    }

    async fn reconcile_all(self: &Arc<Self>) {
        // A transient DB error must NOT read as "no cameras exist" — that would stop every
        // publisher and delete every path. Skip the tick and retry in 30s instead.
        let warm_ids: Vec<String> = match sqlx::query_scalar::<_, String>(
            "SELECT id FROM cameras WHERE enabled = 1 AND live_warm = 1",
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!(error = %e, "live: reconcile skipped (camera query failed)");
                return;
            }
        };
        let running_ids: Vec<String> = { self.tasks.lock().await.keys().cloned().collect() };
        let all_ids: HashSet<String> = warm_ids.into_iter().chain(running_ids).collect();

        let idle_close = Duration::from_secs(self.cfg.live_idle_close_secs);
        let api = self.cfg.mediamtx_api_url.trim_end_matches('/').to_string();
        for id in all_ids {
            if self.shutting_down.load(Ordering::Relaxed) {
                return;
            }
            // Serialize with the PATCH/DELETE hooks and viewer demand: hold the camera's lock
            // across the fresh-row read AND the action, so this step can never act on state a
            // hook has already superseded (no resurrection window at all).
            let lock = self.camera_lock(&id).await;
            let _g = lock.lock().await;
            let cam: Result<Option<Camera>, _> =
                sqlx::query_as("SELECT * FROM cameras WHERE id = ?")
                    .bind(&id)
                    .fetch_optional(&self.pool)
                    .await;
            match cam {
                Err(e) => {
                    tracing::warn!(camera_id = %id, error = %e, "live: reconcile read failed; skipping");
                }
                Ok(None) => {
                    // Camera deleted: stop the publisher AND remove the path (the delete hook's
                    // one-shot cleanup may have raced an in-flight respawn).
                    self.stop(&id).await;
                    mediamtx::delete_path(&self.http, &api, &format!("cam_{id}")).await;
                }
                Ok(Some(cam)) if !cam.enabled => {
                    self.stop(&id).await;
                    mediamtx::delete_path(&self.http, &api, &format!("cam_{id}")).await;
                }
                Ok(Some(cam)) if cam.should_warm() => {
                    // Warm: keep running; also picks up engine/credential drift.
                    self.ensure_running(&cam).await;
                }
                Ok(Some(cam)) => {
                    // Enabled, on-demand. Only relevant if a publisher is running.
                    let running = { self.tasks.lock().await.contains_key(&id) };
                    if !running {
                        continue;
                    }
                    let readers =
                        mediamtx::path_readers(&self.http, &api, &format!("cam_{id}")).await;
                    match readers {
                        Some(n) if n > 0 => {
                            // Viewers attached: that IS demand. Refresh it, and apply config
                            // drift (an engine change restarts the publisher — viewers see a
                            // brief reconnect, which is the documented behavior).
                            {
                                let tasks = self.tasks.lock().await;
                                if let Some(t) = tasks.get(&id) {
                                    *t.last_demand.lock().unwrap() = Instant::now();
                                }
                            }
                            self.ensure_running(&cam).await;
                        }
                        Some(_) => {
                            // Confirmed zero readers: reap ONLY if still idle at the moment of
                            // removal (re-checked under the lock — a viewer who demanded the
                            // stream after this tick started is never cut).
                            if self.stop_if_idle(&id, idle_close).await {
                                tracing::info!(camera_id = %id, "live: reaped idle publisher");
                            } else {
                                // Fresh demand or not yet idle: apply config drift instead.
                                self.ensure_running(&cam).await;
                            }
                        }
                        None => {
                            // MediaMTX unreachable / path missing: unknown viewer state — never
                            // reap blind. The supervise loop re-adds a missing path itself.
                        }
                    }
                }
            }
        }
    }

    /// Run one reconcile pass NOW (spawned; non-blocking for the caller). Used when a runtime
    /// setting that publishers embed (the transcode engine) changes, so it applies in seconds
    /// instead of waiting for the periodic tick.
    pub fn poke(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            me.reconcile_all().await;
        });
    }

    /// Stop a camera's publisher (kills its ffmpeg). Returns once the task is gone.
    pub async fn stop(self: &Arc<Self>, camera_id: &str) {
        let task = { self.tasks.lock().await.remove(camera_id) };
        if let Some(task) = task {
            Self::kill_task(camera_id, task).await;
        }
    }

    /// Remove and stop the publisher ONLY if its last demand is at least `idle_close` old at the
    /// moment of removal (checked under the tasks lock). Returns whether it was reaped.
    async fn stop_if_idle(self: &Arc<Self>, camera_id: &str, idle_close: Duration) -> bool {
        let task = {
            let mut tasks = self.tasks.lock().await;
            let idle = tasks
                .get(camera_id)
                .map(|t| t.last_demand.lock().unwrap().elapsed() >= idle_close)
                .unwrap_or(false);
            if idle {
                tasks.remove(camera_id)
            } else {
                None
            }
        };
        match task {
            Some(task) => {
                Self::kill_task(camera_id, task).await;
                true
            }
            None => false,
        }
    }

    async fn kill_task(camera_id: &str, task: PublisherTask) {
        let _ = task.stop.send(true);
        let mut handle = task.handle;
        if tokio::time::timeout(Duration::from_secs(8), &mut handle)
            .await
            .is_err()
        {
            tracing::warn!(%camera_id, "live: publisher did not stop within 8s; aborting");
            handle.abort();
            let _ = handle.await;
        }
    }

    /// Stop all publishers and refuse new spawns (graceful shutdown). The latch is checked by
    /// `ensure_running`/`spawn`/the reconcile loop, so nothing respawns after this returns.
    pub async fn shutdown(self: &Arc<Self>) {
        self.shutting_down.store(true, Ordering::Relaxed);
        let ids: Vec<String> = { self.tasks.lock().await.keys().cloned().collect() };
        if !ids.is_empty() {
            tracing::info!(count = ids.len(), "live: shutting down publishers");
        }
        for id in ids {
            self.stop(&id).await;
        }
    }

    async fn spawn(self: &Arc<Self>, camera_id: String, args: Vec<String>, display: String) {
        if self.shutting_down.load(Ordering::Relaxed) {
            return;
        }
        let (tx, rx) = watch::channel(false);
        let last_demand = Arc::new(std::sync::Mutex::new(Instant::now()));
        let mut tasks = self.tasks.lock().await;
        let me = self.clone();
        let id_for_task = camera_id.clone();
        let handle = tokio::spawn(async move {
            me.supervise(id_for_task, args, rx).await;
        });
        if let Some(old) = tasks.insert(
            camera_id,
            PublisherTask {
                stop: tx,
                handle,
                command: display,
                last_demand,
            },
        ) {
            // Displaced a previous task: signal AND abort so two publishers never overlap.
            let _ = old.stop.send(true);
            old.handle.abort();
        }
    }

    /// Per-camera supervise loop: ensure the MediaMTX path exists (plain — no exec config), spawn
    /// the ffmpeg publisher, restart with backoff on exit, honor the stop signal. Self-heals a
    /// MediaMTX restart (which loses API-added paths) by re-adding the path before each attempt.
    async fn supervise(
        self: Arc<Self>,
        camera_id: String,
        args: Vec<String>,
        mut stop: watch::Receiver<bool>,
    ) {
        let api = self.cfg.mediamtx_api_url.trim_end_matches('/').to_string();
        let name = format!("cam_{camera_id}");
        let mut backoff: u64 = 1;
        loop {
            if *stop.borrow() {
                return;
            }
            if let Err(e) = mediamtx::ensure_plain_path(&self.http, &api, &name).await {
                tracing::warn!(%camera_id, error = %e, "live: MediaMTX path setup failed; retrying");
                if sleep_or_stop(&mut stop, backoff).await {
                    return;
                }
                backoff = next_backoff(backoff, 0);
                continue;
            }

            let masked = camera_url::mask_url(args.join(" ").as_str());
            tracing::info!(%camera_id, cmd = %masked, "live: starting publisher");
            let mut child = match Command::new(&self.cfg.ffmpeg_bin)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(%camera_id, error = %e, "live: spawn ffmpeg failed");
                    if sleep_or_stop(&mut stop, 15).await {
                        return;
                    }
                    continue;
                }
            };

            // Drain stderr concurrently, keeping a bounded tail (never blocks ffmpeg).
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

            let started = Instant::now();
            tokio::select! {
                status = child.wait() => {
                    let raw = String::from_utf8_lossy(&stderr_task.await.unwrap_or_default())
                        .trim().to_string();
                    let err_tail = camera_url::mask_url(&raw);
                    let ran = started.elapsed().as_secs() as i64;
                    match status {
                        Ok(s) if s.success() =>
                            tracing::warn!(%camera_id, ran_s = ran, "live: publisher exited (stream ended)"),
                        Ok(s) =>
                            tracing::warn!(%camera_id, ran_s = ran, code = ?s.code(), tail = %err_tail, "live: publisher exited with error"),
                        Err(e) =>
                            tracing::error!(%camera_id, error = %e, "live: publisher wait failed"),
                    }
                    backoff = next_backoff(backoff, ran);
                    if sleep_or_stop(&mut stop, backoff).await {
                        return;
                    }
                }
                _ = stop.changed() => {
                    tracing::info!(%camera_id, "live: stop requested");
                    let _ = child.kill().await;
                    return;
                }
            }
        }
    }
}

/// Sleep `secs`, returning `true` early if the stop signal fires.
async fn sleep_or_stop(stop: &mut watch::Receiver<bool>, secs: u64) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(secs)) => false,
        _ = stop.changed() => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_args_shape_video_only_and_audio() {
        let a = publish_args(
            "rtsp://u:p@10.0.0.9:554/s",
            "-an",
            "-c:v libx264 -g 30",
            "rtsp://127.0.0.1:8554/cam_x",
        );
        let joined = a.join(" ");
        assert!(joined.starts_with(
            "-nostdin -rtsp_transport tcp -timeout 10000000 -i rtsp://u:p@10.0.0.9:554/s"
        ));
        assert!(joined.contains(" -an "));
        assert!(joined.contains("-c:v libx264"));
        assert!(joined.ends_with("-f rtsp rtsp://127.0.0.1:8554/cam_x"));
        // The source URL stays ONE argv element — never shell-split or interpolated into a shell.
        assert!(a.contains(&"rtsp://u:p@10.0.0.9:554/s".to_string()));

        let with_audio = publish_args(
            "rtsp://s",
            "-c:a aac -b:a 96k",
            "-c:v h264_nvenc",
            "rtsp://l/p",
        );
        let j = with_audio.join(" ");
        assert!(j.contains("-c:a aac -b:a 96k"));
        assert!(!j.contains("-an"));
    }

    /// The publish target follows the MediaMTX instance (the API URL's host), never the
    /// client-facing RTSP base's host — an operator pointing `HELDAR_MEDIAMTX_RTSP_BASE` at a CDN
    /// must not redirect the kernel's own publisher off-box (publish auth is loopback-only).
    #[test]
    fn publish_base_uses_api_host_and_rtsp_port() {
        // default config
        assert_eq!(
            publish_base("http://127.0.0.1:9997", "rtsp://127.0.0.1:8554"),
            "rtsp://127.0.0.1:8554"
        );
        // client-facing base overridden to an external name → publisher still targets the box
        assert_eq!(
            publish_base("http://127.0.0.1:9997", "rtsp://cdn.example.com:8554"),
            "rtsp://127.0.0.1:8554"
        );
        // MediaMTX on another host (api url remote) → publish there
        assert_eq!(
            publish_base("http://10.0.0.5:9997", "rtsp://10.0.0.5:9554"),
            "rtsp://10.0.0.5:9554"
        );
        // no port on the rtsp base → default 8554; trailing path on api url tolerated
        assert_eq!(
            publish_base("http://127.0.0.1:9997/", "rtsp://box.local"),
            "rtsp://127.0.0.1:8554"
        );
        // ipv6 api host is preserved bracketed
        assert_eq!(
            publish_base("http://[::1]:9997", "rtsp://[::1]:8554"),
            "rtsp://[::1]:8554"
        );
    }

    #[test]
    fn backoff_doubles_caps_and_resets_after_healthy_run() {
        assert_eq!(next_backoff(1, 0), 2);
        assert_eq!(next_backoff(2, 5), 4);
        assert_eq!(next_backoff(16, 5), 30); // capped
        assert_eq!(next_backoff(30, 5), 30);
        assert_eq!(next_backoff(30, 61), 1); // healthy run resets
    }
}
