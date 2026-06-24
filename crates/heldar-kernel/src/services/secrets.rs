//! Optional encryption-at-rest for sensitive fields (currently camera credentials), keyed by
//! `HELDAR_SECRET_KEY`.
//!
//! - **No key configured** (the LAN-appliance default, and the open-source default): values are stored
//!   and served as plaintext — behaviour is unchanged.
//! - **Key configured** (production): new writes are AES-256-GCM sealed (`enc:v1:` + base64 of
//!   `nonce ‖ ciphertext+tag`); reads transparently decrypt both sealed and legacy-plaintext values.
//!
//! The key is process-global immutable config: [`init_key`] is called once at startup (before any
//! camera URL is built), and the `camera_url` builder reads it via [`decrypt_stored`]. A sealed value
//! encountered with no/wrong key is a hard error — the kernel never feeds ciphertext to ffmpeg.

use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use rand_core::{OsRng, RngCore};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};

/// Storage marker for a sealed value. Anything without this prefix is treated as legacy plaintext.
const PREFIX: &str = "enc:v1:";

/// Process-wide encryption key (None = encryption disabled). Set once at startup via [`init_key`].
static KEY: OnceLock<Option<[u8; 32]>> = OnceLock::new();

/// Decode + validate `HELDAR_SECRET_KEY` (base64 of 32 bytes) and install it process-wide. Call once
/// at startup. `None`/empty disables encryption (plaintext at rest). Errors on a malformed key so a
/// misconfigured master key fails loud at boot rather than silently disabling encryption.
pub fn init_key(secret_key_b64: Option<&str>) -> Result<()> {
    let key = match secret_key_b64.map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(b64) => {
            let bytes = B64
                .decode(b64)
                .context("HELDAR_SECRET_KEY must be valid base64")?;
            let key: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                anyhow!(
                    "HELDAR_SECRET_KEY must decode to 32 bytes (got {})",
                    bytes.len()
                )
            })?;
            Some(key)
        }
    };
    // First set wins; ignore a redundant re-init with the same intent (e.g. tests).
    let _ = KEY.set(key);
    Ok(())
}

fn process_key() -> Option<&'static [u8; 32]> {
    KEY.get().and_then(|k| k.as_ref())
}

/// Whether encryption-at-rest is active for this process.
pub fn enabled() -> bool {
    process_key().is_some()
}

/// Is this stored value already sealed?
pub fn is_encrypted(stored: &str) -> bool {
    stored.starts_with(PREFIX)
}

/// Seal `plaintext` for storage using the process key (plaintext passthrough when no key is set).
pub fn encrypt_for_storage(plaintext: &str) -> Result<String> {
    encrypt(process_key(), plaintext)
}

/// Decrypt a stored value using the process key (legacy-plaintext passthrough; sealed-without-key errors).
pub fn decrypt_stored(stored: &str) -> Result<String> {
    decrypt(process_key(), stored)
}

/// Seal `plaintext` with an explicit key. `None` returns the plaintext unchanged.
pub fn encrypt(key: Option<&[u8; 32]>, plaintext: &str) -> Result<String> {
    let Some(key) = key else {
        return Ok(plaintext.to_string());
    };
    let sealing = LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, key).map_err(|_| anyhow!("invalid AES-256 key"))?,
    );
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let mut in_out = plaintext.as_bytes().to_vec();
    sealing
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut in_out,
        )
        .map_err(|_| anyhow!("seal failed"))?;
    let mut blob = Vec::with_capacity(NONCE_LEN + in_out.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&in_out);
    Ok(format!("{PREFIX}{}", B64.encode(blob)))
}

/// Decrypt a stored value with an explicit key. A value without the `enc:v1:` prefix is returned as-is
/// (legacy plaintext). A sealed value with `None`/wrong key is an error (never serve ciphertext).
pub fn decrypt(key: Option<&[u8; 32]>, stored: &str) -> Result<String> {
    let Some(rest) = stored.strip_prefix(PREFIX) else {
        return Ok(stored.to_string()); // legacy plaintext
    };
    let key = key
        .ok_or_else(|| anyhow!("an encrypted secret is stored but HELDAR_SECRET_KEY is not set"))?;
    let blob = B64
        .decode(rest)
        .context("malformed encrypted secret (base64)")?;
    if blob.len() <= NONCE_LEN {
        return Err(anyhow!("encrypted secret too short"));
    }
    let (nonce, ct) = blob.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().expect("checked length");
    let opening = LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, key).map_err(|_| anyhow!("invalid AES-256 key"))?,
    );
    let mut buf = ct.to_vec();
    let plain = opening
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf)
        .map_err(|_| anyhow!("decrypt failed (wrong key or corrupt secret)"))?;
    String::from_utf8(plain.to_vec()).context("decrypted secret is not valid UTF-8")
}

/// One-time migration: when a key is configured, seal any legacy-plaintext camera passwords. Idempotent
/// (skips already-sealed rows). Returns how many rows were re-encrypted. No-op when no key is set.
pub async fn reencrypt_camera_passwords(pool: &sqlx::SqlitePool) -> Result<usize> {
    if !enabled() {
        return Ok(0);
    }
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, password FROM cameras WHERE password IS NOT NULL AND password != ''",
    )
    .fetch_all(pool)
    .await?;
    let mut n = 0usize;
    for (id, pw) in rows {
        if is_encrypted(&pw) {
            continue;
        }
        let sealed = encrypt_for_storage(&pw)?;
        sqlx::query("UPDATE cameras SET password = ? WHERE id = ?")
            .bind(&sealed)
            .bind(&id)
            .execute(pool)
            .await?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = i as u8;
        }
        k
    }

    #[test]
    fn round_trip_with_key() {
        let k = key();
        let sealed = encrypt(Some(&k), "SohHikVision").unwrap();
        assert!(
            sealed.starts_with(PREFIX),
            "sealed value carries the marker"
        );
        assert!(
            !sealed.contains("SohHikVision"),
            "plaintext must not appear"
        );
        assert_eq!(decrypt(Some(&k), &sealed).unwrap(), "SohHikVision");
    }

    #[test]
    fn no_key_is_plaintext_passthrough() {
        // Encrypt + decrypt are identity when no key is configured (LAN appliance / open default).
        assert_eq!(encrypt(None, "secret").unwrap(), "secret");
        assert_eq!(decrypt(None, "secret").unwrap(), "secret");
    }

    #[test]
    fn legacy_plaintext_reads_through_even_with_key() {
        // A pre-encryption row (no enc:v1: prefix) still reads, so enabling a key doesn't break
        // existing cameras before the re-encrypt pass runs.
        assert_eq!(
            decrypt(Some(&key()), "legacy-plain").unwrap(),
            "legacy-plain"
        );
    }

    #[test]
    fn sealed_without_key_errors() {
        let sealed = encrypt(Some(&key()), "secret").unwrap();
        assert!(
            decrypt(None, &sealed).is_err(),
            "must not silently return ciphertext"
        );
    }

    #[test]
    fn wrong_key_errors() {
        let sealed = encrypt(Some(&key()), "secret").unwrap();
        let mut wrong = key();
        wrong[0] ^= 0xff;
        assert!(decrypt(Some(&wrong), &sealed).is_err());
    }

    #[test]
    fn nonce_is_random_per_call() {
        let k = key();
        assert_ne!(
            encrypt(Some(&k), "x").unwrap(),
            encrypt(Some(&k), "x").unwrap(),
            "fresh nonce per encryption"
        );
    }
}
