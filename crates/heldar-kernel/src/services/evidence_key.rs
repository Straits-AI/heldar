//! The appliance's evidence-signing key (#118).
//!
//! An Ed25519 key pair, generated at first use and stored as PKCS#8 under the data directory. It
//! signs evidence bundle manifests and NOTHING else.
//!
//! WHY A DEDICATED KEY, AND NOT `HELDAR_SECRET_KEY`.
//!
//! `HELDAR_SECRET_KEY` encrypts camera credentials at rest. It is a symmetric key that every process
//! able to read a camera URL must also hold — so anyone who can decrypt a credential could forge an
//! evidence signature with it, and possession of it proves nothing to a third party. An evidence
//! signature has the opposite shape: the private half never leaves the appliance, the public half is
//! published, and the whole point is that someone who does NOT trust the appliance's operator can
//! still check it. Those are different keys because they answer different questions.
//!
//! WHAT THIS SIGNATURE DOES AND DOES NOT ESTABLISH.
//!
//! It establishes that a bundle was produced by the appliance holding this key and has not been
//! altered since. It does NOT establish when it was produced — the appliance stamps its own clock,
//! and an appliance that lies about its clock signs the lie faithfully. It is not a trusted
//! timestamp, and the manifest says so in its own words rather than leaving a reader to assume.
//!
//! The key file is written 0600. On a box where an attacker can read the data directory, they can
//! sign bundles — this raises the bar from "anyone with a hex editor" to "root on the recorder",
//! which is the honest description of what an appliance-held key buys.
//!
//! #126 added the secret-source chain (env / `NAME_FILE` / systemd credential) for the DEPLOYMENT
//! secrets. This key is not one of them: it is generated on the box and never supplied by an
//! operator, so there is nothing to resolve. Loading it from an HSM or external provider remains
//! the open half of #126, and the loader is still deliberately one function so that swap is one
//! function.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use sha2::{Digest, Sha256};

/// Filename under the data directory. Not configurable: a second key path is a second key, and an
/// operator who moves it silently invalidates every bundle already handed out.
const KEY_FILE: &str = "evidence-signing-key.pkcs8";

/// The loaded signing key plus its published identity.
pub struct EvidenceKey {
    pair: Ed25519KeyPair,
    /// Raw 32-byte Ed25519 public key, base64.
    pub public_key_b64: String,
    /// `sha256:<hex>` over the raw public key. Short, stable, and what a verifier pins.
    pub key_id: String,
}

impl EvidenceKey {
    /// Load the appliance's key, generating one on first use.
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        let path = key_path(data_dir);
        let pkcs8 = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => generate(&path)?,
            Err(e) => return Err(e).context(format!("reading {}", path.display())),
        };
        Self::from_pkcs8(&pkcs8)
            // A corrupt key file is NOT a reason to quietly mint a new one. Doing that would
            // invalidate every bundle already exported under the old key while reporting success,
            // and an investigator holding one would be told the signature is from an unknown key.
            .with_context(|| format!("{} is not a usable Ed25519 key", path.display()))
    }

    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self> {
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8)
            .map_err(|e| anyhow::anyhow!("malformed PKCS#8 Ed25519 key: {e}"))?;
        let public = pair.public_key().as_ref().to_vec();
        Ok(Self {
            public_key_b64: B64.encode(&public),
            key_id: format!("sha256:{:x}", Sha256::digest(&public)),
            pair,
        })
    }

    /// Sign the canonical manifest bytes.
    pub fn sign(&self, msg: &[u8]) -> String {
        B64.encode(self.pair.sign(msg).as_ref())
    }
}

fn key_path(data_dir: &Path) -> PathBuf {
    data_dir.join(KEY_FILE)
}

/// Generate a key and write it 0600, failing if one appeared in the meantime.
fn generate(path: &Path) -> Result<Vec<u8>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let rng = SystemRandom::new();
    let doc = Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|e| anyhow::anyhow!("generating an Ed25519 key: {e}"))?;
    let bytes = doc.as_ref().to_vec();

    // create_new so two workers racing at first boot cannot each generate a key and have the loser
    // silently overwrite the winner's — bundles signed in between would verify against a key the
    // appliance no longer holds.
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    match opts.open(path) {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(&bytes)
                .with_context(|| format!("writing {}", path.display()))?;
            tracing::info!(
                target: "heldar::security",
                path = %path.display(),
                "evidence: generated the appliance's evidence-signing key"
            );
            Ok(bytes)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::read(path).with_context(|| format!("reading {}", path.display()))
        }
        Err(e) => Err(e).context(format!("creating {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("heldar-evkey-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_key_is_stable_across_loads() {
        let dir = tmp("stable");
        let a = EvidenceKey::load_or_create(&dir).unwrap();
        let b = EvidenceKey::load_or_create(&dir).unwrap();
        assert_eq!(
            a.key_id, b.key_id,
            "a second load must reuse the stored key — regenerating it would invalidate every \
             bundle already exported, and an investigator would be told the key is unknown"
        );
        assert!(a.key_id.starts_with("sha256:"));
    }

    /// A signature over the exact bytes verifies; one byte of drift does not. This is the property
    /// the whole bundle format rests on, so it is asserted here rather than assumed from `ring`.
    #[test]
    fn it_signs_bytes_that_verify_and_refuses_altered_ones() {
        use ring::signature::{UnparsedPublicKey, ED25519};
        let dir = tmp("roundtrip");
        let k = EvidenceKey::load_or_create(&dir).unwrap();
        let msg = br#"{"format":"heldar-evidence/1"}"#;
        let sig = B64.decode(k.sign(msg)).unwrap();
        let pubkey = B64.decode(&k.public_key_b64).unwrap();
        let v = UnparsedPublicKey::new(&ED25519, &pubkey);
        assert!(v.verify(msg, &sig).is_ok(), "the signature must verify");
        assert!(
            v.verify(br#"{"format":"heldar-evidence/2"}"#, &sig)
                .is_err(),
            "an altered manifest must NOT verify"
        );
    }

    #[test]
    fn a_corrupt_key_file_is_an_error_not_a_silent_new_key() {
        let dir = tmp("corrupt");
        std::fs::write(key_path(&dir), b"not a pkcs8 key").unwrap();
        let err = match EvidenceKey::load_or_create(&dir) {
            Ok(_) => panic!("a corrupt key file must be refused, not replaced with a fresh key"),
            Err(e) => e,
        };
        assert!(
            format!("{err:#}").contains("not a usable Ed25519 key"),
            "got: {err:#}"
        );
    }
}
