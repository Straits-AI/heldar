use std::path::PathBuf;

use crate::env::{parse_bool, parse_or, var, var_or};

/// Runtime configuration, loaded from environment (see `.env.example`).
#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub data_dir: PathBuf,
    pub recordings_dir: PathBuf,
    pub clips_dir: PathBuf,
    pub snapshots_dir: PathBuf,
    pub frames_dir: PathBuf,
    /// Directory where segment-spanning HLS playback sessions are generated (one subdir per session).
    pub playback_dir: PathBuf,
    /// Signed evidence bundles (#118). Separate from `clips_dir` because a bundle is a different
    /// artifact with a different retention and a different meaning: a clip is convenience, a bundle
    /// is a chain-of-custody claim.
    pub evidence_dir: PathBuf,
    pub ffmpeg_bin: String,
    pub ffprobe_bin: String,
    pub mediamtx_api_url: String,
    pub mediamtx_hls_base: String,
    pub mediamtx_rtsp_base: String,
    pub mediamtx_webrtc_base: String,
    /// Emit SAME-ORIGIN media URLs for live view (`HELDAR_MEDIA_SAME_ORIGIN`).
    ///
    /// MediaMTX serves HLS/WebRTC on its own plaintext ports (8888/8889). The default behaviour hands
    /// the browser an absolute `http://<host>:8888/…` URL, which works on a plain-HTTP LAN dashboard
    /// but is BLOCKED AS MIXED CONTENT the moment the dashboard is served over HTTPS — so live view
    /// silently dies behind a TLS terminator. With this set, live URLs become origin-relative
    /// (`/live/hls/…`, `/live/whep/…`) and the reverse proxy in front is responsible for routing
    /// those prefixes to MediaMTX. `deploy/compose.tls.yml` sets it; see `deploy/Caddyfile`.
    pub media_same_origin: bool,
    /// TTL (seconds) of a minted live-view/playback read token (`HELDAR_LIVEVIEW_TOKEN_TTL_SECS`).
    /// Must comfortably outlast a viewing session's reconnects; the dashboard re-fetches `/liveview`
    /// (re-minting) whenever it reopens a stream. Only enforced when kernel auth is enabled.
    pub live_token_ttl_secs: i64,
    /// How often to re-check the credentials behind LIVE MediaMTX sessions and kick the withdrawn.
    ///
    /// This is the revocation latency for transports that never re-present their token — WebRTC
    /// authorizes once at negotiation, RTSP readers once at connect. HLS is unaffected: it
    /// re-presents per segment, so the auth callback already stops it. Trades that latency against
    /// MediaMTX API chatter; 0 disables the reaper entirely.
    pub live_reaper_interval_s: u64,
    /// Max concurrent interactive media jobs — playback session builds, clip exports, snapshots
    /// (`HELDAR_MEDIA_JOB_CONCURRENCY`). Each forks ffmpeg/ffprobe and does heavy disk I/O; unbounded,
    /// they starve the RECORDER, which is the one process that must never miss. Clamped to >= 1.
    pub media_job_concurrency: usize,
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
    /// Hard cap on the metadata DB file size (`heldar.db`). When the DB exceeds this, the retention
    /// sweep sheds the oldest `detections` (events/audit are protected) and incrementally vacuums.
    /// A generous backstop above normal time-retention usage; non-positive disables the cap.
    pub max_db_bytes: u64,
    /// When true (default), a pre-existing DB is converted to auto_vacuum=INCREMENTAL by a
    /// one-time BACKGROUND task after boot. Set false to skip it (run `convert-autovacuum` manually).
    pub db_autovacuum_convert: bool,
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
    /// Hard ceiling (hours) on a session's total life when SLIDING expiry is enabled. 0 (default)
    /// keeps expiry strictly absolute: `expires_at` is fixed at login and never moves, so an operator
    /// working continuously is still logged out after `session_ttl_hours`.
    ///
    /// Set > 0 to let an in-use session slide — each use pushes `expires_at` to
    /// `now + session_ttl_hours`, never past `created_at + this`. That gives "stay signed in while I
    /// am working, log me out if I walk away", which is what a refresh token would be used for in a
    /// stateless design. Sessions here are opaque and DB-backed (revocable by deleting one row), so no
    /// second long-lived credential is warranted — sliding the one session is the whole mechanism.
    ///
    /// The cap is not optional: without it, sliding makes a stolen cookie effectively immortal, which
    /// is precisely what the absolute TTL exists to prevent. Opt-in by design, so upgrading never
    /// silently lengthens a session's life.
    pub session_max_lifetime_hours: i64,
    /// Add `Secure` to the session cookie (require HTTPS). Default false for HTTP LAN/overlay
    /// appliances; set true when the deployment is served over TLS.
    pub auth_cookie_secure: bool,
    /// Capability enforcement for credentials with NO explicit grant (`HELDAR_MACHINE_AUTH`).
    ///
    /// `off` / `warn` (DEFAULT) expand a legacy key from its role to exactly today's reach, so nothing
    /// deployed changes behaviour; `warn` additionally logs, once per key per hour, every capability
    /// that `enforce` would take away. `enforce` narrows the `integration` role to what a real AI worker
    /// calls. Promoted to `enforce` automatically when `HELDAR_DEPLOYMENT_MODE=production*`.
    pub machine_auth: EnforcementTier,
    /// Frame-ticket requirement on the AI ingest path (`HELDAR_INGEST_PROVENANCE`). `warn` (DEFAULT)
    /// accepts a ticketless batch exactly as today; `enforce` requires a server-issued frame ticket.
    ///
    /// **NOT auto-promoted by `HELDAR_DEPLOYMENT_MODE`**, unlike [`Self::machine_auth`] — this is the
    /// one tier a deployment label must never move, because it is a CLIENT protocol requirement and
    /// promoting it silently stops all AI ingest on a box that otherwise looks healthy. Only an
    /// explicit `HELDAR_INGEST_PROVENANCE=enforce` turns it on; see the rationale at the
    /// `tier_from_env` call site in [`Config::from_env`], and docs/AI-WORKERS.md §5.0.
    pub ingest_provenance: EnforcementTier,
    /// TTL (seconds) of a minted per-frame ingest ticket (`HELDAR_FRAME_TICKET_TTL_SECS`). Long enough
    /// to survive a slow inference pass, short enough that a leaked ticket is worthless. Consumed by
    /// Stage B.
    pub frame_ticket_ttl_secs: i64,
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
    /// Operator's explicit declaration that this box is reachable from outside the trusted LAN
    /// (`HELDAR_INTERNET_EXPOSED=true`). The automatic detection ([`Config::internet_exposed`]) only
    /// knows about the opt-in remote paths (rendezvous / overlay / control-plane) — it CANNOT see a
    /// reverse proxy, a port-forward, or a public-cloud bind. Operators using those must set this so
    /// the auth-off boot refusal + hardening guardrails still fire. Default false (LAN appliance).
    pub exposed_declared: bool,
    /// Optional deployment-mode ladder (`HELDAR_DEPLOYMENT_MODE`). Unset/empty preserves today's
    /// permissive defaults exactly. `production` (and its `production-lan` / `production-remote`
    /// variants) TIGHTENS behavior: auth-off on a non-loopback bind becomes a hard boot refusal
    /// instead of a warning. Documented in docs/PRODUCTION.md. Stored lowercased; never a breaking
    /// default.
    pub deployment_mode: String,
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
    /// Cap on the cumulative size of `archive_dir` (all retained exports). A new export is rejected
    /// (HTTP 400) when it would push the directory over this, so accumulated exports can't fill the
    /// recordings filesystem and drive the retention sweeper into evicting live recordings.
    pub archive_dir_max_bytes: u64,
    /// How long archive exports + finished backup-job rows are kept before retention prunes them.
    pub archive_retention_hours: i64,
    // ---- ONVIF (kernel platform feature; Profile S MVP) ----
    /// How long the WS-Discovery probe listens for ProbeMatch replies (milliseconds).
    pub onvif_discovery_timeout_ms: u64,
    /// Per-request timeout for an ONVIF SOAP call (GetDeviceInformation, PTZ, etc.) in milliseconds.
    pub onvif_request_timeout_ms: u64,
    /// Per-request timeout for a HikVision ISAPI camera-config call (HTTP Digest) in milliseconds.
    pub isapi_request_timeout_ms: u64,
    /// Poll cadence of the camera-native ANPR plate poller (per enabled camera) in milliseconds.
    pub native_anpr_poll_ms: u64,
    /// Quiet gap (seconds) after which a new on-camera event burst logs a fresh event
    /// (`services::camera_events` rising-edge debounce).
    pub camera_events_rearm_secs: u64,
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
    /// Reap an on-demand live publisher after this many seconds with no viewers and no demand.
    pub live_idle_close_secs: u64,
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

