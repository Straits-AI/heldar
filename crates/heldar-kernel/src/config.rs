use std::path::PathBuf;

use crate::env::{parse_bool, parse_or, var, var_or};

/// Runtime configuration, loaded from environment (see `.env.example`).
#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub data_dir: PathBuf,
    pub recordings_dir: PathBuf,
    pub clips_dir: PathBuf,
    pub snapshots_dir: PathBuf,
    pub frames_dir: PathBuf,
    /// Directory where segment-spanning HLS playback sessions are generated (one subdir per session).
    pub playback_dir: PathBuf,
    pub ffmpeg_bin: String,
    pub ffprobe_bin: String,
    pub mediamtx_api_url: String,
    pub mediamtx_hls_base: String,
    pub mediamtx_rtsp_base: String,
    pub mediamtx_webrtc_base: String,
    /// Max SQLite pool connections. Tunable per deployment: more absorbs bursts of concurrent
    /// requests (WAL serves reads concurrently; writes still serialize), at the cost of memory.
    pub db_max_connections: u32,
    pub recorder_enabled: bool,
    /// Optional second recordings root for dual/mirror recording. When set, cameras with
    /// `mirror_enabled` get a SECOND ffmpeg pipeline writing byte-identical segments here (a redundant
    /// DVR copy on a separate volume). Empty/unset disables mirror recording entirely.
    pub mirror_recordings_dir: Option<PathBuf>,
    /// Master switch for ANR (Automatic Network Replenishment) edge re-fill: re-fetch missed footage
    /// from a camera's onboard storage to fill recording gaps. Cameras still need `anr_enabled`.
    pub anr_enabled: bool,
    /// How often the ANR loop scans for pending gaps to fill (seconds).
    pub anr_interval_s: u64,
    /// Ignore gaps older than this many hours (most cameras only retain recent onboard footage).
    pub anr_max_gap_hours: i64,
    /// Give up on a gap after this many fill attempts (marked `failed`).
    pub anr_max_attempts: i64,
    pub default_segment_seconds: i64,
    pub default_retention_hours: i64,
    /// Default per-camera storage quota (bytes) applied when a camera is created without an explicit
    /// `storage_quota_bytes`. 0 means no default quota (the camera's quota is stored as NULL).
    pub default_camera_quota_bytes: u64,
    /// Default audio-recording toggle applied when a camera is created without an explicit
    /// `record_audio`. When false (default) the recorder drops audio (video only).
    pub default_record_audio: bool,
    /// Default pre-roll seconds applied when a camera is created without an explicit
    /// `pre_roll_seconds` (event / scheduled_event recording). Clamped to 0..300 in handlers.
    pub default_pre_roll_seconds: i64,
    /// Default post-roll seconds (the trigger recording window) applied when a camera is created
    /// without an explicit `post_roll_seconds`. Clamped to 0..3600 in handlers.
    pub default_post_roll_seconds: i64,
    pub indexer_interval_s: u64,
    pub health_interval_s: u64,
    pub retention_interval_s: u64,
    pub api_host: String,
    pub api_port: u16,
    pub cors_origins: Vec<String>,
    /// Soft cap on total recording footprint; oldest unlocked segments are pruned above this.
    pub max_recordings_bytes: u64,
    /// Hard floor on free disk space; when free space drops below this, oldest unlocked segments
    /// are pruned regardless of age/size policy (protects the host from a full disk).
    pub min_free_disk_bytes: u64,
    /// How often the alert notifier polls for new events to deliver.
    pub notifier_interval_s: u64,
    /// Master switch for AI frame sampling (Stage 2). Cameras still need an enabled AI task.
    pub ai_enabled: bool,
    /// Global frame-sampling budget (frames/sec summed across all cameras); per-camera fps is
    /// reduced proportionally above this so adding AI cameras degrades fps instead of overloading.
    pub ai_max_total_fps: f64,
    pub default_ai_fps: f64,
    pub default_ai_width: i64,
    /// How long detection rows are kept before the retention sweeper prunes them.
    pub detection_retention_hours: i64,
    // ---- Scheduled interval snapshots ----
    /// Master switch for the background snapshot scheduler (interval live-frame captures).
    pub snapshot_scheduler_enabled: bool,
    /// How often the scheduler ticks to look for due schedules (seconds).
    pub snapshot_scheduler_interval_s: u64,
    /// How long captured snapshots are kept before the retention sweeper prunes them. 0 = no pruning.
    pub snapshot_retention_hours: i64,
    // ---- Per-camera recording schedule (time-of-day windows) ----
    /// How often the schedule watcher ticks to open/close recording windows for `scheduled` /
    /// `scheduled_event` cameras (seconds). Windows are evaluated against the SERVER's LOCAL timezone.
    pub schedule_check_interval_s: u64,
    // ---- Segment-spanning HLS playback sessions (kernel platform feature) ----
    /// How long a generated playback session (its HLS dir + the segment read-locks it holds) is
    /// retained before the cleanup sweeper removes the dir and releases its locks. Server time.
    pub playback_session_ttl_minutes: i64,
    /// Maximum playback session span (seconds); a longer requested range is rejected (HTTP 400).
    pub max_playback_seconds: f64,
    // ---- Auth / RBAC (kernel platform feature) ----
    /// Master switch for authentication + RBAC. When false, the API is open (dev/single-tenant
    /// LAN appliance default) and a synthetic admin principal is used. When true, the auth/admin
    /// surface requires a valid bearer token (session or API key) and enforces roles.
    pub auth_enabled: bool,
    /// Lifetime of an issued login session token.
    pub session_ttl_hours: i64,
    /// Idle timeout (minutes): a session unused for longer than this is rejected even before its
    /// absolute TTL. 0 (default) disables it. Recommended for internet-exposed remote-dashboard
    /// access (bounds a stolen token's window), paired with a shorter `session_ttl_hours`.
    pub session_idle_timeout_minutes: i64,
    /// Add `Secure` to the session cookie (require HTTPS). Default false for HTTP LAN/overlay
    /// appliances; set true when the deployment is served over TLS.
    pub auth_cookie_secure: bool,
    /// Per-account brute-force lockout: lock an account after this many CONSECUTIVE failed logins
    /// (the per-IP Worker rate limit is complementary). 0 disables account lockout.
    pub login_max_failures: i64,
    /// How long a locked account stays locked (minutes); auto-unlocks after the window. 0 disables.
    pub login_lockout_min: i64,
    /// Base64-encoded 32-byte master key for encryption-at-rest of sensitive fields (camera
    /// credentials). Unset = plaintext at rest (LAN appliance). Installed via `services::secrets`.
    pub secret_key_b64: Option<String>,
    /// Turn the production guardrails (see `enforce_production_guardrails`) into hard boot failures
    /// instead of warnings, for an internet-exposed deployment.
    pub strict_prod: bool,
    /// Optional first-run admin bootstrap (only used when no users exist yet).
    pub bootstrap_admin_user: Option<String>,
    pub bootstrap_admin_password: Option<String>,
    /// How long kernel audit-log + generic-event rows are kept before retention prunes them.
    pub audit_retention_days: i64,
    // ---- Remote-access overlay (kernel platform feature; see docs/REMOTE-ACCESS.md) ----
    /// Whether this deployment is reached through a WireGuard overlay (Tailscale / NetBird /
    /// wireguard) running as an external daemon on the host. The kernel does not manage the
    /// overlay; it only reports whether the configured interface is present + up so the dashboard
    /// can surface remote-access health. When false, the deployment is LAN-only.
    pub overlay_enabled: bool,
    /// Label for the overlay in use: `tailscale` | `netbird` | `wireguard` | `none`.
    pub overlay_kind: String,
    /// The overlay's network interface to probe (e.g. `tailscale0`, `wt0`, `wg0`).
    pub overlay_iface: Option<String>,
    // ---- Backup subsystem (kernel platform feature) ----
    /// Path to the `rclone` binary used for sftp/ftp/s3 remote backups. Local/NAS-mount backups use
    /// std fs copy and never need it; remote backups degrade to a clear job error when it is missing.
    pub rclone_bin: String,
    /// Master switch for the background backup scheduler (scheduled policy jobs). On-demand archive
    /// export still works when this is false.
    pub backup_enabled: bool,
    /// How often the backup scheduler ticks to look for due policies (seconds).
    pub backup_scheduler_interval_s: u64,
    /// Hard timeout for a single backup job's transfer (seconds); a job exceeding it is marked error.
    pub backup_job_timeout_s: u64,
    /// Maximum number of backup jobs running concurrently (a tokio Semaphore bounds the scheduler +
    /// manual triggers).
    pub backup_max_concurrent_jobs: usize,
    /// Where on-demand archive (.zip) exports are written; also served at `/media/archives`.
    pub archive_dir: PathBuf,
    /// Maximum total source footprint (sum of segment sizes) for a single archive export; a larger
    /// selection is rejected (HTTP 400).
    pub archive_max_bytes: u64,
    /// How long archive exports + finished backup-job rows are kept before retention prunes them.
    pub archive_retention_hours: i64,
    // ---- ONVIF (kernel platform feature; Profile S MVP) ----
    /// How long the WS-Discovery probe listens for ProbeMatch replies (milliseconds).
    pub onvif_discovery_timeout_ms: u64,
    /// Per-request timeout for an ONVIF SOAP call (GetDeviceInformation, PTZ, etc.) in milliseconds.
    pub onvif_request_timeout_ms: u64,
    /// Per-request timeout for a HikVision ISAPI camera-config call (HTTP Digest) in milliseconds.
    pub isapi_request_timeout_ms: u64,
    // ---- Disk / array health (HA ops; see docs/HA.md) ----
    /// Run periodic SMART self-assessment checks (`smartctl -H`) inside the health loop. Off by
    /// default; needs `smartmontools` on PATH. Missing binary degrades to a one-time log + skip.
    pub smart_check_enabled: bool,
    /// Block devices to query when SMART checks are enabled (e.g. `/dev/sda,/dev/sdb`).
    pub smart_devices: Vec<String>,
    /// Watch `/proc/mdstat` (Linux md/RAID) and emit `raid_degraded` when an array shows a down member.
    pub mdstat_check_enabled: bool,
    /// Cadence of the disk-health (SMART/RAID) check inside the health loop (seconds).
    pub smart_check_interval_s: u64,
    // ---- Readiness HA probe (see docs/HA.md) ----
    /// When > 0, `/readyz` also requires at least this percent of enabled cameras to be actively
    /// recording (503 `insufficient_recorders` otherwise). 0 (default) keeps DB-connectivity-only.
    pub readyz_min_recording_percent: f64,
    // ---- Live preview transcode (HEVC->H.264) hardware acceleration ----
    /// Encoder engine for the live preview transcode path: `software` (libx264, default), `vaapi`,
    /// or `nvenc`. Unknown values warn and fall back to software.
    pub live_transcode_engine: String,
    /// VAAPI render node used when `live_transcode_engine = vaapi`.
    pub vaapi_device: String,
    // ---- Fleet / multi-site identity ----
    /// Optional site identifier stamped onto outbox rows and surfaced at `GET /api/v1/site` for the
    /// edge->cloud fleet uplink. Empty/unset = a single unnamed site.
    pub site_id: Option<String>,
    /// Control-plane base URL for edge-side self-registration (`HELDAR_CP_URL`). Unset (default) = this
    /// node never phones home; the fleet is opt-in. When set together with `site_id` and
    /// `public_base_url`, the node POSTs its identity to the control plane on boot + on a heartbeat, so
    /// the control plane drains it without any static config or restart.
    pub cp_url: Option<String>,
    /// This node's externally reachable base URL, as the control plane should address it
    /// (`HELDAR_PUBLIC_BASE_URL`, e.g. its overlay/WireGuard address). Required for self-registration —
    /// the node cannot infer it (it binds `0.0.0.0`). Unset → self-registration parks.
    pub public_base_url: Option<String>,
    /// Bearer credential the control plane presents when draining this node's outbox
    /// (`HELDAR_CP_TOKEN`). Empty (default) when this node runs with auth disabled (the LAN default);
    /// when auth is enabled, set it to a valid API key the control plane may use.
    pub cp_token: String,
    /// Heartbeat cadence (seconds) for re-registration with the control plane
    /// (`HELDAR_CP_REGISTER_INTERVAL_S`). Re-registration is idempotent, so the heartbeat also
    /// re-teaches a control plane that restarted or lost its registry.
    pub cp_register_interval_s: u64,
    /// Optional mTLS material for talking to the control plane: this node's client cert + key (to
    /// present when registering) and the CA that signed the control plane's server cert (to verify
    /// it). Required as a set when the control plane enforces mTLS; unset = plain HTTP to the control
    /// plane (the LAN/overlay default).
    pub cp_tls: Option<CpTlsCfg>,
    /// Public WebRTC rendezvous URL (`HELDAR_REMOTE_RENDEZVOUS_URL`) the box dials OUT to for universal
    /// remote viewing (ADR 0003, P2). Unset (default) → the rendezvous client parks; remote access is
    /// opt-in. Reuses `site_id` for identity, `cp_token` as bearer, and `cp_tls` for mTLS.
    pub rendezvous_url: Option<String>,
    /// WebRTC ICE servers (`HELDAR_WEBRTC_ICE_SERVERS`) the kernel programs into MediaMTX so it gathers
    /// reachable candidates for remote viewing — **bring your own** STUN/TURN. A JSON array in MediaMTX
    /// `webrtcICEServers2` shape, e.g. `[{"url":"turn:turn.example.com:3478","username":"u","password":"p"}]`.
    /// When unset but a `rendezvous_url` is configured, the kernel fetches short-lived TURN credentials
    /// from the rendezvous (the Heldar-hosted option) and refreshes them; when neither is set, MediaMTX
    /// is left as-is (LAN-only).
    pub webrtc_ice_servers: Option<String>,
    // ---- Plugin registry / store (Phase C) ----
    /// Master switch for the plugin store's remote-registry fetching. When false, the store shows only
    /// the bundled open catalog + locally installed plugins (fully offline). The bundled catalog is
    /// always available regardless.
    pub registry_enabled: bool,
    /// Remote signed-catalog URLs to fetch (comma-separated). Default EMPTY — no phone-home; an
    /// operator (or the proprietary build) sets the official Straits-AI registry here to populate the
    /// proprietary/community shelves.
    pub registry_urls: Vec<String>,
    /// How often the background loop refreshes remote registries (seconds).
    pub registry_refresh_s: u64,
    /// Per-fetch timeout for a remote catalog (seconds).
    pub registry_fetch_timeout_s: u64,
    /// Operator-pinned extra trust anchors, `key_id:base64pubkey` comma-separated, added to the
    /// compile-time pinned keys (for private registries).
    pub registry_trusted_keys: Vec<(String, String)>,
    /// When true, surface a remote registry's entries even if its signature does not verify (badged
    /// unverified). Default false — fail closed.
    pub registry_allow_unverified: bool,
    /// When true, allow remote registry URLs that resolve to private/link-local addresses (default
    /// false; SSRF guard for the admin-configured fetch).
    pub registry_allow_private: bool,
    // ---- Embedded dashboard (single-binary SPA serving) ----
    /// Directory holding the built React dashboard (`apps/web/dist`), served as a static SPA
    /// fallback so the whole product is one binary at one URL. Resolved from `HELDAR_WEB_DIR`; when
    /// unset it falls back to `apps/web/dist` relative to the binary CWD. `None` when neither path
    /// exists — the server then runs API-only (no dashboard).
    pub web_dir: Option<PathBuf>,
    // ---- Email / SMTP notifier (the off-by-default `smtp` feature) ----
    /// SMTP relay host. Unset = email notifications disabled (the notifier parks).
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    /// Envelope/From address (e.g. `heldar@site.example`). Required to send.
    pub smtp_from: Option<String>,
    /// `starttls` (587, default) | `implicit` (465) | `none`.
    pub smtp_tls: String,
    /// Recipient addresses that receive matching-event emails.
    pub smtp_recipients: Vec<String>,
    /// Severity floor for emailed events: `info` | `warning` (default) | `critical`.
    pub smtp_min_severity: String,
    /// How often the notifier polls for new events to email (seconds).
    pub smtp_interval_s: u64,
}

