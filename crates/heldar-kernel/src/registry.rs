//! Plugin registry catalog — types + signature verification (Phase C of the plugin platform).
//!
//! The store browses a *catalog*: a list of available plugins (distinct from [`crate::modules`], which
//! tracks what is loaded/installed). A catalog comes from two kinds of source:
//!
//! * the **bundled** first-party catalog, compiled into the binary (`include_str!`) and therefore
//!   trusted by construction — it lists only OPEN modules so the open repo names no proprietary product;
//! * optional **remote** registries (admin-configured URLs) whose documents are verified against a
//!   pinned Ed25519 public key, so a "verified publisher" badge is a real asymmetric guarantee. The
//!   proprietary shelf is served this way at runtime, never baked into open source.
//!
//! This module is pure (types + crypto, no IO); the fetch/cache/merge lives in
//! [`crate::services::registry`]. Verification is detached Ed25519 over the *exact* catalog bytes
//! (mirroring the webhook signer), so there is no JSON-canonicalization footgun.

use std::sync::OnceLock;

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::modules::{ModuleKind, MountKind, NavEntry};

/// The signed catalog document (`heldar-catalog/v1`).
#[derive(Clone, Debug, Deserialize)]
pub struct CatalogDoc {
    pub format: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub issued_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    pub entries: Vec<CatalogEntry>,
}

/// One advertised plugin. Serialized back to the dashboard (flattened) inside a [`RegistryEntryView`].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub kind: ModuleKind,
    pub summary: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// Icon key resolved by the dashboard (`moduleIcon`); unknown keys fall back to a generic glyph.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    pub install: InstallSpec,
}

/// How an entry is installed. `builtin` modules are compiled into the binary (not runtime-installable;
/// the store shows status + CTA); `sidecar` entries pre-fill the Phase B register form.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstallSpec {
    Builtin {
        /// `open` (Apache-2.0, included in the open build) or `commercial` (contact to obtain).
        #[serde(default)]
        availability: Option<String>,
        #[serde(default)]
        contact: Option<String>,
    },
    Sidecar {
        /// Container image hint (informational only — the kernel never pulls or runs it).
        #[serde(default)]
        image: Option<String>,
        /// Pre-filled, admin-editable base URL the operator points at their running sidecar.
        default_base_url: String,
        #[serde(default)]
        subscribes: Vec<String>,
        /// `viewer` | `integration` (validated by the Phase B register path).
        #[serde(default)]
        role: Option<String>,
        #[serde(default)]
        nav: Vec<NavEntry>,
        #[serde(default)]
        docs: Option<String>,
    },
}

impl InstallSpec {
    pub fn is_sidecar(&self) -> bool {
        matches!(self, InstallSpec::Sidecar { .. })
    }
}

/// The detached-signature sidecar artifact for a remote catalog (`<catalog-url>.sig`).
#[derive(Clone, Debug, Deserialize)]
pub struct SignatureDoc {
    pub alg: String,
    pub key_id: String,
    /// base64 of the raw 64-byte Ed25519 signature over the exact catalog bytes.
    pub signature: String,
    /// Optional informational hex SHA-256 of the catalog (not trusted for verification).
    #[serde(default)]
    pub catalog_sha256: Option<String>,
}

/* ------------------------------------------------------------------ */
/* Trust anchors + verification                                       */
/* ------------------------------------------------------------------ */

/// A pinned trust anchor. Only the PUBLIC key is embedded — the matching private key is held solely in
/// the publisher's release infrastructure and never enters either repo.
pub struct TrustedKey {
    pub key_id: &'static str,
    pub publisher: &'static str,
    pub first_party: bool,
    /// base64 of the 32-byte raw Ed25519 public key.
    pub ed25519_b64: &'static str,
}

/// Compile-time pinned keys. Operators add their own via `HELDAR_REGISTRY_TRUSTED_KEYS`.
///
/// NOTE: the bundled key below is the canonical Straits-AI registry signing key. Rotating it is a
/// kernel release (add a new entry with a fresh `key_id`); multiple pinned keys are all accepted, so a
/// rotation overlaps cleanly. The private half lives only in Straits-AI release infrastructure.
pub const TRUSTED_KEYS: &[TrustedKey] = &[TrustedKey {
    key_id: "straits-ai-registry-2026",
    publisher: "Straits-AI",
    first_party: true,
    ed25519_b64: "ksytnKjDmSszbYkGkf/0PighChPNhWtMqHrUUhgzEeQ=",
}];

/// A resolved key (pinned or operator-supplied) ready for verification.
#[derive(Clone)]
struct ResolvedKey {
    key_id: String,
    publisher: String,
    first_party: bool,
    pubkey: Vec<u8>,
}

/// The set of keys a verification runs against: the pinned [`TRUSTED_KEYS`] plus operator extras.
pub struct Keyset {
    keys: Vec<ResolvedKey>,
}

