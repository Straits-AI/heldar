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

        let cors_origins = var_or("VISIONOPS_CORS_ORIGINS", "http://localhost:5173")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let max_recordings_gb: f64 = parse_or("VISIONOPS_MAX_RECORDINGS_GB", 20.0);

        Config {
            database_url: var_or("VISIONOPS_DATABASE_URL", "sqlite://./data/visionops.db"),
            data_dir,
            recordings_dir,
            clips_dir,
            snapshots_dir,
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
        }
    }

    /// Directory where a camera's segments are stored.
    pub fn camera_recordings_dir(&self, camera_id: &str) -> PathBuf {
        self.recordings_dir.join(camera_id)
    }
}