/// mTLS material the edge presents to / uses to verify the control plane.
#[derive(Clone, Debug)]
pub struct CpTlsCfg {
    /// PEM path: this node's client certificate (CN must equal `site_id`).
    pub client_cert: PathBuf,
    /// PEM path: the private key for the client certificate.
    pub client_key: PathBuf,
    /// PEM path: the CA that signed the control plane's server certificate.
    pub server_ca: PathBuf,
}

/// Read the control-plane mTLS material from the environment. All-or-none: a partial set is a
/// misconfiguration, so warn and disable mTLS (the heartbeat will then fail loudly against an https
/// control plane, which is the visible signal to fix the config).
fn cp_tls_from_env() -> Option<CpTlsCfg> {
    match (
        var("HELDAR_CP_TLS_CLIENT_CERT"),
        var("HELDAR_CP_TLS_CLIENT_KEY"),
        var("HELDAR_CP_TLS_CA"),
    ) {
        (None, None, None) => None,
        (Some(client_cert), Some(client_key), Some(server_ca)) => Some(CpTlsCfg {
            client_cert: client_cert.into(),
            client_key: client_key.into(),
            server_ca: server_ca.into(),
        }),
        _ => {
            tracing::warn!(
                "control-plane mTLS needs all of HELDAR_CP_TLS_CLIENT_CERT, HELDAR_CP_TLS_CLIENT_KEY, HELDAR_CP_TLS_CA; ignoring partial config"
            );
            None
        }
    }
}

