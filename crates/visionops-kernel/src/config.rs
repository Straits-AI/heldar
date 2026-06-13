use std::env;
use std::path::PathBuf;

/// Runtime configuration, loaded from environment (see `.env.example`).
#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub data_dir: PathBuf,
    pub recordings_dir: PathBuf,
    pub clips_dir: PathBuf,
    pub snapshots_dir: PathBuf,
    pub frames_dir: PathBuf,
    pub ffmpeg_bin: String,
    pub ffprobe_bin: String,
    pub mediamtx_api_url: String,
    pub mediamtx_hls_base: String,
    pub mediamtx_rtsp_base: String,
    pub mediamtx_webrtc_base: String,
    pub recorder_enabled: bool,
    pub default_segment_seconds: i64,
    pub default_retention_hours: i64,
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
    /// Optional webhook URL that receives warning/critical events as JSON (alerting).
    pub alert_webhook_url: Option<String>,
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
    // ---- Stage 4: Campus Entry / RBAC ----
    /// Master switch for authentication + RBAC. When false, the API is open (dev/single-tenant
    /// LAN appliance default) and a synthetic admin principal is used. When true, the Stage 4
    /// entry/admin surface requires a valid bearer token (session or API key) and enforces roles.
    pub auth_enabled: bool,
    /// Lifetime of an issued login session token.
    pub session_ttl_hours: i64,
    /// Optional first-run admin bootstrap (only used when no users exist yet).
    pub bootstrap_admin_user: Option<String>,
    pub bootstrap_admin_password: Option<String>,
    /// Minimum reads agreeing on a track's winning plate before the ANPR engine commits an entry
    /// event (temporal voting). Lower = faster but noisier; higher = more accurate but more latency.
    pub anpr_min_votes: u32,
    /// How long old entry events / audit log rows are kept before retention prunes them.
    pub entry_retention_days: i64,
}

fn var(key: &str) -> Option<String> {
    env::var(key).ok().filter(|s| !s.trim().is_empty())
}

fn var_or(key: &str, default: &str) -> String {
    var(key).unwrap_or_else(|| default.to_string())
}

fn parse_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    var(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn parse_bool(key: &str, default: bool) -> bool {
    match var(key) {
        Some(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        None => default,
    }
}

impl Config {
    pub fn from_env() -> Self {
        let data_dir = PathBuf::from(var_or("VISIONOPS_DATA_DIR", "./data"));
        let recordings_dir = var("VISIONOPS_RECORDINGS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("recordings"));
        let clips_dir = var("VISIONOPS_CLIPS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("clips"));
        let snapshots_dir = var("VISIONOPS_SNAPSHOTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("snapshots"));
        let frames_dir = var("VISIONOPS_FRAMES_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("frames"));

        let cors_origins = var_or("VISIONOPS_CORS_ORIGINS", "http://localhost:5173")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let max_recordings_gb: f64 = parse_or("VISIONOPS_MAX_RECORDINGS_GB", 20.0);
        let min_free_disk_gb: f64 = parse_or("VISIONOPS_MIN_FREE_DISK_GB", 5.0);

        Config {
            database_url: var_or("VISIONOPS_DATABASE_URL", "sqlite://./data/visionops.db"),
            data_dir,
            recordings_dir,
            clips_dir,
            snapshots_dir,
            frames_dir,
            ffmpeg_bin: var_or("VISIONOPS_FFMPEG_BIN", "ffmpeg"),
            ffprobe_bin: var_or("VISIONOPS_FFPROBE_BIN", "ffprobe"),
            mediamtx_api_url: var_or("VISIONOPS_MEDIAMTX_API_URL", "http://127.0.0.1:9997"),
            mediamtx_hls_base: var_or("VISIONOPS_MEDIAMTX_HLS_BASE", "http://127.0.0.1:8888"),
            mediamtx_rtsp_base: var_or("VISIONOPS_MEDIAMTX_RTSP_BASE", "rtsp://127.0.0.1:8554"),
            mediamtx_webrtc_base: var_or("VISIONOPS_MEDIAMTX_WEBRTC_BASE", "http://127.0.0.1:8889"),
            recorder_enabled: parse_bool("VISIONOPS_RECORDER_ENABLED", true),
            default_segment_seconds: parse_or("VISIONOPS_DEFAULT_SEGMENT_SECONDS", 60),
            default_retention_hours: parse_or("VISIONOPS_DEFAULT_RETENTION_HOURS", 24),
            indexer_interval_s: parse_or("VISIONOPS_INDEXER_INTERVAL_S", 10),
            health_interval_s: parse_or("VISIONOPS_HEALTH_INTERVAL_S", 15),
            retention_interval_s: parse_or("VISIONOPS_RETENTION_INTERVAL_S", 300),
            api_host: var_or("VISIONOPS_API_HOST", "0.0.0.0"),
            api_port: parse_or("VISIONOPS_API_PORT", 8000),
            cors_origins,
            max_recordings_bytes: (max_recordings_gb * 1024.0 * 1024.0 * 1024.0) as u64,
            min_free_disk_bytes: (min_free_disk_gb * 1024.0 * 1024.0 * 1024.0) as u64,
            alert_webhook_url: var("VISIONOPS_ALERT_WEBHOOK_URL"),
            notifier_interval_s: parse_or("VISIONOPS_NOTIFIER_INTERVAL_S", 15),
            ai_enabled: parse_bool("VISIONOPS_AI_ENABLED", true),
            ai_max_total_fps: parse_or("VISIONOPS_AI_MAX_TOTAL_FPS", 40.0),
            default_ai_fps: parse_or("VISIONOPS_DEFAULT_AI_FPS", 5.0),
            default_ai_width: parse_or("VISIONOPS_DEFAULT_AI_WIDTH", 1280),
            detection_retention_hours: parse_or("VISIONOPS_DETECTION_RETENTION_HOURS", 168),
            auth_enabled: parse_bool("VISIONOPS_AUTH_ENABLED", false),
            session_ttl_hours: parse_or("VISIONOPS_SESSION_TTL_HOURS", 12),
            bootstrap_admin_user: var("VISIONOPS_BOOTSTRAP_ADMIN_USER"),
            bootstrap_admin_password: var("VISIONOPS_BOOTSTRAP_ADMIN_PASSWORD"),
            anpr_min_votes: parse_or::<u32>("VISIONOPS_ANPR_MIN_VOTES", 3).clamp(1, 50),
            entry_retention_days: parse_or("VISIONOPS_ENTRY_RETENTION_DAYS", 365),
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