/// A three-position enforcement switch, the shape already proven by the deployment-mode ladder: ship
/// today's behaviour by default, give the operator a tier that TELLS them what tightening would do, and
/// only then bite.
///
/// Exactly two of these ship (`HELDAR_MACHINE_AUTH`, `HELDAR_INGEST_PROVENANCE`) and both resolved
/// postures are printed in one boxed boot banner — multiple interacting silent switches are themselves
/// a misconfiguration hazard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EnforcementTier {
    /// No enforcement and no logging.
    Off,
    /// No enforcement, but log what `Enforce` would have denied. The default.
    #[default]
    Warn,
    /// Enforce.
    Enforce,
}

impl EnforcementTier {
    pub fn as_str(self) -> &'static str {
        match self {
            EnforcementTier::Off => "off",
            EnforcementTier::Warn => "warn",
            EnforcementTier::Enforce => "enforce",
        }
    }
    pub fn parse(s: &str) -> Option<EnforcementTier> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "0" => EnforcementTier::Off,
            "warn" | "warning" => EnforcementTier::Warn,
            "enforce" | "true" | "1" => EnforcementTier::Enforce,
            _ => return None,
        })
    }
}

/// Read an enforcement tier from the environment: unset → `warn`; unrecognized → `warn` with a loud
/// warning (never a silent tightening AND never a silent loosening); `production*` promotes to
/// `enforce` unless the operator named a tier explicitly.
fn tier_from_env(key: &str, mode_is_production: bool) -> EnforcementTier {
    match var(key) {
        Some(raw) if !raw.trim().is_empty() => match EnforcementTier::parse(&raw) {
            Some(t) => t,
            None => {
                tracing::warn!(
                    "{key}={raw} is not one of off|warn|enforce; falling back to `warn`"
                );
                EnforcementTier::Warn
            }
        },
        _ if mode_is_production => EnforcementTier::Enforce,
        _ => EnforcementTier::Warn,
    }
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
        // NOT `secret()`. This holds a PATH, not a secret — `_FILE` resolution would substitute the
        // key's CONTENTS where a filename is expected, and `fleet_register` interpolates that
        // filename into an error it logs at ERROR level. Wiring it to the chain would have made
        // the branch whose purpose is keeping secrets out of logs print a PEM private key.
        // A systemd credential already IS a path ($CREDENTIALS_DIRECTORY/NAME), which `var()`
        // handles unchanged.
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

/// A deployment SECRET, resolved through the secret-source chain (#126) rather than read straight
/// from the environment.
///
/// `NAME` still wins, so nothing moves on upgrade; `NAME_FILE` and a systemd credential are the
/// hardened alternatives. A named-but-unusable source is fatal at boot rather than a silent
/// fall-through to "no secret" — an operator who pointed at a file asked for it to be used.
fn secret(name: &str) -> Option<String> {
    match crate::services::secret_source::resolve_and_report(name) {
        Ok(r) => r.map(|r| r.expose().to_string()),
        Err(e) => {
            // NOT a panic. `Config::from_env()` is the repo's test-config idiom, called from ~60
            // helpers, so panicking here meant one stale `_FILE` variable in a developer's shell
            // detonated 144 tests. The refusal belongs at boot, where it is actionable: the error is
            // recorded and `heldar_server::run` refuses to start. Fail-closed either way, without
            // the blast radius.
            SECRET_ERRORS
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(format!("{e:#}"));
            None
        }
    }
}

/// Secret sources that were NAMED but could not be read, collected during `from_env`.
///
/// Read by [`Config::secret_source_errors`] so the server can refuse to boot. An operator who set
/// `HELDAR_SECRET_KEY_FILE` asked for encryption at rest; starting anyway would store every camera
/// credential in plaintext while the deployment believed they were sealed.
static SECRET_ERRORS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Hand-written so a secret cannot reach a log through `{:?}`.
///
/// `#[derive(Debug)]` printed `secret_key_b64: Some("yQw+C67...")` in full, and `?cfg` is the
/// idiomatic `tracing` shape used throughout this codebase — so the careful redaction in
/// `secret_source::Resolved` protected a value for the three statements before it was copied into a
/// struct that prints it. The long-lived copy is the one that matters: it lives in an `Arc<Config>`
/// for the process lifetime and is handed to every service.
///
/// Secrets report whether they are SET, which is the operationally useful half and discloses
/// nothing — not even a length, since a length is a meaningful hint about a key.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("data_dir", &self.data_dir)
            .field("deployment_mode", &self.deployment_mode)
            .field("auth_enabled", &self.auth_enabled)
            .field("strict_prod", &self.strict_prod)
            .field("secret_key_b64", &redacted(&self.secret_key_b64))
            .field(
                "bootstrap_admin_password",
                &redacted(&self.bootstrap_admin_password),
            )
            .field("smtp_password", &redacted(&self.smtp_password))
            .finish_non_exhaustive()
    }
}