impl Config {
    /// Whether per-account brute-force lockout is active (both knobs must be > 0).
    pub fn login_lockout_enabled(&self) -> bool {
        self.login_max_failures > 0 && self.login_lockout_min > 0
    }

    /// Fail-loud guardrails for an internet-exposed deployment. A configured remote rendezvous
    /// (`HELDAR_REMOTE_RENDEZVOUS_URL`) means the box is reachable from the internet, so an unsafe auth
    /// posture is a misconfiguration: with auth disabled we **refuse to boot** (the open API must never
    /// be exposed); otherwise we WARN — or refuse, under `HELDAR_STRICT_PROD=true` — on a non-`Secure`
    /// cookie, no idle timeout, an over-long session TTL, a localhost CORS allowlist, or plaintext
    /// camera credentials. A LAN/overlay appliance (no rendezvous) keeps its intentional defaults.
    pub fn enforce_production_guardrails(&self) -> anyhow::Result<()> {
        if self.rendezvous_url.is_none() {
            return Ok(());
        }
        if !self.auth_enabled {
            anyhow::bail!(
                "remote access is configured (HELDAR_REMOTE_RENDEZVOUS_URL) but HELDAR_AUTH_ENABLED=false \
                 — refusing to expose the open API to the internet. Set HELDAR_AUTH_ENABLED=true."
            );
        }
        let mut warnings: Vec<String> = Vec::new();
        if !self.auth_cookie_secure {
            warnings.push(
                "HELDAR_AUTH_COOKIE_SECURE=false — set true so the session cookie requires HTTPS"
                    .into(),
            );
        }
        if self.session_idle_timeout_minutes == 0 {
            warnings.push(
                "HELDAR_SESSION_IDLE_TIMEOUT_MIN=0 — set e.g. 30 to expire idle remote sessions"
                    .into(),
            );
        }
        if self.session_ttl_hours > 12 {
            warnings.push(format!(
                "HELDAR_SESSION_TTL_HOURS={} is long for remote access — consider 4 or less",
                self.session_ttl_hours
            ));
        }
        if self
            .cors_origins
            .iter()
            .any(|o| o.contains("localhost") || o.contains("127.0.0.1"))
        {
            warnings.push(
                "HELDAR_CORS_ORIGINS still allows localhost — lock it to the dashboard origin"
                    .into(),
            );
        }
        if self.secret_key_b64.is_none() {
            warnings.push(
                "HELDAR_SECRET_KEY is unset — camera credentials are stored in plaintext at rest"
                    .into(),
            );
        }
        if warnings.is_empty() {
            return Ok(());
        }
        for w in &warnings {
            tracing::warn!("production guardrail: {w}");
        }
        if self.strict_prod {
            anyhow::bail!(
                "HELDAR_STRICT_PROD=true and {} production guardrail(s) failed (see warnings above)",
                warnings.len()
            );
        }
        Ok(())
    }

