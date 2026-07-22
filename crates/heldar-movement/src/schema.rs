//! Movement intelligence owns its schema, applied against the shared kernel pool on startup.
//!
//! Schema evolution uses the kernel's versioned, append-only app-migration runner (tracked in
//! `_heldar_app_migrations` under the `movement` component). To change the schema, add a new
//! `migrations/NNNN_*.sql` and a line to [`MIGRATIONS`] — never edit an applied migration. `0001_init`
//! is the original idempotent blob, so an existing box upgrades with no data loss.

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
        name: "appearance_score",
        sql: include_str!("../migrations/0003_appearance_score.sql"),
    },
];

pub async fn init(pool: &SqlitePool) -> anyhow::Result<()> {
    run_app_migrations(pool, "movement", MIGRATIONS).await
}
