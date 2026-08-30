//! One-shot operational maintenance the composing binary exposes as CLI subcommands: an online
//! snapshot of the metadata DB and rotation of the camera-credential encryption key. Both are pure
//! batch jobs against the SQLite store (no server, no background tasks).

use anyhow::{Context, Result};
use sqlx::SqlitePool;

use crate::services::secrets;

/// Online, consistent snapshot of the SQLite metadata DB via `VACUUM INTO`. Safe to run against a
/// live box: SQLite takes a read transaction and writes a fully-consistent, defragmented copy without
/// blocking writers (unlike a naive `cp`, which can capture a torn file mid-write). Refuses to
/// overwrite an existing file. Returns the snapshot size in bytes.
pub async fn backup_db(pool: &SqlitePool, dest: &str) -> Result<u64> {
    if std::path::Path::new(dest).exists() {
        anyhow::bail!(
            "destination '{dest}' already exists — choose a fresh path (VACUUM INTO will not overwrite)"
        );
    }
    // `dest` is an operator-supplied local path. VACUUM INTO takes a string-literal filename (it is
    // not a bindable DML statement), so embed the path with single quotes escaped.
    let escaped = dest.replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{escaped}'"))
        .execute(pool)
        .await
        .with_context(|| format!("VACUUM INTO {dest}"))?;
    Ok(std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0))
}

/// Re-seal every stored camera credential from `old_key` to `new_key`. A row already sealed under the
/// new key is skipped (so the job is safely re-runnable); a legacy-plaintext row is sealed under the
/// new key. A row that decrypts under neither key is a hard error — we never write back garbage.
/// Returns how many rows were rotated.
pub async fn rekey_camera_secrets(
    pool: &SqlitePool,
    old_key: &[u8; 32],
    new_key: &[u8; 32],
) -> Result<usize> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, password FROM cameras WHERE password IS NOT NULL AND password != ''",
    )
    .fetch_all(pool)
    .await?;
    let mut rekeyed = 0usize;
    for (id, stored) in rows {
        // Already sealed under the new key? leave it (idempotent re-runs after a partial rotation).
        if secrets::is_encrypted(&stored) && secrets::decrypt(Some(new_key), &stored).is_ok() {
            continue;
        }
        let plain = secrets::decrypt(Some(old_key), &stored).with_context(|| {
            format!("decrypt camera {id} with the old key (HELDAR_SECRET_KEY_OLD)")
        })?;
        let sealed = secrets::encrypt(Some(new_key), &plain)?;
        sqlx::query("UPDATE cameras SET password = ? WHERE id = ?")
            .bind(&sealed)
            .bind(&id)
            .execute(pool)
            .await?;
        rekeyed += 1;
    }
    Ok(rekeyed)
}

async fn rekey_camera_secrets_in(
    conn: &mut sqlx::SqliteConnection,
    old_key: &[u8; 32],
    new_key: &[u8; 32],
) -> Result<usize> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, password FROM cameras WHERE password IS NOT NULL AND password != ''",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut rekeyed = 0usize;
    for (id, stored) in rows {
        // Already sealed under the new key? leave it (idempotent re-runs after a partial rotation).
        if secrets::is_encrypted(&stored) && secrets::decrypt(Some(new_key), &stored).is_ok() {
            continue;
        }
        let plain = secrets::decrypt(Some(old_key), &stored).with_context(|| {
            format!("decrypt camera {id} with the old key (HELDAR_SECRET_KEY_OLD)")
        })?;
        let sealed = secrets::encrypt(Some(new_key), &plain)?;
        sqlx::query("UPDATE cameras SET password = ? WHERE id = ?")
            .bind(&sealed)
            .bind(&id)
            .execute(&mut *conn)
            .await?;
        rekeyed += 1;
    }
    Ok(rekeyed)
}

