// GENERATED FROM openapi.json BY scripts/gen_clients.py — DO NOT EDIT.
//
// Regenerate with:  cargo test -p heldar-server --test openapi_contract write_the_served_document
//                   python3 scripts/gen_clients.py target/openapi.json clients
//
// Contract version: 0.1.0

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

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
pub struct CreateSessionRequest {
    pub from: String,
    pub to: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TzSource {
    #[serde(rename = "site")]
    Site,
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "unset")]
    Unset,
}

/// What each operation requires, from the contract's own extensions.
pub const REQUIREMENTS: &[(&str, &str, Option<&str>, &str)] = &[
    ("GET", "/api/v1/cameras", Some("camera:read"), "scope-filtered"),
    ("DELETE", "/api/v1/cameras/{id}", Some("admin"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}", Some("camera:read"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/clip", Some("video:export"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/gaps", Some("video:playback"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/playback/sessions", Some("video:playback"), "camera-keyed"),
    ("POST", "/api/v1/cameras/{id}/record-trigger", Some("registry:manage"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/segments", Some("video:playback"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/snapshot", Some("video:playback"), "camera-keyed"),
    ("GET", "/api/v1/cameras/{id}/timeline", Some("video:playback"), "camera-keyed"),
    ("GET", "/api/v1/evidence/exports", Some("video:export"), "scope-filtered"),
    ("POST", "/api/v1/evidence/exports", Some("video:export"), "camera-keyed"),
    ("GET", "/api/v1/evidence/exports/{id}", Some("video:export"), "camera-keyed"),
    ("GET", "/api/v1/evidence/signing-key", Some("camera:read"), "scope-neutral"),
    ("DELETE", "/api/v1/playback/sessions/{session_id}", Some("video:playback"), "camera-keyed"),
    ("GET", "/api/v1/sites", Some("camera:read"), "scope-filtered"),
    ("POST", "/api/v1/sites", None, "fleet-only"),
    ("DELETE", "/api/v1/sites/{id}", None, "fleet-only"),
    ("GET", "/api/v1/sites/{id}", Some("camera:read"), "camera-keyed"),
    ("PATCH", "/api/v1/sites/{id}", None, "fleet-only"),
    ("GET", "/api/v1/system/timezone", Some("system:read"), "scope-neutral"),
    ("PUT", "/api/v1/system/timezone", None, "fleet-only"),
];
