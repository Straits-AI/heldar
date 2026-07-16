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
];

/// Apply the access-control migrations. Called by the composing server after the kernel migrations run.
pub async fn init(pool: &SqlitePool) -> anyhow::Result<()> {
    run_app_migrations(pool, "entry", MIGRATIONS).await
}
