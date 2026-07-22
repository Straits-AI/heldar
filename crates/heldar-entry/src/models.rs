//! Access-control domain models (registry + canonical entry events). The RBAC/auth models
//! (User, ApiKey, …) remain in the kernel auth module for now.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::Json;
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Vehicle {
    pub id: String,
    pub plate: String,
    pub plate_norm: String,
    pub owner_name: Option<String>,
    pub owner_type: String,
    pub owner_ref: Option<String>,
    pub site_id: Option<String>,
    pub vehicle_type: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub color: Option<String>,
    pub notes: Option<String>,
    pub active: bool,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct VehicleCreate {
    pub plate: String,
    pub owner_name: Option<String>,
    pub owner_type: Option<String>,
    pub owner_ref: Option<String>,
    pub site_id: Option<String>,
    pub vehicle_type: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub color: Option<String>,
    pub notes: Option<String>,
    pub active: Option<bool>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct VehicleUpdate {
    pub plate: Option<String>,
    pub owner_name: Option<String>,
    pub owner_type: Option<String>,
    pub owner_ref: Option<String>,
    pub site_id: Option<String>,
    pub vehicle_type: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub color: Option<String>,
    pub notes: Option<String>,
    pub active: Option<bool>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct VisitorPass {
    pub id: String,
    pub code: String,
    pub visitor_name: String,
    pub phone: Option<String>,
    pub company: Option<String>,
    pub host: Option<String>,
    pub purpose: Option<String>,
    pub plate: Option<String>,
    pub plate_norm: Option<String>,
    pub vehicle_desc: Option<String>,
    pub site_id: Option<String>,
    pub valid_from: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
    pub status: String,
    pub checked_in_at: Option<DateTime<Utc>>,
    pub checked_out_at: Option<DateTime<Utc>>,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct VisitorPassCreate {
    pub visitor_name: String,
    pub phone: Option<String>,
    pub company: Option<String>,
    pub host: Option<String>,
    pub purpose: Option<String>,
    pub plate: Option<String>,
    pub vehicle_desc: Option<String>,
    pub site_id: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct VisitorPassUpdate {
    pub visitor_name: Option<String>,
    pub phone: Option<String>,
    pub company: Option<String>,
    pub host: Option<String>,
    pub purpose: Option<String>,
    pub plate: Option<String>,
    pub vehicle_desc: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Watchlist {
    pub id: String,
    pub plate: String,
    pub plate_norm: String,
    pub kind: String,
    pub reason: Option<String>,
    pub severity: String,
    pub active: bool,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct WatchlistCreate {
    pub plate: String,
    pub kind: Option<String>,
    pub reason: Option<String>,
    pub severity: Option<String>,
    pub active: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WatchlistUpdate {
    pub kind: Option<String>,
    pub reason: Option<String>,
    pub severity: Option<String>,
    pub active: Option<bool>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct EntryEvent {
    pub id: String,
    pub site_id: Option<String>,
    pub camera_id: Option<String>,
    pub event_type: String,
    pub timestamp: DateTime<Utc>,
    pub direction: String,
    pub plate: Option<String>,
    pub plate_confidence: Option<f64>,
    pub subject: Json<Value>,
    pub authorization: Json<Value>,
    pub auth_status: String,
    pub evidence: Json<Value>,
    pub workflow_status: String,
    pub workflow: Json<Value>,
    pub audit: Json<Value>,
    pub track_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AuditLog {
    pub id: String,
    pub actor: String,
    pub actor_name: Option<String>,
    pub role: Option<String>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub detail: Json<Value>,
    pub created_at: DateTime<Utc>,
}
