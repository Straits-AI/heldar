//! The access-control app owns its own schema, applied against the shared kernel pool on startup
//! (single-tenant-per-deployment). The open kernel does not define these domain tables.
//!
//! Schema evolution uses the kernel's versioned, append-only app-migration runner (each migration is
//! recorded in `_heldar_app_migrations` under the `entry` component and applied exactly once). To
//! change the schema, add a new `migrations/NNNN_*.sql` and a line to [`MIGRATIONS`] — never edit an
//! applied migration (the runner's checksum guard rejects that). `0001_init` is the original idempotent
//! `CREATE TABLE IF NOT EXISTS` blob, so a box that already ran it upgrades with no data loss.

use heldar_kernel::db::{run_app_migrations, AppMigration};
use sqlx::SqlitePool;

const MIGRATIONS: &[AppMigration] = &[
    AppMigration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    },
    AppMigration {
        version: 2,
        name: "read_contract",
        sql: include_str!("../migrations/0002_read_contract.sql"),
    },
    AppMigration {
        version: 3,
        name: "gate",
        sql: include_str!("../migrations/0003_gate.sql"),
    },
    // Reads the kernel's `media_artifacts` (kernel migration 0013), which is safe because the
    // composing server runs the kernel migrations before it calls `init` below.
    AppMigration {
        version: 4,
        name: "entry_evidence_artifacts",
        sql: include_str!("../migrations/0004_entry_evidence_artifacts.sql"),
    },
];

/// Apply the access-control migrations. Called by the composing server after the kernel migrations run.
pub async fn init(pool: &SqlitePool) -> anyhow::Result<()> {
    run_app_migrations(pool, "entry", MIGRATIONS).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use heldar_kernel::services::media_scope::{artifact_key, owners, Owners};

    async fn upgrading_box() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        heldar_kernel::db::run_migrations(&pool).await.unwrap();
        // The schema a box that has been recording for months is actually sitting on: every entry
        // migration EXCEPT the one under test. A backfill can only be exercised from before itself.
        run_app_migrations(&pool, "entry", &MIGRATIONS[..3])
            .await
            .unwrap();
        pool
    }

    async fn legacy_event(pool: &SqlitePool, id: &str, camera_id: Option<&str>, evidence: &str) {
        sqlx::query(
            "INSERT INTO entry_events (id, camera_id, event_type, timestamp, direction, subject,
                authorization, auth_status, evidence, workflow_status, workflow, audit, created_at)
             VALUES (?, ?, 'anpr', ?, 'inbound', '{}', '{}', 'matched', ?, 'pending', '{}', '{}', ?)",
        )
        .bind(id)
        .bind(camera_id)
        .bind(chrono::Utc::now())
        .bind(evidence)
        .bind(chrono::Utc::now())
        .execute(pool)
        .await
        .unwrap();
    }

    /// Kernel migration 0013 backfilled `media_artifacts` from `zone_events` and `embeddings` and
    /// missed `entry_events`, which lives in this crate. All three write a byte-identical flat frame
    /// into the same `snapshots/` directory, so on an upgraded box a camera-scoped credential got 403
    /// on its own pre-upgrade GATE evidence while the zone frame beside it returned 200 — a pure
    /// false deny, and one that reads as the fix having broken the product.
    #[tokio::test]
    async fn gate_evidence_predating_the_media_guard_stays_readable_after_upgrade() {
        let pool = upgrading_box().await;
        legacy_event(
            &pool,
            "evt_a",
            Some("cam_a"),
            r#"{"snapshot_path":"/media/snapshots/entryevt_evt_a.jpg"}"#,
        )
        .await;
        // A guard-recorded manual check-in has no lane. `media_artifacts.camera_id` is NOT NULL and
        // there is nothing honest to put there, so it must be skipped rather than guessed at.
        legacy_event(
            &pool,
            "evt_manual",
            None,
            r#"{"snapshot_path":"/media/snapshots/entryevt_evt_manual.jpg"}"#,
        )
        .await;
        // No frame was ever copied (the sampler had nothing) — nothing to attribute.
        legacy_event(&pool, "evt_dry", Some("cam_b"), r#"{"snapshot_path":null}"#).await;
        // A `detail`-shaped blob that is not JSON at all must not abort the upgrade.
        legacy_event(&pool, "evt_junk", Some("cam_c"), "not json at all").await;

        init(&pool).await.unwrap();

        // The key the backfill writes MUST be the key the guard derives from the served URL. Storing
        // the URL verbatim would write a row that is never found — the same 403, with a row to
        // explain it away.
        let url = "/media/snapshots/entryevt_evt_a.jpg";
        let key = artifact_key(url).expect("the guard derives a key from this URL");
        assert_eq!(key, "snapshots/entryevt_evt_a.jpg");
        assert_eq!(
            owners(&pool, &key).await,
            Owners::Cameras(vec!["cam_a".to_string()]),
            "pre-upgrade gate evidence must resolve to its own lane"
        );
        let kind: String =
            sqlx::query_scalar("SELECT kind FROM media_artifacts WHERE path = ? AND camera_id = ?")
                .bind(&key)
                .bind("cam_a")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(kind, "entry_evidence");

        for absent in [
            "snapshots/entryevt_evt_manual.jpg",
            "snapshots/entryevt_evt_dry.jpg",
            "snapshots/entryevt_evt_junk.jpg",
        ] {
            assert_eq!(
                owners(&pool, absent).await,
                Owners::Unattributed,
                "{absent} has no honest owner and must not be invented"
            );
        }

        // Re-running the whole list is a no-op: the checksum guard accepts 1..4 unchanged and the
        // `INSERT OR IGNORE` cannot duplicate a row on a box that reboots.
        init(&pool).await.unwrap();
        assert_eq!(
            owners(&pool, &key).await,
            Owners::Cameras(vec!["cam_a".to_string()])
        );
    }
}
