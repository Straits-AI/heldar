//! Drift guard for the `breach_alerts_read` cross-app read contract (see migrations/0002_read_contract.sql).
//!
//! Part of the read SEAM, not optional: it fails loudly if a future base-table migration renames or drops
//! a contract column WITHOUT redefining the view (SQLite late-binds views, so the SELECT fails to
//! prepare) — catching the drift in THIS crate's CI before it breaks the heldar-search consumer.

use sqlx::sqlite::SqlitePoolOptions;

/// Every column `heldar-search` (query.rs breach branch) reads from `breach_alerts_read`.
const CONTRACT_COLUMNS: &str =
    "id, created_at, camera_id, rule, subject_type, subject, zone_name, severity, evidence_path";

#[tokio::test]
async fn breach_alerts_read_view_exposes_the_cross_app_contract() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite");
    heldar_movement::schema::init(&pool)
        .await
        .expect("movement schema init (creates breach_alerts + the breach_alerts_read view)");

    let sql = format!("SELECT {CONTRACT_COLUMNS} FROM breach_alerts_read LIMIT 0");
    sqlx::query(&sql)
        .fetch_all(&pool)
        .await
        .unwrap_or_else(|e| panic!("breach_alerts_read read contract broke: {e}\n  query: {sql}"));
}