    pub fn from_env() -> Self {
        let data_dir = PathBuf::from(var_or("HELDAR_DATA_DIR", "./data"));
        let recordings_dir = var("HELDAR_RECORDINGS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("recordings"));
        let clips_dir = var("HELDAR_CLIPS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("clips"));
        let snapshots_dir = var("HELDAR_SNAPSHOTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("snapshots"));
        let frames_dir = var("HELDAR_FRAMES_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("frames"));
        let playback_dir = var("HELDAR_PLAYBACK_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("playback"));
        let archive_dir = var("HELDAR_ARCHIVE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("archives"));

        let cors_origins = var_or("HELDAR_CORS_ORIGINS", "http://localhost:5173")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // Embedded dashboard: explicit HELDAR_WEB_DIR wins; otherwise try `apps/web/dist` relative
        // to the binary CWD. Only `Some` when the directory actually exists (else API-only).
        let web_dir = var("HELDAR_WEB_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("apps/web/dist"));
        let web_dir = if web_dir.is_dir() {
            Some(web_dir)
        } else {
            None
        };

        let max_recordings_gb: f64 = parse_or("HELDAR_MAX_RECORDINGS_GB", 20.0);
        let min_free_disk_gb: f64 = parse_or("HELDAR_MIN_FREE_DISK_GB", 5.0);
        let default_camera_quota_gb: f64 = parse_or("HELDAR_DEFAULT_CAMERA_QUOTA_GB", 0.0);

        Config {
            database_url: var_or("HELDAR_DATABASE_URL", "sqlite://./data/heldar.db"),
            data_dir,
            recordings_dir,
            clips_dir,
            snapshots_dir,
            frames_dir,
            playback_dir,
            ffmpeg_bin: var_or("HELDAR_FFMPEG_BIN", "ffmpeg"),
            ffprobe_bin: var_or("HELDAR_FFPROBE_BIN", "ffprobe"),
            mediamtx_api_url: var_or("HELDAR_MEDIAMTX_API_URL", "http://127.0.0.1:9997"),
            mediamtx_hls_base: var_or("HELDAR_MEDIAMTX_HLS_BASE", "http://127.0.0.1:8888"),
            mediamtx_rtsp_base: var_or("HELDAR_MEDIAMTX_RTSP_BASE", "rtsp://127.0.0.1:8554"),
            mediamtx_webrtc_base: var_or("HELDAR_MEDIAMTX_WEBRTC_BASE", "http://127.0.0.1:8889"),
            db_max_connections: parse_or::<u32>("HELDAR_DB_MAX_CONNECTIONS", 16).clamp(2, 256),
            recorder_enabled: parse_bool("HELDAR_RECORDER_ENABLED", true),
            mirror_recordings_dir: var("HELDAR_MIRROR_RECORDINGS_DIR").map(PathBuf::from),
            anr_enabled: parse_bool("HELDAR_ANR_ENABLED", false),
            anr_interval_s: parse_or("HELDAR_ANR_INTERVAL_S", 300),
            anr_max_gap_hours: parse_or("HELDAR_ANR_MAX_GAP_HOURS", 24),
            anr_max_attempts: parse_or("HELDAR_ANR_MAX_ATTEMPTS", 3),
            default_segment_seconds: parse_or("HELDAR_DEFAULT_SEGMENT_SECONDS", 60),
            default_retention_hours: parse_or("HELDAR_DEFAULT_RETENTION_HOURS", 24),
            default_camera_quota_bytes: (default_camera_quota_gb * 1024.0 * 1024.0 * 1024.0) as u64,
            default_record_audio: parse_bool("HELDAR_DEFAULT_RECORD_AUDIO", false),
            default_pre_roll_seconds: parse_or("HELDAR_DEFAULT_PRE_ROLL_SECONDS", 10),
            default_post_roll_seconds: parse_or("HELDAR_DEFAULT_POST_ROLL_SECONDS", 30),
            indexer_interval_s: parse_or("HELDAR_INDEXER_INTERVAL_S", 10),
            health_interval_s: parse_or("HELDAR_HEALTH_INTERVAL_S", 15),
            retention_interval_s: parse_or("HELDAR_RETENTION_INTERVAL_S", 300),
            api_host: var_or("HELDAR_API_HOST", "0.0.0.0"),
            api_port: parse_or("HELDAR_API_PORT", 8000),
            cors_origins,
            max_recordings_bytes: (max_recordings_gb * 1024.0 * 1024.0 * 1024.0) as u64,
            min_free_disk_bytes: (min_free_disk_gb * 1024.0 * 1024.0 * 1024.0) as u64,
            notifier_interval_s: parse_or("HELDAR_NOTIFIER_INTERVAL_S", 15),
            ai_enabled: parse_bool("HELDAR_AI_ENABLED", true),
            ai_max_total_fps: parse_or("HELDAR_AI_MAX_TOTAL_FPS", 40.0),
            default_ai_fps: parse_or("HELDAR_DEFAULT_AI_FPS", 5.0),
            default_ai_width: parse_or("HELDAR_DEFAULT_AI_WIDTH", 1280),
            detection_retention_hours: parse_or("HELDAR_DETECTION_RETENTION_HOURS", 168),
            snapshot_scheduler_enabled: parse_bool("HELDAR_SNAPSHOT_SCHEDULER_ENABLED", true),
            snapshot_scheduler_interval_s: parse_or("HELDAR_SNAPSHOT_SCHEDULER_INTERVAL_S", 60),
            snapshot_retention_hours: parse_or("HELDAR_SNAPSHOT_RETENTION_HOURS", 168),
            schedule_check_interval_s: parse_or("HELDAR_SCHEDULE_CHECK_INTERVAL_S", 30),
            playback_session_ttl_minutes: parse_or("HELDAR_PLAYBACK_SESSION_TTL_MINUTES", 60),
            max_playback_seconds: parse_or("HELDAR_MAX_PLAYBACK_SECONDS", 7200.0),
            auth_enabled: parse_bool("HELDAR_AUTH_ENABLED", false),
            session_ttl_hours: parse_or("HELDAR_SESSION_TTL_HOURS", 12),
            session_idle_timeout_minutes: parse_or("HELDAR_SESSION_IDLE_TIMEOUT_MIN", 0),
            auth_cookie_secure: parse_bool("HELDAR_AUTH_COOKIE_SECURE", false),
            login_max_failures: parse_or("HELDAR_LOGIN_MAX_FAILURES", 5),
            login_lockout_min: parse_or("HELDAR_LOGIN_LOCKOUT_MIN", 15),
            secret_key_b64: var("HELDAR_SECRET_KEY"),
            strict_prod: parse_bool("HELDAR_STRICT_PROD", false),
            bootstrap_admin_user: var("HELDAR_BOOTSTRAP_ADMIN_USER"),
            bootstrap_admin_password: var("HELDAR_BOOTSTRAP_ADMIN_PASSWORD"),
            audit_retention_days: parse_or("HELDAR_AUDIT_RETENTION_DAYS", 365),
            overlay_enabled: parse_bool("HELDAR_OVERLAY_ENABLED", false),
            overlay_kind: var_or("HELDAR_OVERLAY_KIND", "none"),
            overlay_iface: var("HELDAR_OVERLAY_IFACE"),
            rclone_bin: var_or("HELDAR_RCLONE_BIN", "rclone"),
            backup_enabled: parse_bool("HELDAR_BACKUP_ENABLED", true),
            backup_scheduler_interval_s: parse_or("HELDAR_BACKUP_SCHEDULER_INTERVAL_S", 60),
            backup_job_timeout_s: parse_or("HELDAR_BACKUP_JOB_TIMEOUT_S", 3600),
            backup_max_concurrent_jobs: parse_or::<usize>("HELDAR_BACKUP_MAX_CONCURRENT_JOBS", 2)
                .max(1),
            archive_dir,
            archive_max_bytes: parse_or("HELDAR_ARCHIVE_MAX_BYTES", 10_737_418_240u64),
            archive_retention_hours: parse_or("HELDAR_ARCHIVE_RETENTION_HOURS", 48),
            onvif_discovery_timeout_ms: parse_or("HELDAR_ONVIF_DISCOVERY_TIMEOUT_MS", 2000),
            onvif_request_timeout_ms: parse_or("HELDAR_ONVIF_REQUEST_TIMEOUT_MS", 5000),
            isapi_request_timeout_ms: parse_or("HELDAR_ISAPI_REQUEST_TIMEOUT_MS", 8000),
            smart_check_enabled: parse_bool("HELDAR_SMART_CHECK_ENABLED", false),
            smart_devices: var("HELDAR_SMART_DEVICES")
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            mdstat_check_enabled: parse_bool("HELDAR_MDSTAT_CHECK_ENABLED", false),
            smart_check_interval_s: parse_or("HELDAR_SMART_CHECK_INTERVAL_S", 300),
            readyz_min_recording_percent: parse_or("HELDAR_READYZ_MIN_RECORDING_PERCENT", 0.0),
            live_transcode_engine: var_or("HELDAR_LIVE_TRANSCODE_ENGINE", "software"),
            vaapi_device: var_or("HELDAR_VAAPI_DEVICE", "/dev/dri/renderD128"),
            site_id: var("HELDAR_SITE_ID"),
            cp_url: var("HELDAR_CP_URL"),
            public_base_url: var("HELDAR_PUBLIC_BASE_URL"),
            cp_token: var_or("HELDAR_CP_TOKEN", ""),
            cp_register_interval_s: parse_or("HELDAR_CP_REGISTER_INTERVAL_S", 300),
            cp_tls: cp_tls_from_env(),
            rendezvous_url: var("HELDAR_REMOTE_RENDEZVOUS_URL"),
            webrtc_ice_servers: var("HELDAR_WEBRTC_ICE_SERVERS"),
            registry_enabled: parse_bool("HELDAR_REGISTRY_ENABLED", true),
            registry_urls: var_or("HELDAR_REGISTRY_URLS", "")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            registry_refresh_s: parse_or("HELDAR_REGISTRY_REFRESH_S", 900),
            registry_fetch_timeout_s: parse_or("HELDAR_REGISTRY_FETCH_TIMEOUT_S", 10),
            registry_trusted_keys: var_or("HELDAR_REGISTRY_TRUSTED_KEYS", "")
                .split(',')
                .filter_map(|s| {
                    let s = s.trim();
                    s.split_once(':')
                        .map(|(id, key)| (id.trim().to_string(), key.trim().to_string()))
                        .filter(|(id, key)| !id.is_empty() && !key.is_empty())
                })
                .collect(),
            registry_allow_unverified: parse_bool("HELDAR_REGISTRY_ALLOW_UNVERIFIED", false),
            registry_allow_private: parse_bool("HELDAR_REGISTRY_ALLOW_PRIVATE", false),
            web_dir,
            smtp_host: var("HELDAR_SMTP_HOST"),
            smtp_port: parse_or("HELDAR_SMTP_PORT", 587u16),
            smtp_username: var("HELDAR_SMTP_USERNAME"),
            smtp_password: var("HELDAR_SMTP_PASSWORD"),
            smtp_from: var("HELDAR_SMTP_FROM"),
            smtp_tls: var_or("HELDAR_SMTP_TLS", "starttls"),
            smtp_recipients: var_or("HELDAR_SMTP_RECIPIENTS", "")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            smtp_min_severity: var_or("HELDAR_SMTP_MIN_SEVERITY", "warning"),
            smtp_interval_s: parse_or("HELDAR_SMTP_INTERVAL_S", 30),
        }
    }

    /// Directory where a camera's segments are stored.
    pub fn camera_recordings_dir(&self, camera_id: &str) -> PathBuf {
        self.recordings_dir.join(camera_id)
    }

    /// Directory where a camera's sampled AI frames are written.
    pub fn camera_frames_dir(&self, camera_id: &str) -> PathBuf {
        self.frames_dir.join(camera_id)
    }
}