impl Keyset {
    /// Build from the pinned keys plus operator extras (`key_id:base64pubkey`). Malformed extras are
    /// skipped with a warning rather than failing the whole keyset.
    pub fn load(extra: &[(String, String)]) -> Self {
        let mut keys = Vec::new();
        for k in TRUSTED_KEYS {
            match base64::engine::general_purpose::STANDARD.decode(k.ed25519_b64) {
                Ok(pk) if pk.len() == 32 => keys.push(ResolvedKey {
                    key_id: k.key_id.to_string(),
                    publisher: k.publisher.to_string(),
                    first_party: k.first_party,
                    pubkey: pk,
                }),
                _ => tracing::error!(
                    key_id = k.key_id,
                    "registry: pinned key is not 32-byte base64"
                ),
            }
        }
        for (key_id, b64) in extra {
            match base64::engine::general_purpose::STANDARD.decode(b64) {
                Ok(pk) if pk.len() == 32 => keys.push(ResolvedKey {
                    key_id: key_id.clone(),
                    publisher: format!("operator:{key_id}"),
                    first_party: false,
                    pubkey: pk,
                }),
                _ => tracing::warn!(
                    key_id,
                    "registry: operator key is not 32-byte base64; skipping"
                ),
            }
        }
        Keyset { keys }
    }

    fn find(&self, key_id: &str) -> Option<&ResolvedKey> {
        self.keys.iter().find(|k| k.key_id == key_id)
    }
}

/// Outcome of verifying a catalog document.
#[derive(Clone, Debug)]
pub struct Verification {
    pub verified: bool,
    pub key_id: Option<String>,
    pub publisher: Option<String>,
    pub first_party: bool,
    /// Machine-readable reason when `!verified` (`bad_alg`/`unknown_key`/`malformed_signature`/
    /// `invalid_signature`/`expired`).
    pub reason: Option<String>,
}

impl Verification {
    fn deny(reason: &str) -> Self {
        Verification {
            verified: false,
            key_id: None,
            publisher: None,
            first_party: false,
            reason: Some(reason.to_string()),
        }
    }
}

/// Verify a detached Ed25519 signature over the exact catalog bytes. Fail-closed: any problem returns
/// `verified=false` with a reason; the caller drops an unverified remote source's entries.
pub fn verify_detached(
    catalog_bytes: &[u8],
    sig: &SignatureDoc,
    keyset: &Keyset,
    expires_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Verification {
    if sig.alg != "ed25519" {
        return Verification::deny("bad_alg");
    }
    let Some(key) = keyset.find(&sig.key_id) else {
        return Verification::deny("unknown_key");
    };
    let Ok(sig_raw) = base64::engine::general_purpose::STANDARD.decode(sig.signature.trim()) else {
        return Verification::deny("malformed_signature");
    };
    if sig_raw.len() != 64 {
        return Verification::deny("malformed_signature");
    }
    let pubkey = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &key.pubkey);
    if pubkey.verify(catalog_bytes, &sig_raw).is_err() {
        return Verification::deny("invalid_signature");
    }
    if let Some(exp) = expires_at {
        if exp < now {
            return Verification {
                verified: false,
                key_id: Some(key.key_id.clone()),
                publisher: Some(key.publisher.clone()),
                first_party: key.first_party,
                reason: Some("expired".to_string()),
            };
        }
    }
    Verification {
        verified: true,
        key_id: Some(key.key_id.clone()),
        publisher: Some(key.publisher.clone()),
        first_party: key.first_party,
        reason: None,
    }
}

/* ------------------------------------------------------------------ */
/* Bundled catalog                                                    */
/* ------------------------------------------------------------------ */

const BUNDLED_CATALOG_JSON: &str = include_str!("../catalog/heldar-catalog.json");

/// The first-party catalog compiled into the binary. Trusted by construction (it IS the binary), so
/// its entries are always `verified`. Lists only OPEN modules — the proprietary shelf is remote-only.
pub fn bundled_catalog() -> &'static CatalogDoc {
    static CELL: OnceLock<CatalogDoc> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(BUNDLED_CATALOG_JSON).expect("bundled catalog JSON is valid")
    })
}

/* ------------------------------------------------------------------ */
/* Merged view types (served at GET /api/v1/registry)                 */
/* ------------------------------------------------------------------ */

/// Which store shelf an entry belongs on.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Shelf {
    Core,
    Proprietary,
    Community,
    Import,
}

impl From<ModuleKind> for Shelf {
    fn from(k: ModuleKind) -> Self {
        match k {
            ModuleKind::Core => Shelf::Core,
            ModuleKind::Proprietary => Shelf::Proprietary,
            ModuleKind::Community => Shelf::Community,
            ModuleKind::Imported => Shelf::Import,
        }
    }
}

/// The per-entry live state, cross-referenced against loaded/installed modules.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    /// A sidecar that can be installed now (not yet registered).
    Available,
    /// A sidecar that is registered.
    Installed,
    /// A compiled module present in this build.
    Included,
    /// A compiled module advertised but not in this build (e.g. a commercial add-on).
    NotInBuild,
    /// An installed sidecar whose health probe last failed.
    Unreachable,
    /// A headless plugin (e.g. a Wasm DetectionConsumer) loaded from disk and running.
    Loaded,
}