/// Rotate EVERY sealed secret in one transaction (#126).
///
/// The two functions below each loop row by row against the pool, so a failure part-way through —
/// an undecryptable row, a killed process, a full disk — left some rows under the new key and some
/// under the old. A mixed database is not a partial success: `decrypt_stored` is a hard error by
/// design (the kernel never feeds ciphertext to ffmpeg), so every camera on the wrong side of the
/// split stops recording until someone notices and re-runs the rotation.
///
/// One transaction makes the outcome binary: either every secret is under the new key, or the
/// database is exactly as it was. The issue's requirement is "no mixed-key database is left
/// behind", and that is a property of the commit boundary, not of the loop.
///
/// SQLite gives this cheaply — one writer, and a rotation touches at most a few hundred rows.
pub async fn rekey_all(
    pool: &SqlitePool,
    old_key: &[u8; 32],
    new_key: &[u8; 32],
) -> Result<(usize, usize)> {
    let mut tx = pool.begin().await?;
    let cameras = rekey_camera_secrets_in(&mut tx, old_key, new_key).await?;
    let stored = rekey_stored_credentials_in(&mut tx, old_key, new_key).await?;
    tx.commit().await?;
    Ok((cameras, stored))
}

/// Re-seal webhook signing secrets and backup destination credentials from `old_key` to `new_key`.
///
/// The counterpart of [`rekey_camera_secrets`] for the other secret-bearing fields. A key rotation
/// that covered only camera passwords would leave these sealed under a key the operator has just
/// retired — every webhook would deliver unsigned and every backup destination would fail to
/// authenticate, with nothing saying why. Idempotent: values already readable with the new key are
/// skipped, so a re-run after a partial rotation is safe.
pub async fn rekey_stored_credentials(
    pool: &SqlitePool,
    old_key: &[u8; 32],
    new_key: &[u8; 32],
) -> Result<usize> {
    let mut rekeyed = 0usize;

    let hooks: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, secret FROM webhook_subscriptions WHERE secret IS NOT NULL AND secret != ''",
    )
    .fetch_all(pool)
    .await?;
    for (id, stored) in hooks {
        if secrets::is_encrypted(&stored) && secrets::decrypt(Some(new_key), &stored).is_ok() {
            continue;
        }
        let plain = secrets::decrypt(Some(old_key), &stored).with_context(|| {
            format!("decrypt webhook {id} secret with the old key (HELDAR_SECRET_KEY_OLD)")
        })?;
        let sealed = secrets::encrypt(Some(new_key), &plain)?;
        sqlx::query("UPDATE webhook_subscriptions SET secret = ? WHERE id = ?")
            .bind(&sealed)
            .bind(&id)
            .execute(pool)
            .await?;
        rekeyed += 1;
    }

    let dests: Vec<(String, String)> = sqlx::query_as("SELECT id, config FROM backup_destinations")
        .fetch_all(pool)
        .await?;
    for (id, raw) in dests {
        let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let mut changed = false;
        if let Some(obj) = cfg.as_object_mut() {
            for key in crate::models::BACKUP_SECRET_KEYS {
                let Some(stored) = obj.get(*key).and_then(|v| v.as_str()).map(str::to_string)
                else {
                    continue;
                };
                if stored.is_empty()
                    || (secrets::is_encrypted(&stored)
                        && secrets::decrypt(Some(new_key), &stored).is_ok())
                {
                    continue;
                }
                let plain = secrets::decrypt(Some(old_key), &stored).with_context(|| {
                    format!("decrypt backup destination {id} `{key}` with the old key")
                })?;
                obj.insert(
                    (*key).to_string(),
                    serde_json::Value::String(secrets::encrypt(Some(new_key), &plain)?),
                );
                changed = true;
            }
        }
        if changed {
            sqlx::query("UPDATE backup_destinations SET config = ? WHERE id = ?")
                .bind(serde_json::to_string(&cfg)?)
                .bind(&id)
                .execute(pool)
                .await?;
            rekeyed += 1;
        }
    }
    Ok(rekeyed)
}