/// `"<set>"` or `"<unset>"` — never the value, never its length.
fn redacted(v: &Option<String>) -> &'static str {
    if v.is_some() {
        "<set>"
    } else {
        "<unset>"
    }
}

impl Config {
    /// Secret sources that were NAMED but could not be read (#126).
    ///
    /// Non-empty means an operator asked for a secret to come from somewhere and it did not. The
    /// server refuses to boot on this: continuing would store camera credentials in plaintext while
    /// the deployment believed they were sealed, and the failure would stay invisible until someone
    /// read the database.
    pub fn secret_source_errors() -> Vec<String> {
        SECRET_ERRORS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Forget any recorded secret-source errors. For tests that deliberately misconfigure one.
    #[doc(hidden)]
    pub fn clear_secret_source_errors() {
        SECRET_ERRORS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    /// Whether per-account brute-force lockout is active (both knobs must be > 0).
    pub fn login_lockout_enabled(&self) -> bool {
        self.login_max_failures > 0 && self.login_lockout_min > 0
    }

    /// Whether the box is reachable from outside the trusted LAN by ANY configured path — the WebRTC
    /// rendezvous (`HELDAR_REMOTE_RENDEZVOUS_URL`), an overlay network (`HELDAR_OVERLAY_ENABLED`:
    /// Tailscale/NetBird/WireGuard), or self-registration to the control plane (`HELDAR_CP_URL` +
    /// `HELDAR_PUBLIC_BASE_URL`). The production guardrails key on this, not on the rendezvous alone,
    /// so an overlay/control-plane deployment can't slip past the auth-off boot refusal with LAN
    /// defaults.
    pub fn internet_exposed(&self) -> bool {
        is_internet_exposed(
            self.rendezvous_url.is_some(),
            self.overlay_enabled,
            self.cp_url.is_some() && self.public_base_url.is_some(),
            self.exposed_declared,
        )
    }

    /// Fail-loud guardrails for an internet-exposed deployment. When the box is reachable from outside
    /// the LAN (see [`Config::internet_exposed`]) an unsafe auth posture is a misconfiguration: with
    /// auth disabled we **refuse to boot** (the open API must never be exposed); otherwise we WARN —
    /// or refuse, under `HELDAR_STRICT_PROD=true` — on a non-`Secure` cookie, no idle timeout, an
    /// over-long session TTL, a localhost or wildcard CORS allowlist, plaintext camera credentials, or
    /// an empty dial-out bearer. A LAN-only appliance keeps its intentional defaults.
    pub fn enforce_production_guardrails(&self) -> anyhow::Result<()> {
        let state = GuardrailState {
            exposed: self.internet_exposed(),
            rendezvous: self.rendezvous_url.is_some(),
            auth_enabled: self.auth_enabled,
            auth_cookie_secure: self.auth_cookie_secure,
            session_idle_timeout_minutes: self.session_idle_timeout_minutes,
            session_ttl_hours: self.session_ttl_hours,
            cors_has_localhost: self
                .cors_origins
                .iter()
                .any(|o| o.contains("localhost") || o.contains("127.0.0.1")),
            cors_has_wildcard: self.cors_origins.iter().any(|o| o.trim() == "*"),
            secret_key_set: self.secret_key_b64.is_some(),
            cp_token_empty: self.cp_token.trim().is_empty(),
            strict_prod: self.strict_prod,
        };
        match evaluate_production_guardrails(&state) {
            GuardrailOutcome::Refuse(msg) => anyhow::bail!(msg),
            GuardrailOutcome::Pass(warnings) => {
                for w in &warnings {
                    tracing::warn!("production guardrail: {w}");
                }
            }
            GuardrailOutcome::StrictRefuse(warnings) => {
                for w in &warnings {
                    tracing::warn!("production guardrail: {w}");
                }
                anyhow::bail!(
                    "HELDAR_STRICT_PROD=true and {} production guardrail(s) failed (see warnings above)",
                    warnings.len()
                )
            }
        }
        // Separate, LAN-only posture check: auth off on a non-loopback bind trusts the whole LAN as
        // admin. Runs alongside (not inside) the exposure guardrails above so it stays independently
        // testable. When the box is internet-exposed with auth off, the exposure refusal above already
        // bailed; this only bites the plain LAN-bind case the exposure check can't see.
        self.enforce_lan_trust_guardrail()
    }

    /// Whether `HELDAR_DEPLOYMENT_MODE` selects a production tier. Any `production*` value
    /// (`production`, `production-lan`, `production-remote`) counts; unset/empty/`dev`/`commissioning`
    /// do not — so the default behavior is unchanged.
    pub fn deployment_mode_is_production(&self) -> bool {
        self.deployment_mode.starts_with("production")
    }

    /// LAN-trust guardrail. With auth OFF and the API bound to a non-loopback address, every device on
    /// the LAN is trusted as full admin. Default posture: emit a loud, boxed startup WARNING and boot
    /// (so `docker compose up -d` commissioning still works). If `HELDAR_DEPLOYMENT_MODE=production*`,
    /// this becomes a hard boot refusal instead. Kept as its own method (and pure decision function) so
    /// it is separately testable from [`evaluate_production_guardrails`].
    fn enforce_lan_trust_guardrail(&self) -> anyhow::Result<()> {
        match evaluate_lan_trust(
            self.auth_enabled,
            bind_is_loopback(&self.api_host),
            self.deployment_mode_is_production(),
        ) {
            LanTrustOutcome::Silent => Ok(()),
            LanTrustOutcome::Warn => {
                warn_lan_trusted_as_admin(&self.api_host);
                Ok(())
            }
            LanTrustOutcome::Refuse => anyhow::bail!(
                "HELDAR_DEPLOYMENT_MODE={} requires authentication: the API is bound to {} (a \
                 non-loopback address) with HELDAR_AUTH_ENABLED=false, which trusts every device on \
                 the LAN as full admin. Set HELDAR_AUTH_ENABLED=true, or bind \
                 HELDAR_API_HOST=127.0.0.1 for local dev.",
                self.deployment_mode,
                self.api_host
            ),
        }
    }

    /// Emit the single boxed boot banner reporting the resolved machine-credential posture.
    ///
    /// One banner, both switches, and the list of credentials still riding on a role expansion — an
    /// operator should never have to infer the enforcement posture from which env vars they remember
    /// setting. `legacy_keys` is `(id, name)` for every api key with `capabilities IS NULL`; the caller
    /// supplies it because config owns no database handle.
    pub fn log_machine_auth_banner(&self, legacy_keys: &[(String, String)]) {
        let legacy = if legacy_keys.is_empty() {
            "none — every credential carries an explicit grant".to_string()
        } else {
            let named: Vec<String> = legacy_keys
                .iter()
                .take(20)
                .map(|(id, name)| format!("{name} ({id})"))
                .collect();
            let more = legacy_keys.len().saturating_sub(named.len());
            let mut s = named.join(", ");
            if more > 0 {
                let _ = std::fmt::Write::write_fmt(&mut s, format_args!(" … +{more} more"));
            }
            s
        };
        let effect = match self.machine_auth {
            EnforcementTier::Off => "legacy keys keep today's full reach; nothing is logged",
            EnforcementTier::Warn => {
                "legacy keys keep today's full reach; each is logged once an hour with what \
                 `enforce` would deny"
            }
            EnforcementTier::Enforce => {
                "legacy `integration` keys are narrowed to the AI-worker capability set"
            }
        };
        tracing::info!(
            target: "heldar::security",
            machine_auth = %self.machine_auth.as_str(),
            ingest_provenance = %self.ingest_provenance.as_str(),
            "machine-credential posture resolved\n\
             ┌──────────────────────────────────────────────────────────────────────────────┐\n\
             │  MACHINE CREDENTIALS — resolved enforcement posture                           │\n\
             └──────────────────────────────────────────────────────────────────────────────┘\n\
             HELDAR_MACHINE_AUTH      = {machine_auth}\n\
                 {effect}\n\
             HELDAR_INGEST_PROVENANCE = {ingest_provenance}\n\
                 {ticket_state}\n\
             deployment mode          = {mode}\n\
             keys with no explicit capability grant: {legacy}",
            machine_auth = self.machine_auth.as_str(),
            ingest_provenance = self.ingest_provenance.as_str(),
            effect = effect,
            // State the NON-enforced posture as plainly as the enforced one. An operator who set
            // HELDAR_DEPLOYMENT_MODE=production reasonably assumes both tiers moved; only
            // `machine_auth` did, and reading that from the absence of a line is exactly how a box
            // ends up accepting ticketless ingest while its operator believes otherwise.
            ticket_state = match self.ingest_provenance {
                EnforcementTier::Enforce =>
                    "ENFORCED — a ticketless ingest batch is rejected (401 frame_ticket_required)",
                EnforcementTier::Warn =>
                    "NOT ENFORCED — a ticketless ingest batch is ACCEPTED. Each such credential is \
                     logged once an hour with what `enforce` would deny. This tier is never \
                     promoted by HELDAR_DEPLOYMENT_MODE: set HELDAR_INGEST_PROVENANCE=enforce \
                     explicitly, once that hourly log is empty.",
                EnforcementTier::Off =>
                    "NOT ENFORCED and NOT REPORTED — a ticketless ingest batch is ACCEPTED silently. \
                     This tier is never promoted by HELDAR_DEPLOYMENT_MODE; `warn` at minimum is \
                     recommended so you can see which credentials are ticketless.",
            },
            mode = if self.deployment_mode.is_empty() {
                "(unset)"
            } else {
                &self.deployment_mode
            },
            legacy = legacy,
        );
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
        let evidence_dir = var("HELDAR_EVIDENCE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("evidence"));

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

        // Resolved before the struct literal: the two enforcement tiers default to `enforce` when a
        // production deployment mode is selected, so they need it in hand.
        let deployment_mode = var_or("HELDAR_DEPLOYMENT_MODE", "")
            .trim()
            .to_ascii_lowercase();
        let mode_is_production = deployment_mode.starts_with("production");

        let max_recordings_gb: f64 = parse_or("HELDAR_MAX_RECORDINGS_GB", 20.0);
        let min_free_disk_gb: f64 = parse_or("HELDAR_MIN_FREE_DISK_GB", 5.0);
        let max_db_gb: f64 = parse_or("HELDAR_MAX_DB_GB", 4.0);
        let default_camera_quota_gb: f64 = parse_or("HELDAR_DEFAULT_CAMERA_QUOTA_GB", 0.0);

        Config {
            database_url: var_or("HELDAR_DATABASE_URL", "sqlite://./data/heldar.db"),
            data_dir,
            recordings_dir,
            clips_dir,
            evidence_dir,
            snapshots_dir,
            frames_dir,
            playback_dir,
            ffmpeg_bin: var_or("HELDAR_FFMPEG_BIN", "ffmpeg"),
            ffprobe_bin: var_or("HELDAR_FFPROBE_BIN", "ffprobe"),
            mediamtx_api_url: var_or("HELDAR_MEDIAMTX_API_URL", "http://127.0.0.1:9997"),
            mediamtx_hls_base: var_or("HELDAR_MEDIAMTX_HLS_BASE", "http://127.0.0.1:8888"),
            mediamtx_rtsp_base: var_or("HELDAR_MEDIAMTX_RTSP_BASE", "rtsp://127.0.0.1:8554"),
            mediamtx_webrtc_base: var_or("HELDAR_MEDIAMTX_WEBRTC_BASE", "http://127.0.0.1:8889"),
            media_same_origin: parse_bool("HELDAR_MEDIA_SAME_ORIGIN", false),
            live_token_ttl_secs: parse_or("HELDAR_LIVEVIEW_TOKEN_TTL_SECS", 3600),
            live_reaper_interval_s: parse_or("HELDAR_LIVE_REAPER_INTERVAL_S", 15),
            db_max_connections: parse_or::<u32>("HELDAR_DB_MAX_CONNECTIONS", 16).clamp(2, 256),
            media_job_concurrency: parse_or::<usize>("HELDAR_MEDIA_JOB_CONCURRENCY", 3)
                .clamp(1, 64),
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
            max_db_bytes: (max_db_gb * 1024.0 * 1024.0 * 1024.0) as u64,
            db_autovacuum_convert: parse_bool("HELDAR_DB_AUTOVACUUM_CONVERT", true),
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
            session_max_lifetime_hours: parse_or("HELDAR_SESSION_MAX_LIFETIME_HOURS", 0),
            auth_cookie_secure: parse_bool("HELDAR_AUTH_COOKIE_SECURE", false),
            machine_auth: tier_from_env("HELDAR_MACHINE_AUTH", mode_is_production),
            // NOT auto-promoted by deployment mode, unlike `machine_auth`. The frame ticket is a
            // CLIENT PROTOCOL requirement: enforcing it rejects every worker that does not yet mint
            // one, including third-party and not-yet-upgraded workers. Inferring that from a
            // deployment label would mean an operator who followed the hardening advice
            // (HELDAR_DEPLOYMENT_MODE=production-lan) upgrades and silently loses ALL AI ingest —
            // detection stops with a healthy-looking box. `machine_auth` is safe to promote because
            // it is server-side only and `enforced_caps` deliberately keeps every endpoint a real
            // worker calls. Requiring tickets is an explicit operator decision, taken once the whole
            // fleet speaks the protocol; until then `warn` reports exactly who would break.
            ingest_provenance: tier_from_env("HELDAR_INGEST_PROVENANCE", false),
            frame_ticket_ttl_secs: parse_or::<i64>("HELDAR_FRAME_TICKET_TTL_SECS", 120)
                .clamp(10, 900),
            login_max_failures: parse_or("HELDAR_LOGIN_MAX_FAILURES", 5),
            login_lockout_min: parse_or("HELDAR_LOGIN_LOCKOUT_MIN", 15),
            secret_key_b64: secret("HELDAR_SECRET_KEY"),
            strict_prod: parse_bool("HELDAR_STRICT_PROD", false),
            exposed_declared: parse_bool("HELDAR_INTERNET_EXPOSED", false),
            deployment_mode,
            bootstrap_admin_user: var("HELDAR_BOOTSTRAP_ADMIN_USER"),
            bootstrap_admin_password: secret("HELDAR_BOOTSTRAP_ADMIN_PASSWORD"),
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
            archive_dir_max_bytes: parse_or("HELDAR_ARCHIVE_DIR_MAX_BYTES", 53_687_091_200u64),
            archive_retention_hours: parse_or("HELDAR_ARCHIVE_RETENTION_HOURS", 48),
            onvif_discovery_timeout_ms: parse_or("HELDAR_ONVIF_DISCOVERY_TIMEOUT_MS", 2000),
            onvif_request_timeout_ms: parse_or("HELDAR_ONVIF_REQUEST_TIMEOUT_MS", 5000),
            isapi_request_timeout_ms: parse_or("HELDAR_ISAPI_REQUEST_TIMEOUT_MS", 8000),
            native_anpr_poll_ms: parse_or("HELDAR_NATIVE_ANPR_POLL_MS", 1000),
            camera_events_rearm_secs: parse_or("HELDAR_CAMERA_EVENTS_REARM_SECS", 10),
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
            live_idle_close_secs: var_or("HELDAR_LIVE_IDLE_CLOSE_SECS", "60")
                .parse()
                .unwrap_or(60),
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
            smtp_password: secret("HELDAR_SMTP_PASSWORD"),
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

/// Whether the box is reachable from outside the trusted LAN by any path. Pure (free function) so the
/// exposure decision is unit-tested directly, independent of the full `Config`/environment. The opt-in
/// remote paths (rendezvous/overlay/control-plane) are auto-detected; `declared` is the operator's
/// explicit `HELDAR_INTERNET_EXPOSED` for the paths config can't see (reverse proxy, port-forward,
/// public-cloud bind).
fn is_internet_exposed(
    rendezvous: bool,
    overlay: bool,
    control_plane_registered: bool,
    declared: bool,
) -> bool {
    rendezvous || overlay || control_plane_registered || declared
}

/// The subset of config the production guardrails reason about, extracted so the decision is a pure,
/// fully unit-testable function (no environment, no full `Config`).
#[derive(Debug, Clone)]
struct GuardrailState {
    exposed: bool,
    rendezvous: bool,
    auth_enabled: bool,
    auth_cookie_secure: bool,
    session_idle_timeout_minutes: i64,
    session_ttl_hours: i64,
    cors_has_localhost: bool,
    cors_has_wildcard: bool,
    secret_key_set: bool,
    cp_token_empty: bool,
    strict_prod: bool,
}

/// Outcome of evaluating the guardrails. `Refuse` is an unconditional boot failure (auth off while
/// exposed); `StrictRefuse` carries soft warnings that became fatal under strict mode; `Pass` carries
/// soft warnings to log before booting.
#[derive(Debug)]
enum GuardrailOutcome {
    Pass(Vec<String>),
    Refuse(String),
    StrictRefuse(Vec<String>),
}

/// Pure guardrail decision. Not exposed → always pass. Exposed + auth off → hard refuse. Exposed +
/// auth on → collect soft warnings; refuse only under strict mode.
fn evaluate_production_guardrails(s: &GuardrailState) -> GuardrailOutcome {
    if !s.exposed {
        return GuardrailOutcome::Pass(Vec::new());
    }
    if !s.auth_enabled {
        return GuardrailOutcome::Refuse(
            "remote access is configured (rendezvous, overlay, or control-plane registration) but \
             HELDAR_AUTH_ENABLED=false — refusing to expose the open API. Set HELDAR_AUTH_ENABLED=true."
                .into(),
        );
    }
    let mut warnings: Vec<String> = Vec::new();
    if !s.auth_cookie_secure {
        warnings.push(
            "HELDAR_AUTH_COOKIE_SECURE=false — set true so the session cookie requires HTTPS"
                .into(),
        );
    }
    if s.session_idle_timeout_minutes == 0 {
        warnings.push(
            "HELDAR_SESSION_IDLE_TIMEOUT_MIN=0 — set e.g. 30 to expire idle remote sessions".into(),
        );
    }
    if s.session_ttl_hours > 12 {
        warnings.push(format!(
            "HELDAR_SESSION_TTL_HOURS={} is long for remote access — consider 4 or less",
            s.session_ttl_hours
        ));
    }
    if s.cors_has_localhost {
        warnings.push(
            "HELDAR_CORS_ORIGINS still allows localhost — lock it to the dashboard origin".into(),
        );
    }
    if s.cors_has_wildcard {
        warnings.push(
            "HELDAR_CORS_ORIGINS contains '*' (wildcard) — lock it to the dashboard origin(s)"
                .into(),
        );
    }
    if !s.secret_key_set {
        warnings.push(
            "HELDAR_SECRET_KEY is unset — camera credentials are stored in plaintext at rest"
                .into(),
        );
    }
    if s.rendezvous && s.cp_token_empty {
        warnings.push(
            "HELDAR_CP_TOKEN is empty while a rendezvous is configured — the box cannot authenticate \
             its dial-out and will not serve remote video"
                .into(),
        );
    }
    if warnings.is_empty() {
        return GuardrailOutcome::Pass(warnings);
    }
    if s.strict_prod {
        return GuardrailOutcome::StrictRefuse(warnings);
    }
    GuardrailOutcome::Pass(warnings)
}

/// What the boot path should do about the LAN-trust posture (auth off on a non-loopback bind).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LanTrustOutcome {
    /// Nothing to say: auth is on, or the API is bound loopback-only.
    Silent,
    /// Auth off on a LAN bind — emit the boxed warning, then boot.
    Warn,
    /// Auth off on a LAN bind AND a production deployment mode — refuse to boot.
    Refuse,
}

/// Pure LAN-trust decision, split out so it is unit-testable without the environment or full `Config`.
/// Auth on, or a loopback-only bind → nothing to flag. Auth off on a non-loopback bind trusts the
/// whole LAN as admin: warn by default, or refuse when a production deployment mode is selected.
fn evaluate_lan_trust(
    auth_enabled: bool,
    bind_is_loopback: bool,
    mode_is_production: bool,
) -> LanTrustOutcome {
    if auth_enabled || bind_is_loopback {
        return LanTrustOutcome::Silent;
    }
    if mode_is_production {
        return LanTrustOutcome::Refuse;
    }
    LanTrustOutcome::Warn
}

/// Whether the API bind address is loopback-only (`127.0.0.1`, `::1`, or `localhost`). Anything we
/// cannot confirm as loopback — `0.0.0.0`, `::`, a LAN IP, or an unresolvable hostname — is treated as
/// non-loopback so the warning fails loud rather than silent.
fn bind_is_loopback(api_host: &str) -> bool {
    let host = api_host.trim();
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Emit the prominent, boxed startup warning for the auth-off-on-LAN posture. The box text is fully
/// static (so it stays aligned in logs); the dynamic bind address is named in the lead sentence.
fn warn_lan_trusted_as_admin(api_host: &str) {
    tracing::warn!(
        target: "heldar::security",
        "AUTHENTICATION IS DISABLED and the API is bound to {host} (a non-loopback address), so the \
         box is trusting EVERY device on the LAN as full admin.\n\
         ┌──────────────────────────────────────────────────────────────────────────────┐\n\
         │  ⚠  SECURITY WARNING: HELDAR_AUTH_ENABLED=false on a LAN-reachable API          │\n\
         │                                                                                │\n\
         │  Every device on this network — any camera, IoT gadget, or laptop — can reach  │\n\
         │  the NVR API with FULL ADMIN rights and NO login. A single compromised camera  │\n\
         │  or IoT device on the LAN therefore gets full control of your cameras,         │\n\
         │  recordings, and settings.                                                     │\n\
         │                                                                                │\n\
         │  Remediation (pick one):                                                       │\n\
         │    • Require login:   set HELDAR_AUTH_ENABLED=true       (recommended)          │\n\
         │    • Local dev only:  set HELDAR_API_HOST=127.0.0.1      (loopback, no LAN)      │\n\
         │                                                                                │\n\
         │  This is expected during first-time COMMISSIONING — enable auth before you     │\n\
         │  rely on this box. See docs/PRODUCTION.md for the deployment-mode ladder.       │\n\
         └──────────────────────────────────────────────────────────────────────────────┘",
        host = api_host,
    );
}

#[cfg(test)]
mod guardrail_tests {
    use super::*;

    /// A clean, internet-exposed (via rendezvous) box: auth on, secure cookie, sane session, locked
    /// CORS, key set, dial-out bearer present. Each test perturbs one field.
    fn exposed_clean() -> GuardrailState {
        GuardrailState {
            exposed: true,
            rendezvous: true,
            auth_enabled: true,
            auth_cookie_secure: true,
            session_idle_timeout_minutes: 30,
            session_ttl_hours: 4,
            cors_has_localhost: false,
            cors_has_wildcard: false,
            secret_key_set: true,
            cp_token_empty: false,
            strict_prod: false,
        }
    }

    #[test]
    fn exposure_predicate_covers_every_path() {
        assert!(!is_internet_exposed(false, false, false, false), "LAN-only");
        assert!(is_internet_exposed(true, false, false, false), "rendezvous");
        assert!(is_internet_exposed(false, true, false, false), "overlay");
        assert!(
            is_internet_exposed(false, false, true, false),
            "control plane"
        );
        assert!(
            is_internet_exposed(false, false, false, true),
            "operator-declared (reverse proxy / port-forward / cloud bind)"
        );
    }

    #[test]
    fn lan_only_always_passes_even_with_auth_off() {
        let s = GuardrailState {
            exposed: false,
            auth_enabled: false,
            auth_cookie_secure: false,
            ..exposed_clean()
        };
        assert!(
            matches!(evaluate_production_guardrails(&s), GuardrailOutcome::Pass(w) if w.is_empty())
        );
    }

    #[test]
    fn exposed_with_auth_off_refuses() {
        let s = GuardrailState {
            auth_enabled: false,
            ..exposed_clean()
        };
        assert!(matches!(
            evaluate_production_guardrails(&s),
            GuardrailOutcome::Refuse(_)
        ));
    }

    #[test]
    fn overlay_exposed_with_auth_off_refuses() {
        // The overlay/control-plane path (no rendezvous) must still trip the hard refusal — the bug
        // this fix closes.
        let s = GuardrailState {
            rendezvous: false,
            auth_enabled: false,
            ..exposed_clean()
        };
        assert!(matches!(
            evaluate_production_guardrails(&s),
            GuardrailOutcome::Refuse(_)
        ));
    }

    #[test]
    fn exposed_and_clean_passes_with_no_warnings() {
        assert!(matches!(
            evaluate_production_guardrails(&exposed_clean()),
            GuardrailOutcome::Pass(w) if w.is_empty()
        ));
    }

    #[test]
    fn soft_violation_warns_but_boots() {
        let s = GuardrailState {
            auth_cookie_secure: false,
            ..exposed_clean()
        };
        assert!(
            matches!(evaluate_production_guardrails(&s), GuardrailOutcome::Pass(w) if !w.is_empty())
        );
    }

    #[test]
    fn strict_mode_turns_soft_violation_into_refusal() {
        let s = GuardrailState {
            auth_cookie_secure: false,
            strict_prod: true,
            ..exposed_clean()
        };
        assert!(matches!(
            evaluate_production_guardrails(&s),
            GuardrailOutcome::StrictRefuse(_)
        ));
    }

    #[test]
    fn wildcard_cors_is_flagged() {
        let s = GuardrailState {
            cors_has_wildcard: true,
            ..exposed_clean()
        };
        assert!(
            matches!(evaluate_production_guardrails(&s), GuardrailOutcome::Pass(w) if w.iter().any(|x| x.contains("'*'")))
        );
    }

    #[test]
    fn empty_cp_token_with_rendezvous_is_flagged() {
        let s = GuardrailState {
            cp_token_empty: true,
            ..exposed_clean()
        };
        assert!(
            matches!(evaluate_production_guardrails(&s), GuardrailOutcome::Pass(w) if w.iter().any(|x| x.contains("HELDAR_CP_TOKEN")))
        );
    }

    // --- LAN-trust guardrail (auth off on a non-loopback bind) -------------------------------------

    #[test]
    fn lan_trust_warns_when_auth_off_and_non_loopback() {
        // The gap the audit flagged: auth off + LAN bind (0.0.0.0), no remote access → warn, not silent.
        assert_eq!(
            evaluate_lan_trust(false, false, false),
            LanTrustOutcome::Warn
        );
    }

    #[test]
    fn lan_trust_silent_when_auth_off_but_loopback() {
        // Dev posture: auth off but bound to 127.0.0.1/::1 — no LAN exposure, nothing to warn about.
        assert_eq!(
            evaluate_lan_trust(false, true, false),
            LanTrustOutcome::Silent
        );
    }

    #[test]
    fn lan_trust_silent_when_auth_on() {
        // Auth on: the LAN is not trusted as admin regardless of bind address.
        assert_eq!(
            evaluate_lan_trust(true, false, false),
            LanTrustOutcome::Silent
        );
        assert_eq!(
            evaluate_lan_trust(true, true, false),
            LanTrustOutcome::Silent
        );
    }

    #[test]
    fn lan_trust_refuses_in_production_mode_when_auth_off_and_non_loopback() {
        // HELDAR_DEPLOYMENT_MODE=production tightens the warning into a hard boot refusal.
        assert_eq!(
            evaluate_lan_trust(false, false, true),
            LanTrustOutcome::Refuse
        );
    }

    #[test]
    fn lan_trust_production_mode_does_not_bite_loopback_or_auth_on() {
        // Production mode only refuses the actually-unsafe posture; loopback or auth-on stay silent.
        assert_eq!(
            evaluate_lan_trust(false, true, true),
            LanTrustOutcome::Silent
        );
        assert_eq!(
            evaluate_lan_trust(true, false, true),
            LanTrustOutcome::Silent
        );
    }

    #[test]
    fn bind_is_loopback_classifies_addresses() {
        assert!(bind_is_loopback("127.0.0.1"), "IPv4 loopback");
        assert!(bind_is_loopback("127.0.0.5"), "IPv4 loopback range");
        assert!(bind_is_loopback("::1"), "IPv6 loopback");
        assert!(bind_is_loopback("localhost"), "localhost hostname");
        assert!(
            bind_is_loopback("LOCALHOST"),
            "localhost is case-insensitive"
        );
        assert!(bind_is_loopback("  127.0.0.1  "), "trims surrounding space");

        assert!(!bind_is_loopback("0.0.0.0"), "all-interfaces bind");
        assert!(!bind_is_loopback("::"), "IPv6 unspecified");
        assert!(!bind_is_loopback("192.168.1.10"), "LAN address");
        assert!(!bind_is_loopback("10.0.0.1"), "private LAN address");
        // A hostname we can't confirm as loopback is treated as non-loopback (fail-loud).
        assert!(!bind_is_loopback("nvr.local"), "unresolvable hostname");
        assert!(!bind_is_loopback(""), "empty string");
    }
}
