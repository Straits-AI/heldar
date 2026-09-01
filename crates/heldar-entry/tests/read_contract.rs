//! Drift guard for the `entry_events_read` cross-app read contract (see migrations/0002_read_contract.sql).
//!
//! This test is part of the read SEAM, not optional: it fails loudly if a future base-table migration
//! renames or drops a contract column WITHOUT redefining the view. SQLite late-binds views, so the
//! contract SELECT below fails to prepare — catching the drift in THIS crate's CI, in the same PR that
//! changed the column, instead of at runtime in the distant heldar-movement / heldar-search consumers.

use sqlx::sqlite::SqlitePoolOptions;

/// Every column `heldar-movement` (reid.rs, breach.rs) and `heldar-search` (query.rs) read from
/// `entry_events_read`. Keep this list in lockstep with the view's projection.
const CONTRACT_COLUMNS: &str =
    "id, timestamp, camera_id, event_type, plate, subject, auth_status, evidence, direction, track_id";

#[tokio::test]
async fn entry_events_read_view_exposes_the_cross_app_contract() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite");
    // Same order the composing server boots in: kernel first, then the app crates. It is not
    // ceremony — entry migration 0004 backfills the KERNEL's `media_artifacts` with the gate evidence
    // this crate recorded before the media guard existed, so an entry schema stood up against a bare
    // database is a schema no deployment ever has.
    heldar_kernel::db::run_migrations(&pool)
        .await
        .expect("kernel migrations");
    heldar_entry::schema::init(&pool)
        .await
        .expect("entry schema init (creates entry_events + the entry_events_read view)");

    // If any contract column no longer resolves on the view, this SELECT fails to prepare → RED here.
    let sql = format!("SELECT {CONTRACT_COLUMNS} FROM entry_events_read LIMIT 0");
    sqlx::query(&sql)
        .fetch_all(&pool)
        .await
        .unwrap_or_else(|e| panic!("entry_events_read read contract broke: {e}\n  query: {sql}"));
}