async fn rekey_stored_credentials_in(
    conn: &mut sqlx::SqliteConnection,
    old_key: &[u8; 32],
    new_key: &[u8; 32],
) -> Result<usize> {
    let mut rekeyed = 0usize;

    let hooks: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, secret FROM webhook_subscriptions WHERE secret IS NOT NULL AND secret != ''",
    )
    .fetch_all(&mut *conn)
    .await?;
    for (id, stored) in hooks {
        if secrets::is_encrypted(&stored) && secrets::decrypt(Some(new_key), &stored).is_ok() {
            continue;
        }
        let plain = secrets::decrypt(Some(old_key), &stored).with_context(|| {
            format!("decrypt webhook {id} secret with the old key (HELDAR_SECRET_KEY_OLD)")
        })?;
        let sealed = secrets::encrypt(Some(new_key), &plain)?;
        sqlx::query("UPDATE webhook_subscriptions SET secret = ? WHERE id = ?")
            .bind(&sealed)
            .bind(&id)
            .execute(&mut *conn)
            .await?;
        rekeyed += 1;
    }

    let dests: Vec<(String, String)> = sqlx::query_as("SELECT id, config FROM backup_destinations")
        .fetch_all(&mut *conn)
        .await?;
    for (id, raw) in dests {
        let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let mut changed = false;
        if let Some(obj) = cfg.as_object_mut() {
            for key in crate::models::BACKUP_SECRET_KEYS {
                let Some(stored) = obj.get(*key).and_then(|v| v.as_str()).map(str::to_string)
                else {
                    continue;
                };
                if stored.is_empty()
                    || (secrets::is_encrypted(&stored)
                        && secrets::decrypt(Some(new_key), &stored).is_ok())
                {
                    continue;
                }
                let plain = secrets::decrypt(Some(old_key), &stored).with_context(|| {
                    format!("decrypt backup destination {id} `{key}` with the old key")
                })?;
                obj.insert(
                    (*key).to_string(),
                    serde_json::Value::String(secrets::encrypt(Some(new_key), &plain)?),
                );
                changed = true;
            }
        }
        if changed {
            sqlx::query("UPDATE backup_destinations SET config = ? WHERE id = ?")
                .bind(serde_json::to_string(&cfg)?)
                .bind(&id)
                .execute(&mut *conn)
                .await?;
            rekeyed += 1;
        }
    }
    Ok(rekeyed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::secrets;

    fn key(seed: u8) -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = seed ^ (i as u8);
        }
        k
    }

    async fn mem_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    async fn insert_cam(pool: &SqlitePool, id: &str, password: &str) {
        let now = chrono::Utc::now();
        sqlx::query(
            "INSERT INTO cameras (id, name, password, retention_hours, created_at, updated_at)
             VALUES (?, ?, ?, 24, ?, ?)",
        )
        .bind(id)
        .bind(id)
        .bind(password)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn stored_pw(pool: &SqlitePool, id: &str) -> String {
        sqlx::query_scalar("SELECT password FROM cameras WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn rekey_rotates_old_seals_plaintext_and_skips_already_new() {
        let old = key(1);
        let new = key(2);
        let pool = mem_pool().await;
        insert_cam(
            &pool,
            "old",
            &secrets::encrypt(Some(&old), "pw-old").unwrap(),
        )
        .await;
        insert_cam(&pool, "plain", "pw-plain").await; // legacy plaintext (no enc:v1: prefix)
        insert_cam(
            &pool,
            "new",
            &secrets::encrypt(Some(&new), "pw-new").unwrap(),
        )
        .await;

        let n = rekey_camera_secrets(&pool, &old, &new).await.unwrap();
        assert_eq!(
            n, 2,
            "old-sealed + plaintext rotated; already-new is skipped"
        );

        // Every row now decrypts under the NEW key to its original plaintext.
        assert_eq!(
            secrets::decrypt(Some(&new), &stored_pw(&pool, "old").await).unwrap(),
            "pw-old"
        );
        assert_eq!(
            secrets::decrypt(Some(&new), &stored_pw(&pool, "plain").await).unwrap(),
            "pw-plain"
        );
        assert_eq!(
            secrets::decrypt(Some(&new), &stored_pw(&pool, "new").await).unwrap(),
            "pw-new"
        );
        // The formerly-plaintext row is now sealed (not stored in the clear).
        assert!(secrets::is_encrypted(&stored_pw(&pool, "plain").await));

        // Re-running is idempotent — nothing is left to rotate.
        assert_eq!(rekey_camera_secrets(&pool, &old, &new).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn rekey_errors_on_row_decryptable_under_neither_key() {
        let old = key(1);
        let new = key(2);
        let third = key(9);
        let pool = mem_pool().await;
        insert_cam(
            &pool,
            "alien",
            &secrets::encrypt(Some(&third), "pw").unwrap(),
        )
        .await;
        // Sealed under a key we hold neither of -> hard error, never write back garbage.
        assert!(rekey_camera_secrets(&pool, &old, &new).await.is_err());
    }
}

#[cfg(test)]
mod rotation_tests {
    use super::*;

    async fn pool_with(cams: &[(&str, &str)], key: &[u8; 32]) -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let now = chrono::Utc::now();
        for (id, password) in cams {
            let sealed = secrets::encrypt(Some(key), password).unwrap();
            sqlx::query(
                "INSERT INTO cameras (id, name, password, created_at, updated_at) VALUES (?,?,?,?,?)",
            )
            .bind(id)
            .bind(id)
            .bind(&sealed)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }
        pool
    }

    async fn stored(pool: &SqlitePool, id: &str) -> String {
        sqlx::query_scalar::<_, String>("SELECT password FROM cameras WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn rotation_moves_every_secret_to_the_new_key() {
        let old = [1u8; 32];
        let new = [2u8; 32];
        let pool = pool_with(&[("cam_a", "pw-a"), ("cam_b", "pw-b")], &old).await;

        let (cameras, _) = rekey_all(&pool, &old, &new).await.unwrap();
        assert_eq!(cameras, 2);
        for (id, pw) in [("cam_a", "pw-a"), ("cam_b", "pw-b")] {
            let s = stored(&pool, id).await;
            assert_eq!(secrets::decrypt(Some(&new), &s).unwrap(), pw);
            assert!(
                secrets::decrypt(Some(&old), &s).is_err(),
                "{id} must no longer open with the old key"
            );
        }
    }

    /// THE POINT OF THE TRANSACTION. A failure part-way through must leave the database exactly as
    /// it was, not half-rotated.
    ///
    /// A mixed database is not a partial success. `decrypt_stored` is a hard error by design — the
    /// kernel never feeds ciphertext to ffmpeg — so every camera on the wrong side of the split
    /// stops recording, and the operator sees a rotation that "failed" while half their fleet has
    /// silently gone dark.
    ///
    /// The failure here is a row sealed under a THIRD key, which is what a half-finished earlier
    /// rotation or a hand-edited row looks like.
    #[tokio::test]
    async fn a_failure_part_way_through_leaves_no_mixed_key_database() {
        let old = [1u8; 32];
        let new = [2u8; 32];
        let alien = [9u8; 32];
        // Ordered so a good row is processed BEFORE the bad one — otherwise the loop fails first
        // and there is nothing committed to roll back, and the test would pass vacuously.
        let pool = pool_with(&[("cam_a", "pw-a")], &old).await;
        let sealed_alien = secrets::encrypt(Some(&alien), "pw-z").unwrap();
        let now = chrono::Utc::now();
        sqlx::query("INSERT INTO cameras (id, name, password, created_at, updated_at) VALUES ('cam_z','z',?,?,?)")
            .bind(&sealed_alien)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();

        let before_a = stored(&pool, "cam_a").await;
        let err = rekey_all(&pool, &old, &new).await;
        assert!(err.is_err(), "an undecryptable row must fail the rotation");

        let after_a = stored(&pool, "cam_a").await;
        assert_eq!(
            before_a, after_a,
            "cam_a must be byte-identical to before. If it moved to the new key while cam_z stayed \
             behind, the database is mixed and cam_a stops recording under the old key the server \
             is still running with."
        );
        assert_eq!(
            secrets::decrypt(Some(&old), &after_a).unwrap(),
            "pw-a",
            "and it must still open with the key the server actually holds"
        );
    }

    /// Re-running after a successful rotation is a no-op, so an operator who is unsure can just run
    /// it again rather than reasoning about what state the box is in.
    #[tokio::test]
    async fn rotation_is_idempotent() {
        let old = [1u8; 32];
        let new = [2u8; 32];
        let pool = pool_with(&[("cam_a", "pw-a")], &old).await;
        assert_eq!(rekey_all(&pool, &old, &new).await.unwrap().0, 1);
        assert_eq!(
            rekey_all(&pool, &old, &new).await.unwrap().0,
            0,
            "a second run must re-seal nothing"
        );
        assert_eq!(
            secrets::decrypt(Some(&new), &stored(&pool, "cam_a").await).unwrap(),
            "pw-a"
        );
    }
}