/// One catalog entry with its computed shelf/state/verification, ready for the dashboard.
#[derive(Clone, Debug, Serialize)]
pub struct RegistryEntryView {
    #[serde(flatten)]
    pub entry: CatalogEntry,
    pub shelf: Shelf,
    pub state: EntryState,
    pub verified: bool,
    /// `bundled` or the source URL.
    pub source: String,
    /// How the module mounts (only set for entries derived from a loaded module, e.g. `headless` for a
    /// Wasm plugin); `None` for advertised catalog entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount: Option<MountKind>,
}

/// A catalog source's status (for the "registry signature" indicator + diagnostics).
#[derive(Clone, Debug, Serialize)]
pub struct RegistrySourceView {
    /// `bundled` or the URL.
    pub source: String,
    pub name: String,
    pub verified: bool,
    pub first_party: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<DateTime<Utc>>,
    pub entry_count: usize,
}

/// The full `GET /api/v1/registry` response.
#[derive(Clone, Debug, Serialize)]
pub struct RegistryView {
    pub enabled: bool,
    pub sources: Vec<RegistrySourceView>,
    pub entries: Vec<RegistryEntryView>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // A dev keypair generated for tests (independent of the pinned production key).
    // pub b64 below matches the secret used to produce the signatures in the asserts.
    fn test_keyset(pub_b64: &str) -> Keyset {
        Keyset::load(&[("test-key".to_string(), pub_b64.to_string())])
    }

    #[test]
    fn bundled_catalog_parses_and_is_open_only() {
        let cat = bundled_catalog();
        assert_eq!(cat.format, "heldar-catalog/v1");
        assert!(!cat.entries.is_empty());
        // The open bundled catalog must name no proprietary module.
        for e in &cat.entries {
            assert_ne!(
                e.kind,
                ModuleKind::Proprietary,
                "open bundle lists {}",
                e.id
            );
        }
    }

    #[test]
    fn rejects_bad_alg_and_unknown_key() {
        let ks = Keyset::load(&[]);
        let sig = SignatureDoc {
            alg: "rsa".into(),
            key_id: "straits-ai-registry-2026".into(),
            signature: "AA==".into(),
            catalog_sha256: None,
        };
        assert_eq!(
            verify_detached(b"x", &sig, &ks, None, Utc::now())
                .reason
                .as_deref(),
            Some("bad_alg")
        );
        let sig2 = SignatureDoc {
            alg: "ed25519".into(),
            key_id: "nope".into(),
            signature: "AA==".into(),
            catalog_sha256: None,
        };
        assert_eq!(
            verify_detached(b"x", &sig2, &ks, None, Utc::now())
                .reason
                .as_deref(),
            Some("unknown_key")
        );
    }

    #[test]
    fn malformed_signature_is_rejected() {
        let ks = test_keyset("ksytnKjDmSszbYkGkf/0PighChPNhWtMqHrUUhgzEeQ=");
        let sig = SignatureDoc {
            alg: "ed25519".into(),
            key_id: "test-key".into(),
            signature: "not-base64-!!!".into(),
            catalog_sha256: None,
        };
        assert_eq!(
            verify_detached(b"x", &sig, &ks, None, Utc::now())
                .reason
                .as_deref(),
            Some("malformed_signature")
        );
    }

    /// A valid signature verifies; tampering the bytes breaks it; an expired doc is denied. Uses a
    /// ring-generated ephemeral keypair so the whole sign→verify path is exercised, not just rejects.
    #[test]
    fn roundtrip_valid_tampered_expired() {
        use base64::engine::general_purpose::STANDARD as B64;
        use ring::rand::SystemRandom;
        use ring::signature::{Ed25519KeyPair, KeyPair};

        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let kp = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let pub_b64 = B64.encode(kp.public_key().as_ref());
        let ks = test_keyset(&pub_b64);

        let msg = br#"{"format":"heldar-catalog/v1","entries":[]}"#;
        let sig = kp.sign(msg);
        let sigdoc = SignatureDoc {
            alg: "ed25519".into(),
            key_id: "test-key".into(),
            signature: B64.encode(sig.as_ref()),
            catalog_sha256: None,
        };

        let now = Utc::now();
        // valid
        assert!(verify_detached(msg, &sigdoc, &ks, None, now).verified);
        // tampered bytes -> invalid_signature
        let bad = verify_detached(b"{\"format\":\"x\"}", &sigdoc, &ks, None, now);
        assert!(!bad.verified);
        assert_eq!(bad.reason.as_deref(), Some("invalid_signature"));
        // valid signature but expired doc -> denied (fail-closed on staleness)
        let past = now - chrono::Duration::hours(1);
        let exp = verify_detached(msg, &sigdoc, &ks, Some(past), now);
        assert!(!exp.verified);
        assert_eq!(exp.reason.as_deref(), Some("expired"));
    }
}
