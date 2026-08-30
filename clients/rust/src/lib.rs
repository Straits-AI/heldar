// GENERATED FROM openapi.json BY scripts/gen_clients.py — DO NOT EDIT.
//
// Regenerate with:  cargo test -p heldar-server --test openapi_contract write_the_served_document
//                   python3 scripts/gen_clients.py target/openapi.json clients
//
// Contract version: 0.1.0

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiIngest {
    pub camera_id: String,
    pub detections: Option<Vec<DetectionIngest>>,
    pub event: Option<IngestEvent>,
    pub frame_id: Option<String>,
    pub frame_ticket: Option<String>,
    pub task_type: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiTaskCreate {
    pub config: Option<serde_json::Value>,
    pub enabled: Option<bool>,
    pub fps: Option<f64>,
    pub stream_profile: Option<String>,
    pub task_type: String,
    pub width: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiTaskUpdate {
    pub config: Option<serde_json::Value>,
    pub enabled: Option<bool>,
    pub fps: Option<f64>,
    pub stream_profile: Option<String>,
    pub task_type: Option<String>,
    pub width: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyCreate {
    pub capabilities: Option<Vec<String>>,
    pub confirm_privileged: Option<bool>,
    pub expires_at: Option<String>,
    pub name: String,
    #[serde(rename = "role")]
    pub role: Option<String>,
    pub scope_cameras: Option<Vec<String>>,
    pub scope_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyUpdate {
    pub active: Option<bool>,
    pub capabilities: Option<Vec<String>>,
    pub confirm_privileged: Option<bool>,
    pub expires_at: Option<String>,
    #[serde(rename = "revoked_at")]
    pub revoked_at: Option<String>,
    pub scope_cameras: Option<Vec<String>>,
    pub scope_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveExportRequest {
    pub camera_ids: Option<Vec<String>>,
    pub from: Option<String>,
    pub incident_lock_only: Option<bool>,
    pub to: Option<String>,
    pub trim: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupDestinationCreate {
    pub config: Option<serde_json::Value>,
    pub enabled: Option<bool>,
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupDestinationUpdate {
    pub config: Option<serde_json::Value>,
    pub enabled: Option<bool>,
    pub kind: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupDestinationView {
    pub config: serde_json::Value,
    pub created_at: String,
    pub enabled: bool,
    pub has_credentials: bool,
    pub id: String,
    pub kind: String,
    pub name: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupJob {
    pub bytes_copied: i64,
    pub camera_ids: Vec<String>,
    pub created_at: String,
    pub created_by: Option<String>,
    pub created_by_kind: Option<String>,
    pub destination_id: Option<String>,
    pub error: Option<String>,
    pub files_copied: i64,
    pub files_total: i64,
    pub finished_at: Option<String>,
    pub from_time: Option<String>,
    pub id: String,
    pub incident_lock_only: bool,
    pub kind: String,
    pub output_path: Option<String>,
    pub output_url: Option<String>,
    pub policy_id: Option<String>,
    pub started_at: Option<String>,
    pub status: String,
    pub to_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupPolicy {
    pub camera_ids: Vec<String>,
    pub created_at: String,
    pub destination_id: String,
    pub enabled: bool,
    pub id: String,
    pub incident_lock_only: bool,
    pub last_job_id: Option<String>,
    pub last_run_at: Option<String>,
    pub lookback_hours: i64,
    pub name: String,
    pub schedule_interval_s: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupPolicyCreate {
    pub camera_ids: Option<serde_json::Value>,
    pub destination_id: String,
    pub enabled: Option<bool>,
    pub incident_lock_only: Option<bool>,
    pub lookback_hours: Option<i64>,
    pub name: String,
    pub schedule_interval_s: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupPolicyUpdate {
    pub camera_ids: Option<serde_json::Value>,
    pub destination_id: Option<String>,
    pub enabled: Option<bool>,
    pub incident_lock_only: Option<bool>,
    pub lookback_hours: Option<i64>,
    pub name: Option<String>,
    pub schedule_interval_s: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupTestResult {
    pub error: Option<String>,
    pub latency_ms: i64,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkAction {
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkCameraResult {
    pub camera_id: String,
    pub error: Option<String>,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkConfigRequest {
    pub action: BulkAction,
    pub camera_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkConfigResponse {
    pub failed: i64,
    #[serde(rename = "results")]
    pub results: Vec<BulkCameraResult>,
    pub succeeded: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraLinkCreate {
    pub bidirectional: Option<bool>,
    pub from_camera: String,
    pub note: Option<String>,
    pub to_camera: String,
    pub transit_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraView {
    pub address: Option<String>,
    pub anr_enabled: bool,
    pub anr_replay_url_template: Option<String>,
    pub capabilities: serde_json::Value,
    pub codec: Option<String>,
    pub created_at: String,
    pub enabled: bool,
    pub fps_main: Option<i64>,
    pub fps_sub: Option<i64>,
    pub has_password: bool,
    pub id: String,
    pub live_warm: bool,
    pub mirror_enabled: bool,
    pub model: Option<String>,
    pub name: String,
    pub native_anpr_enabled: bool,
    pub native_events_enabled: bool,
    pub post_roll_seconds: i64,
    pub pre_roll_seconds: i64,
    pub priority: i64,
    #[serde(rename = "record_audio")]
    pub record_audio: bool,
    #[serde(rename = "record_enabled")]
    pub record_enabled: bool,
    #[serde(rename = "record_mode")]
    pub record_mode: String,
    #[serde(rename = "record_stream")]
    pub record_stream: String,
    #[serde(rename = "record_url_masked")]
    pub record_url_masked: Option<String>,
    #[serde(rename = "resolution_main")]
    pub resolution_main: Option<String>,
    #[serde(rename = "resolution_sub")]
    pub resolution_sub: Option<String>,
    #[serde(rename = "retention_hours")]
    pub retention_hours: i64,
    #[serde(rename = "rtsp_port")]
    pub rtsp_port: i64,
    pub segment_seconds: i64,
    pub site_id: Option<String>,
    pub storage_quota_bytes: Option<i64>,
    pub updated_at: String,
    pub username: Option<String>,
    pub vendor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipRequest {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuousMoveRequest {
    pub pan: Option<f64>,
    pub tilt: Option<f64>,
    pub zoom: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayNightConfig {
    pub mode: String,
    pub sensitivity: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayNightPatch {
    pub mode: Option<String>,
    pub sensitivity: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbConvertResult {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbLimitUpdate {
    pub max_db_gb: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbStatus {
    pub db_bytes: i64,
    pub incremental: bool,
    pub max_db_bytes: i64,
    pub max_db_gb: f64,
    pub max_overridden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionIngest {
    pub attributes: Option<serde_json::Value>,
    pub bbox: Option<serde_json::Value>,
    pub confidence: Option<f64>,
    pub label: Option<String>,
    pub track_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionUpdate {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_name: Option<String>,
    pub firmware_version: Option<String>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingIngest {
    pub camera_id: String,
    pub dim: i64,
    pub frame_id: Option<String>,
    pub frame_ticket: Option<String>,
    pub items: Vec<EmbeddingItem>,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingItem {
    pub bbox: Option<serde_json::Value>,
    pub detection_id: Option<String>,
    pub label: Option<String>,
    pub thumb_b64: Option<String>,
    pub timestamp: Option<String>,
    pub track_id: Option<String>,
    pub vec: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsureOnvifUserRequest {
    pub password: String,
    pub user_type: Option<OnvifUserType>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub error: String,
    #[serde(rename = "retryable")]
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRequest {
    pub camera_id: Option<String>,
    pub dry_run: Option<bool>,
    pub from: String,
    pub incident_id: Option<String>,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatePolicy {
    pub camera_id: String,
    pub enabled: bool,
    pub output_port: i64,
    pub pulse_ms: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatePolicyUpdate {
    pub enabled: Option<bool>,
    pub output_port: Option<i64>,
    pub pulse_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSettingsUpdate {
    pub kill_switch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GotoPresetRequest {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageConfig {
    pub blc_enabled: Option<bool>,
    pub brightness: Option<i64>,
    pub contrast: Option<i64>,
    pub ir_light_brightness: Option<i64>,
    pub saturation: Option<i64>,
    pub supplement_brightness_mode: Option<String>,
    pub supplement_light_mode: Option<String>,
    pub wdr_level: Option<i64>,
    pub wdr_mode: Option<String>,
    pub white_light_brightness: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestEvent {
    pub event_type: String,
    pub payload: Option<serde_json::Value>,
    pub severity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntrusionConfig {
    pub enabled: bool,
    #[serde(rename = "regions")]
    pub regions: Vec<SmartRegion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoOutput {
    pub default_state: Option<String>,
    pub id: i64,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRequest {
    pub max_tasks: Option<i64>,
    pub task_types: Option<Vec<String>>,
    pub ttl_secs: Option<i64>,
    pub worker_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineCrossingConfig {
    pub enabled: bool,
    pub lines: Vec<SmartLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub password: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotionConfig {
    pub enabled: bool,
    pub sensitivity: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NlBody {
    pub query: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NtpConfig {
    pub addressing_format: String,
    pub host_name: String,
    pub port: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnvifSettings {
    pub isapi_enabled: bool,
    pub onvif_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OnvifUserType {
    #[serde(rename = "administrator")]
    Administrator,
    #[serde(rename = "operator")]
    Operator,
    #[serde(rename = "mediaUser")]
    Mediauser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsdConfig {
    pub channel_name_enabled: bool,
    pub date_style: Option<String>,
    pub datetime_enabled: bool,
    pub display_week: Option<bool>,
    pub time_style: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeRequest {
    pub device_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PulseRequest {
    pub pulse_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPlan {
    pub auth_status: Option<Vec<String>>,
    pub cameras: Option<Vec<String>>,
    pub color: Option<String>,
    pub event_type: Option<String>,
    pub from: Option<String>,
    pub hour_max: Option<i64>,
    pub hour_min: Option<i64>,
    pub limit: Option<i64>,
    pub plate: Option<String>,
    pub sources: Option<Vec<String>>,
    pub subject_type: Option<String>,
    pub text: Option<String>,
    pub to: Option<String>,
    pub tz: Option<String>,
    pub vehicle_type: Option<String>,
    pub zone: Option<String>,
    pub zone_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub dim: Option<i64>,
    pub error: Option<String>,
    pub model: Option<String>,
    pub vec: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebootRequest {
    pub confirm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveBody {
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionLimits {
    pub max_overridden: bool,
    pub max_recordings_bytes: i64,
    pub max_recordings_gb: f64,
    pub min_free_disk_bytes: i64,
    pub min_free_disk_gb: f64,
    pub min_free_overridden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionUpdate {
    pub max_recordings_gb: Option<f64>,
    pub min_free_disk_gb: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticBody {
    pub cameras: Option<Vec<String>>,
    pub from: Option<String>,
    pub image_b64: Option<String>,
    pub k: Option<i64>,
    pub label: Option<String>,
    pub text: Option<String>,
    pub to: Option<String>,
    pub zone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteCreate {
    pub id: String,
    pub name: String,
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteRow {
    pub created_at: String,
    pub id: String,
    pub name: String,
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteUpdate {
    pub name: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartLine {
    pub direction: String,
    pub enabled: bool,
    pub id: i64,
    pub points: Vec<Vec<f64>>,
    pub sensitivity: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartRegion {
    pub enabled: bool,
    pub id: i64,
    pub points: Vec<Vec<f64>>,
    pub sensitivity: i64,
    pub time_threshold: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeConfig {
    pub local_time: String,
    pub time_mode: String,
    pub time_zone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimezoneSettings {
    pub configured: Option<String>,
    pub server_local_offset: String,
    pub source: TzSource,
    pub unconfigured_behaviour: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimezoneUpdate {
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeSettings {
    pub engine: String,
    pub env_default: String,
    pub nvenc_available: bool,
    pub overridden: bool,
    pub vaapi_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeUpdate {
    pub engine: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TzSource {
    #[serde(rename = "site")]
    Site,
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "unset")]
    Unset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserCreate {
    pub active: Option<bool>,
    pub display_name: Option<String>,
    pub password: String,
    #[serde(rename = "role")]
    pub role: Option<String>,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserUpdate {
    pub active: Option<bool>,
    pub display_name: Option<String>,
    pub password: Option<String>,
    #[serde(rename = "role")]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserView {
    pub active: bool,
    pub created_at: String,
    pub display_name: Option<String>,
    pub id: String,
    #[serde(rename = "role")]
    pub role: String,
    pub updated_at: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vehicle {
    pub active: bool,
    pub color: Option<String>,
    pub created_at: String,
    pub id: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub notes: Option<String>,
    pub owner_name: Option<String>,
    pub owner_ref: Option<String>,
    pub owner_type: String,
    pub plate: String,
    pub plate_norm: String,
    pub site_id: Option<String>,
    pub updated_at: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub vehicle_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VehicleCreate {
    pub active: Option<bool>,
    pub color: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub notes: Option<String>,
    pub owner_name: Option<String>,
    pub owner_ref: Option<String>,
    pub owner_type: Option<String>,
    pub plate: String,
    pub site_id: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub vehicle_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VehicleUpdate {
    pub active: Option<bool>,
    pub color: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub notes: Option<String>,
    pub owner_name: Option<String>,
    pub owner_ref: Option<String>,
    pub owner_type: Option<String>,
    pub plate: Option<String>,
    pub site_id: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub vehicle_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    pub bitrate: i64,
    pub channel_id: i64,
    pub channel_name: Option<String>,
    pub codec: String,
    pub fps: i64,
    pub gop: i64,
    pub height: i64,
    pub quality_control: String,
    pub vbr_upper_cap: i64,
    pub width: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfigPatch {
    pub bitrate: Option<i64>,
    pub codec: Option<String>,
    pub fps: Option<i64>,
    pub gop: Option<i64>,
    pub height: Option<i64>,
    pub quality_control: Option<String>,
    pub vbr_upper_cap: Option<i64>,
    pub width: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisitorPass {
    pub checked_in_at: Option<String>,
    pub checked_out_at: Option<String>,
    pub code: String,
    pub company: Option<String>,
    pub created_at: String,
    pub created_by: Option<String>,
    pub host: Option<String>,
    pub id: String,
    pub phone: Option<String>,
    pub plate: Option<String>,
    pub plate_norm: Option<String>,
    pub purpose: Option<String>,
    pub site_id: Option<String>,
    pub status: String,
    pub updated_at: String,
    pub valid_from: String,
    pub valid_until: String,
    pub vehicle_desc: Option<String>,
    pub visitor_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisitorPassCreate {
    pub company: Option<String>,
    pub host: Option<String>,
    pub phone: Option<String>,
    pub plate: Option<String>,
    pub purpose: Option<String>,
    pub site_id: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub vehicle_desc: Option<String>,
    pub visitor_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisitorPassUpdate {
    pub company: Option<String>,
    pub host: Option<String>,
    pub phone: Option<String>,
    pub plate: Option<String>,
    pub purpose: Option<String>,
    pub status: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub vehicle_desc: Option<String>,
    pub visitor_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watchlist {
    pub active: bool,
    pub created_at: String,
    pub created_by: Option<String>,
    pub id: String,
    pub kind: String,
    pub plate: String,
    pub plate_norm: String,
    #[serde(rename = "reason")]
    pub reason: Option<String>,
    pub severity: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistCreate {
    pub active: Option<bool>,
    pub kind: Option<String>,
    pub plate: String,
    #[serde(rename = "reason")]
    pub reason: Option<String>,
    pub severity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistUpdate {
    pub active: Option<bool>,
    pub kind: Option<String>,
    #[serde(rename = "reason")]
    pub reason: Option<String>,
    pub severity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDelivery {
    pub attempts: i64,
    pub created_at: String,
    pub delivered_at: Option<String>,
    pub error: Option<String>,
    pub event_id: Option<String>,
    pub event_type: Option<String>,
    pub id: String,
    #[serde(rename = "response_code")]
    pub response_code: Option<i64>,
    pub status: String,
    pub subscription_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSubscriptionCreate {
    pub enabled: Option<bool>,
    pub event_types: Option<Vec<String>>,
    pub min_severity: Option<String>,
    pub name: String,
    pub secret: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSubscriptionUpdate {
    pub enabled: Option<bool>,
    pub event_types: Option<Vec<String>>,
    pub min_severity: Option<String>,
    pub name: Option<String>,
    pub secret: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSubscriptionView {
    pub created_at: String,
    pub cursor_at: Option<String>,
    pub enabled: bool,
    pub event_types: Vec<String>,
    pub has_secret: bool,
    pub id: String,
    pub min_severity: String,
    pub name: String,
    pub updated_at: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookTestResult {
    pub error: Option<String>,
    pub ok: bool,
    pub status: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneCreate {
    pub config: Option<serde_json::Value>,
    pub dwell_seconds: Option<f64>,
    pub enabled: Option<bool>,
    pub kind: Option<String>,
    pub labels: Option<serde_json::Value>,
    pub name: String,
    pub polygon: serde_json::Value,
    pub severity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneUpdate {
    pub config: Option<serde_json::Value>,
    pub dwell_seconds: Option<f64>,
    pub enabled: Option<bool>,
    pub kind: Option<String>,
    pub labels: Option<serde_json::Value>,
    pub name: Option<String>,
    pub polygon: Option<serde_json::Value>,
    pub severity: Option<String>,
}

/// What each operation requires, from the contract's own extensions.
pub const REQUIREMENTS: &[(&str, &str, Option<&str>, &str)] = &[
    ("DELETE", "/api/v1/ai-tasks/{task_id}", Some("registry:manage"), "camera-keyed"),
    ("PATCH", "/api/v1/ai-tasks/{task_id}", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/ai/embed-queries", Some("ai:embedwork"), "scope-neutral"),
    ("POST", "/api/v1/ai/embed-queries/{id}/result", Some("ai:embedwork"), "scope-neutral"),
    ("POST", "/api/v1/ai/embeddings", Some("ai:ingest"), "camera-keyed"),
    ("POST", "/api/v1/ai/events", Some("ai:ingest"), "camera-keyed"),
    ("POST", "/api/v1/ai/leases", Some("ai:tasks"), "scope-filtered"),
    ("DELETE", "/api/v1/ai/leases/{lease_id}", Some("ai:tasks"), "scope-neutral"),
    ("GET", "/api/v1/ai/samplers", Some("ai:tasks"), "scope-filtered"),
    ("GET", "/api/v1/ai/tasks", Some("ai:tasks"), "scope-filtered"),
    ("GET", "/api/v1/api-keys", None, "fleet-only"),
    ("POST", "/api/v1/api-keys", None, "fleet-only"),
    ("DELETE", "/api/v1/api-keys/{id}", None, "fleet-only"),
    ("PATCH", "/api/v1/api-keys/{id}", None, "fleet-only"),
    ("POST", "/api/v1/archive/export", Some("registry:manage"), "scope-filtered"),
    ("GET", "/api/v1/archive/exports", Some("system:read"), "scope-filtered"),
    ("GET", "/api/v1/audit", Some("registry:manage"), "scope-filtered"),
    ("POST", "/api/v1/auth/login", None, "scope-neutral"),
    ("POST", "/api/v1/auth/logout", None, "scope-neutral"),
    ("GET", "/api/v1/auth/me", None, "scope-neutral"),
    ("GET", "/api/v1/backup/destinations", Some("system:read"), "fleet-only"),
    ("POST", "/api/v1/backup/destinations", Some("registry:manage"), "fleet-only"),
    ("DELETE", "/api/v1/backup/destinations/{id}", Some("registry:manage"), "fleet-only"),
    ("PATCH", "/api/v1/backup/destinations/{id}", Some("registry:manage"), "fleet-only"),
    ("POST", "/api/v1/backup/destinations/{id}/test", Some("registry:manage"), "fleet-only"),
    ("GET", "/api/v1/backup/jobs", Some("system:read"), "scope-filtered"),
    ("DELETE", "/api/v1/backup/jobs/{id}", Some("registry:manage"), "scope-filtered"),
    ("GET", "/api/v1/backup/jobs/{id}", Some("system:read"), "scope-filtered"),
    ("GET", "/api/v1/backup/policies", Some("system:read"), "scope-filtered"),
    ("POST", "/api/v1/backup/policies", Some("registry:manage"), "scope-filtered"),
    ("DELETE", "/api/v1/backup/policies/{id}", Some("registry:manage"), "scope-filtered"),
    ("PATCH", "/api/v1/backup/policies/{id}", Some("registry:manage"), "scope-filtered"),
    ("POST", "/api/v1/backup/policies/{id}/trigger", Some("registry:manage"), "scope-filtered"),
    ("GET", "/api/v1/cameras", Some("camera:read"), "scope-filtered"),
    ("POST", "/api/v1/cameras/config/bulk", Some("registry:manage"), "scope-filtered"),
    ("DELETE", "/api/v1/cameras/{id}", Some("admin"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}", Some("camera:read"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/ai-tasks", Some("ai:tasks"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/ai-tasks", Some("registry:manage"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/clip", Some("video:export"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/config/device_info", Some("camera:read"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/config/onvif", Some("camera:read"), "camera-keyed"),
    ("PUT", "/api/v1/cameras/{id}/config/onvif", Some("registry:manage"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/config/onvif/ensure_user", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/config/osd", Some("camera:read"), "camera-keyed"),
    ("PUT", "/api/v1/cameras/{id}/config/osd", Some("registry:manage"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/config/reboot", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/config/time", Some("camera:read"), "camera-keyed"),
    ("PUT", "/api/v1/cameras/{id}/config/time", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/config/time/ntp", Some("camera:read"), "camera-keyed"),
    ("PUT", "/api/v1/cameras/{id}/config/time/ntp", Some("registry:manage"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/config/time/sync_now", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/config/video", Some("camera:read"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/config/video/{channel}", Some("camera:read"), "camera-keyed"),
    ("PUT", "/api/v1/cameras/{id}/config/video/{channel}", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/control/capabilities", Some("camera:read"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/control/day_night", Some("camera:read"), "camera-keyed"),
    ("PUT", "/api/v1/cameras/{id}/control/day_night", Some("registry:manage"), "camera-keyed"),
    ("PUT", "/api/v1/cameras/{id}/control/detections/{kind}", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/control/image", Some("camera:read"), "camera-keyed"),
    ("PUT", "/api/v1/cameras/{id}/control/image", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/control/intrusion", Some("camera:read"), "camera-keyed"),
    ("PUT", "/api/v1/cameras/{id}/control/intrusion", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/control/io/outputs", Some("camera:read"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/control/io/outputs/{port}/pulse", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/control/line_crossing", Some("camera:read"), "camera-keyed"),
    ("PUT", "/api/v1/cameras/{id}/control/line_crossing", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/control/motion", Some("camera:read"), "camera-keyed"),
    ("PUT", "/api/v1/cameras/{id}/control/motion", Some("registry:manage"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/control/probe", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/detections", Some("events:read"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/frame", Some("ai:frames"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/gaps", Some("video:playback"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/onvif", Some("camera:read"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/onvif/probe", Some("registry:manage"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/playback/sessions", Some("video:playback"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/ptz/continuous", Some("registry:manage"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/ptz/goto_preset", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/ptz/presets", Some("camera:read"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/ptz/presets/refresh", Some("registry:manage"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/ptz/stop", Some("registry:manage"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/record-trigger", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/segments", Some("video:playback"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/snapshot", Some("video:playback"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/timeline", Some("video:playback"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/zone-events", Some("events:read"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/zone-events/aggregates", Some("events:read"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/zones", Some("events:read"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/zones", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/zones/occupancy", Some("events:read"), "camera-keyed"),
    ("GET", "/api/v1/entry-events", Some("events:read"), "scope-filtered"),
    ("GET", "/api/v1/entry-events/{id}", Some("events:read"), "camera-keyed"),
    ("POST", "/api/v1/entry-events/{id}/confirm", Some("gate:operate"), "camera-keyed"),
    ("POST", "/api/v1/entry-events/{id}/reject", Some("gate:operate"), "camera-keyed"),
    ("GET", "/api/v1/entry/gate", Some("identity:read"), "scope-filtered"),
    ("POST", "/api/v1/entry/gate/open/{camera_id}", Some("gate:operate"), "camera-keyed"),
    ("DELETE", "/api/v1/entry/gate/policies/{camera_id}", Some("registry:manage"), "camera-keyed"),
    ("PUT", "/api/v1/entry/gate/policies/{camera_id}", Some("registry:manage"), "camera-keyed"),
    ("PUT", "/api/v1/entry/gate/settings", Some("registry:manage"), "fleet-only"),
    ("GET", "/api/v1/events/types", Some("events:read"), "scope-neutral"),
    ("GET", "/api/v1/evidence/exports", Some("video:export"), "scope-filtered"),
    ("POST", "/api/v1/evidence/exports", Some("video:export"), "camera-keyed"),
    ("GET", "/api/v1/evidence/exports/{id}", Some("video:export"), "camera-keyed"),
    ("GET", "/api/v1/evidence/signing-key", Some("camera:read"), "scope-neutral"),
    ("GET", "/api/v1/modules/entry/ui/index.js", Some("events:read"), "scope-neutral"),
    ("GET", "/api/v1/modules/movement/ui/index.js", Some("events:read"), "scope-neutral"),
    ("GET", "/api/v1/modules/search/ui/index.js", Some("events:read"), "scope-neutral"),
    ("GET", "/api/v1/movement/breaches", Some("events:read"), "scope-filtered"),
    ("POST", "/api/v1/movement/breaches/{id}/ack", Some("gate:operate"), "camera-keyed"),
    ("POST", "/api/v1/movement/breaches/{id}/resolve", Some("gate:operate"), "camera-keyed"),
    ("GET", "/api/v1/movement/candidates", Some("events:read"), "scope-filtered"),
    ("POST", "/api/v1/movement/candidates/{id}/confirm", Some("gate:operate"), "camera-keyed"),
    ("POST", "/api/v1/movement/candidates/{id}/reject", Some("gate:operate"), "camera-keyed"),
    ("GET", "/api/v1/movement/links", Some("events:read"), "scope-filtered"),
    ("POST", "/api/v1/movement/links", Some("registry:manage"), "camera-keyed"),
    ("DELETE", "/api/v1/movement/links/{id}", Some("registry:manage"), "camera-keyed"),
    ("POST", "/api/v1/movement/run", Some("registry:manage"), "fleet-only"),
    ("GET", "/api/v1/movement/search/person", Some("events:read"), "camera-keyed"),
    ("GET", "/api/v1/movement/search/plate/{plate}", Some("events:read"), "scope-filtered"),
    ("POST", "/api/v1/onvif/discover", Some("registry:manage"), "fleet-only"),
    ("GET", "/api/v1/passes", Some("identity:read"), "scope-neutral"),
    ("POST", "/api/v1/passes", Some("gate:operate"), "fleet-only"),
    ("DELETE", "/api/v1/passes/{id}", Some("registry:manage"), "fleet-only"),
    ("GET", "/api/v1/passes/{id}", Some("identity:read"), "scope-neutral"),
    ("PATCH", "/api/v1/passes/{id}", Some("gate:operate"), "fleet-only"),
    ("POST", "/api/v1/passes/{id}/checkin", Some("gate:operate"), "fleet-only"),
    ("POST", "/api/v1/passes/{id}/checkout", Some("gate:operate"), "fleet-only"),
    ("DELETE", "/api/v1/playback/sessions/{session_id}", Some("video:playback"), "camera-keyed"),
    ("GET", "/api/v1/reports/entry-log", Some("events:read"), "scope-filtered"),
    ("GET", "/api/v1/reports/exceptions", Some("events:read"), "scope-filtered"),
    ("POST", "/api/v1/search/events", Some("events:read"), "scope-filtered"),
    ("POST", "/api/v1/search/nl", Some("events:read"), "scope-filtered"),
    ("POST", "/api/v1/search/plan", Some("events:read"), "scope-filtered"),
    ("POST", "/api/v1/search/semantic", Some("events:read"), "scope-filtered"),
    ("GET", "/api/v1/sites", Some("camera:read"), "scope-filtered"),
    ("POST", "/api/v1/sites", None, "fleet-only"),
    ("DELETE", "/api/v1/sites/{id}", None, "fleet-only"),
    ("GET", "/api/v1/sites/{id}", Some("camera:read"), "camera-keyed"),
    ("PATCH", "/api/v1/sites/{id}", None, "fleet-only"),
    ("GET", "/api/v1/system", Some("system:read"), "scope-filtered"),
    ("GET", "/api/v1/system/db", Some("system:read"), "fleet-only"),
    ("PUT", "/api/v1/system/db", None, "fleet-only"),
    ("POST", "/api/v1/system/db/convert", None, "fleet-only"),
    ("GET", "/api/v1/system/retention", Some("system:read"), "scope-neutral"),
    ("PUT", "/api/v1/system/retention", None, "fleet-only"),
    ("GET", "/api/v1/system/timezone", Some("system:read"), "scope-neutral"),
    ("PUT", "/api/v1/system/timezone", None, "fleet-only"),
    ("GET", "/api/v1/system/transcode", Some("system:read"), "scope-neutral"),
    ("PUT", "/api/v1/system/transcode", None, "fleet-only"),
    ("GET", "/api/v1/users", None, "fleet-only"),
    ("POST", "/api/v1/users", None, "fleet-only"),
    ("DELETE", "/api/v1/users/{id}", None, "fleet-only"),
    ("PATCH", "/api/v1/users/{id}", None, "fleet-only"),
    ("POST", "/api/v1/users/{id}/unlock", None, "fleet-only"),
    ("GET", "/api/v1/vehicles", Some("identity:read"), "scope-neutral"),
    ("POST", "/api/v1/vehicles", Some("registry:manage"), "fleet-only"),
    ("DELETE", "/api/v1/vehicles/{id}", Some("registry:manage"), "fleet-only"),
    ("GET", "/api/v1/vehicles/{id}", Some("identity:read"), "scope-neutral"),
    ("PATCH", "/api/v1/vehicles/{id}", Some("registry:manage"), "fleet-only"),
    ("GET", "/api/v1/watchlist", Some("identity:read"), "scope-neutral"),
    ("POST", "/api/v1/watchlist", Some("registry:manage"), "fleet-only"),
    ("DELETE", "/api/v1/watchlist/{id}", Some("registry:manage"), "fleet-only"),
    ("PATCH", "/api/v1/watchlist/{id}", Some("registry:manage"), "fleet-only"),
    ("GET", "/api/v1/webhooks", Some("events:read"), "scope-neutral"),
    ("POST", "/api/v1/webhooks", Some("registry:manage"), "fleet-only"),
    ("DELETE", "/api/v1/webhooks/{id}", Some("registry:manage"), "fleet-only"),
    ("PATCH", "/api/v1/webhooks/{id}", Some("registry:manage"), "fleet-only"),
    ("GET", "/api/v1/webhooks/{id}/deliveries", Some("events:read"), "scope-neutral"),
    ("POST", "/api/v1/webhooks/{id}/test", Some("registry:manage"), "fleet-only"),
    ("DELETE", "/api/v1/zones/{zone_id}", Some("registry:manage"), "camera-keyed"),
    ("PATCH", "/api/v1/zones/{zone_id}", Some("registry:manage"), "camera-keyed"),
];
