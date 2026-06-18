use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::Json;
use sqlx::FromRow;

/// Config keys that hold a secret. Masked in [`BackupDestinationView`] (and preserved across an
/// update when the client round-trips the `***` placeholder back).
pub const BACKUP_SECRET_KEYS: &[&str] = &["pass", "password", "secret_key", "secret"];

/// A backup transfer target. `config` is a kind-specific JSON blob (credentials live here and are
/// never serialized raw — use [`BackupDestinationView`]). Not `Serialize` for exactly that reason.
#[derive(Debug, Clone, FromRow)]
pub struct BackupDestination {
    pub id: String,
    pub name: String,
    /// `local` | `sftp` | `ftp` | `s3`.
    pub kind: String,
    pub config: Json<Value>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Client-facing destination: secret config values are replaced with `***`.
#[derive(Debug, Clone, Serialize)]
pub struct BackupDestinationView {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// The config blob with any secret values masked to `***`.
    pub config: Value,
    /// Whether at least one secret credential is configured (so the UI can show "set" without the value).
    pub has_credentials: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mask the secret values in a config blob, returning the masked blob and whether any secret was set.
pub fn mask_backup_config(mut config: Value) -> (Value, bool) {
    let mut has_credentials = false;
    if let Some(obj) = config.as_object_mut() {
        for key in BACKUP_SECRET_KEYS {
            if let Some(v) = obj.get_mut(*key) {
                if v.as_str().map(|s| !s.is_empty()).unwrap_or(false) {
                    has_credentials = true;
                    *v = Value::String("***".to_string());
                }
            }
        }
    }
    (config, has_credentials)
}

impl From<BackupDestination> for BackupDestinationView {
    fn from(d: BackupDestination) -> Self {
        let (config, has_credentials) = mask_backup_config(d.config.0);
        BackupDestinationView {
            id: d.id,
            name: d.name,
            kind: d.kind,
            config,
            has_credentials,
            enabled: d.enabled,
            created_at: d.created_at,
            updated_at: d.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct BackupDestinationCreate {
    pub name: String,
    /// `local` | `sftp` | `ftp` | `s3`.
    pub kind: String,
    pub config: Option<Value>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct BackupDestinationUpdate {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub config: Option<Value>,
    pub enabled: Option<bool>,
}

/// Result of POST /api/v1/backup/destinations/{id}/test (a connectivity / writability probe).
#[derive(Debug, Clone, Serialize)]
pub struct BackupTestResult {
    pub ok: bool,
    pub error: Option<String>,
    pub latency_ms: i64,
}

/// A scheduled backup policy: ship a camera selection's recent footage to a destination on an interval.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct BackupPolicy {
    pub id: String,
    pub name: String,
    pub destination_id: String,
    /// JSON array of camera ids; empty array means all cameras.
    pub camera_ids: Json<Value>,
    pub incident_lock_only: bool,
    pub schedule_interval_s: i64,
    pub lookback_hours: i64,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_job_id: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct BackupPolicyCreate {
    pub name: String,
    pub destination_id: String,
    pub camera_ids: Option<Value>,
    pub incident_lock_only: Option<bool>,
    pub schedule_interval_s: Option<i64>,
    pub lookback_hours: Option<i64>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct BackupPolicyUpdate {
    pub name: Option<String>,
    pub destination_id: Option<String>,
    pub camera_ids: Option<Value>,
    pub incident_lock_only: Option<bool>,
    pub schedule_interval_s: Option<i64>,
    pub lookback_hours: Option<i64>,
    pub enabled: Option<bool>,
}

/// A single backup run (policy-scheduled, manually triggered, or an on-demand archive export).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct BackupJob {
    pub id: String,
    pub policy_id: Option<String>,
    pub destination_id: Option<String>,
    /// `policy` | `on_demand_archive`.
    pub kind: String,
    pub camera_ids: Json<Value>,
    pub from_time: Option<DateTime<Utc>>,
    pub to_time: Option<DateTime<Utc>>,
    pub incident_lock_only: bool,
    /// `pending` | `running` | `completed` | `error`.
    pub status: String,
    pub files_total: i64,
    pub files_copied: i64,
    pub bytes_copied: i64,
    pub error: Option<String>,
    /// Filesystem path of the produced artifact (archive .zip), if any.
    pub output_path: Option<String>,
    /// Browser-fetchable URL of the produced artifact (under /media/archives/...), if any.
    pub output_url: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Request body for POST /api/v1/archive/export — zip a selection of recorded footage on demand.
#[derive(Debug, Deserialize)]
pub struct ArchiveExportRequest {
    /// Camera ids to include; empty/omitted means all cameras.
    #[serde(default)]
    pub camera_ids: Vec<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub incident_lock_only: Option<bool>,
    /// Trim each segment to the [from, to] window (re-mux with -c copy); requires both bounds.
    pub trim: Option<bool>,
}
