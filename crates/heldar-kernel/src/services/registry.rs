//! Plugin registry service (Phase C): fetch + verify + cache remote catalogs, and merge the bundled +
//! remote catalogs with live module state into the store view.
//!
//! Remote catalogs are fetched with a dedicated SSRF-guarded client (no redirects, scheme + literal-IP
//! checks, size cap, timeout) and Ed25519-verified against the pinned + operator keys. An unverified
//! source contributes ZERO entries unless `registry_allow_unverified` is set. Everything is cached
//! in-memory so the store renders offline (and instantly) between refreshes; the bundled catalog is
//! always present. The merge cross-references [`AppState::modules`] (compiled) and the
//! `module_registrations` table (installed sidecars) to compute each entry's shelf + state.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::RwLock;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::config::Config;
use crate::modules::{ModuleManifest, ModuleRegistration, MountKind};
use crate::registry::{
    bundled_catalog, verify_detached, CatalogDoc, CatalogEntry, EntryState, InstallSpec, Keyset,
    RegistryEntryView, RegistrySourceView, RegistryView, Shelf, SignatureDoc,
};

/// Hard cap on a fetched catalog/signature body (defense-in-depth against a hostile/huge response).
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// One remote source's last-known state (kept in memory so the store renders offline).
struct CachedSource {
    url: String,
    name: String,
    verified: bool,
    first_party: bool,
    key_id: Option<String>,
    error: Option<String>,
    fetched_at: Option<DateTime<Utc>>,
    entries: Vec<CatalogEntry>,
}

/// The plugin store's catalog engine, held in [`crate::state::AppState`].
pub struct CatalogService {
    enabled: bool,
    urls: Vec<String>,
    refresh_s: u64,
    allow_unverified: bool,
    allow_private: bool,
    keyset: Keyset,
    client: reqwest::Client,
    remote: RwLock<Vec<CachedSource>>,
}

impl CatalogService {
    pub fn new(cfg: &Config) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.registry_fetch_timeout_s.max(1)))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        let remote = cfg
            .registry_urls
            .iter()
            .map(|u| CachedSource {
                url: u.clone(),
                name: u.clone(),
                verified: false,
                first_party: false,
                key_id: None,
                error: Some("not yet fetched".into()),
                fetched_at: None,
                entries: Vec::new(),
            })
            .collect();
        Self {
            enabled: cfg.registry_enabled,
            urls: cfg.registry_urls.clone(),
            refresh_s: cfg.registry_refresh_s.max(30),
            allow_unverified: cfg.registry_allow_unverified,
            allow_private: cfg.registry_allow_private,
            keyset: Keyset::load(&cfg.registry_trusted_keys),
            client,
            remote: RwLock::new(remote),
        }
    }

    /// Re-fetch + verify every configured remote registry, updating the in-memory cache. Best-effort:
    /// a failing source keeps its prior cached entries and records the error.
    pub async fn refresh(&self) {
        if !self.enabled {
            return;
        }
        for url in &self.urls {
            let result = self.fetch_one(url).await;
            let mut guard = self.remote.write().unwrap();
            if let Some(slot) = guard.iter_mut().find(|s| &s.url == url) {
                match result {
                    Ok(fresh) => *slot = fresh,
                    Err(e) => {
                        slot.error = Some(e);
                        slot.fetched_at = Some(Utc::now());
                        // keep last-good entries + verified flag for offline resilience
                    }
                }
            }
        }
    }

    async fn fetch_one(&self, url: &str) -> Result<CachedSource, String> {
        validate_registry_url(url, self.allow_private)?;
        let raw = self.get_capped(url).await?;
        let sig_raw = self.get_capped(&format!("{url}.sig")).await.ok();

        let doc: CatalogDoc =
            serde_json::from_slice(&raw).map_err(|e| format!("catalog parse: {e}"))?;
        if doc.format != "heldar-catalog/v1" {
            return Err(format!("unsupported catalog format `{}`", doc.format));
        }

        // Verify the detached signature over the EXACT bytes. No .sig => unverified.
        let verification = match sig_raw
            .as_deref()
            .and_then(|b| serde_json::from_slice::<SignatureDoc>(b).ok())
        {
            Some(sig) => verify_detached(&raw, &sig, &self.keyset, doc.expires_at, Utc::now()),
            None => crate::registry::Verification {
                verified: false,
                key_id: None,
                publisher: None,
                first_party: false,
                reason: Some("no_signature".into()),
            },
        };

        // Fail-closed: drop entries from an unverified source unless explicitly allowed.
        let entries = if verification.verified || self.allow_unverified {
            doc.entries
        } else {
            Vec::new()
        };
        let error = if verification.verified {
            None
        } else {
            Some(format!(
                "unverified ({})",
                verification.reason.as_deref().unwrap_or("unknown")
            ))
        };
        Ok(CachedSource {
            url: url.to_string(),
            name: if doc.name.is_empty() {
                url.to_string()
            } else {
                doc.name
            },
            verified: verification.verified,
            first_party: verification.first_party,
            key_id: verification.key_id,
            error,
            fetched_at: Some(Utc::now()),
            entries,
        })
    }

    /// GET with a STREAMING size cap: bytes are accumulated chunk-by-chunk and the fetch aborts the
    /// instant the running total exceeds `MAX_BODY_BYTES`, so a chunked / length-omitting hostile body
    /// can never force an unbounded allocation (the content-length pre-check is only an early-out).
    async fn get_capped(&self, url: &str) -> Result<Vec<u8>, String> {
        let mut resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("fetch {url}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("fetch {url}: HTTP {}", resp.status()));
        }
        if let Some(len) = resp.content_length() {
            if len as usize > MAX_BODY_BYTES {
                return Err(format!("fetch {url}: body too large ({len} bytes)"));
            }
        }
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| format!("read {url}: {e}"))? {
            if buf.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(format!("fetch {url}: body exceeds {MAX_BODY_BYTES} bytes"));
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(buf)
    }

    /// Merge bundled + remote catalogs with live state into the store view. `modules` = AppState.modules
    /// (compiled-in), `registrations` = installed sidecars.
    pub fn view(
        &self,
        modules: &[ModuleManifest],
        registrations: &[ModuleRegistration],
    ) -> RegistryView {
        let compiled_ids: HashSet<&str> = modules
            .iter()
            .filter(|m| m.mount == MountKind::Bundled)
            .map(|m| m.id.as_str())
            .collect();
        let installed: std::collections::HashMap<&str, &str> = registrations
            .iter()
            .map(|r| (r.id.as_str(), r.health.as_str()))
            .collect();

        let mut seen: HashSet<String> = HashSet::new();
        let mut entries: Vec<RegistryEntryView> = Vec::new();
        let mut sources: Vec<RegistrySourceView> = Vec::new();

        // 1) bundled — trusted by construction.
        let bundled = bundled_catalog();
        sources.push(RegistrySourceView {
            source: "bundled".into(),
            name: if bundled.name.is_empty() {
                "bundled".into()
            } else {
                bundled.name.clone()
            },
            verified: true,
            first_party: true,
            key_id: None,
            error: None,
            fetched_at: None,
            entry_count: bundled.entries.len(),
        });
        for e in &bundled.entries {
            if seen.insert(e.id.clone()) {
                entries.push(make_view(
                    e.clone(),
                    "bundled",
                    true,
                    &compiled_ids,
                    &installed,
                ));
            }
        }

        // 2) remote sources (entries only when verified / allowed; fail-closed otherwise).
        if self.enabled {
            let guard = self.remote.read().unwrap();
            for cs in guard.iter() {
                sources.push(RegistrySourceView {
                    source: cs.url.clone(),
                    name: cs.name.clone(),
                    verified: cs.verified,
                    first_party: cs.first_party,
                    key_id: cs.key_id.clone(),
                    error: cs.error.clone(),
                    fetched_at: cs.fetched_at,
                    entry_count: cs.entries.len(),
                });
                for e in &cs.entries {
                    if seen.insert(e.id.clone()) {
                        entries.push(make_view(
                            e.clone(),
                            &cs.url,
                            cs.verified,
                            &compiled_ids,
                            &installed,
                        ));
                    }
                }
            }
        }

        // 3) bring-your-own: installed sidecars present in no catalog -> Import shelf.
        for reg in registrations {
            if seen.insert(reg.id.clone()) {
                let e = CatalogEntry {
                    id: reg.id.clone(),
                    name: reg.name.clone(),
                    publisher: reg.publisher.clone(),
                    kind: crate::modules::ModuleKind::Imported,
                    summary: if reg.description.is_empty() {
                        "Self-installed sidecar plugin.".into()
                    } else {
                        reg.description.clone()
                    },
                    description: None,
                    version: Some(reg.version.clone()),
                    icon: reg.nav.0.first().map(|n| n.icon.clone()),
                    homepage: None,
                    categories: Vec::new(),
                    install: InstallSpec::Sidecar {
                        image: None,
                        default_base_url: reg.base_url.clone(),
                        subscribes: reg.subscribes.0.clone(),
                        role: Some(reg.role.clone()),
                        nav: reg.nav.0.clone(),
                        docs: None,
                    },
                };
                // BYO sidecars have no signed catalog listing -> never "verified".
                entries.push(make_view(e, "local", false, &compiled_ids, &installed));
            }
        }

        // 4) headless plugins loaded from disk (e.g. Wasm DetectionConsumers): not catalog entries and
        // not store-installable in v1, but surfaced so the operator sees what's running. State=Loaded,
        // mount=Headless (the dashboard shows a compute treatment, no Open/Install).
        for m in modules.iter().filter(|m| m.mount == MountKind::Headless) {
            if seen.insert(m.id.clone()) {
                let e = CatalogEntry {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    publisher: m.publisher.clone(),
                    kind: m.kind,
                    summary: m.description.clone(),
                    description: None,
                    version: Some(m.version.clone()).filter(|v| !v.is_empty()),
                    icon: m.nav.first().map(|n| n.icon.clone()),
                    homepage: None,
                    categories: vec!["headless".into()],
                    install: InstallSpec::Builtin {
                        availability: Some("loaded".into()),
                        contact: None,
                    },
                };
                entries.push(RegistryEntryView {
                    shelf: Shelf::from(e.kind),
                    state: EntryState::Loaded,
                    verified: false,
                    source: "local".into(),
                    mount: Some(MountKind::Headless),
                    entry: e,
                });
            }
        }

        RegistryView {
            enabled: self.enabled,
            sources,
            entries,
        }
    }
}

/// Compute an entry's shelf + state + render it.
fn make_view(
    entry: CatalogEntry,
    source: &str,
    verified: bool,
    compiled_ids: &HashSet<&str>,
    installed: &std::collections::HashMap<&str, &str>,
) -> RegistryEntryView {
    let shelf = Shelf::from(entry.kind);
    let state = match &entry.install {
        InstallSpec::Builtin { .. } => {
            if compiled_ids.contains(entry.id.as_str()) {
                EntryState::Included
            } else {
                EntryState::NotInBuild
            }
        }
        InstallSpec::Sidecar { .. } => match installed.get(entry.id.as_str()) {
            Some(&"unreachable") => EntryState::Unreachable,
            Some(_) => EntryState::Installed,
            None => EntryState::Available,
        },
    };
    RegistryEntryView {
        entry,
        shelf,
        state,
        verified,
        source: source.to_string(),
        mount: None,
    }
}

/// SSRF guard for an admin-configured registry URL: scheme allowlist + literal-IP rejection of
/// loopback/private/link-local (unless `allow_private`). Hostname→IP resolution rebinding is out of
/// scope for v1 (admin-trusted URLs, redirects disabled); documented in the registry guide.
fn validate_registry_url(url: &str, allow_private: bool) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("bad registry url: {e}"))?;
    match parsed.scheme() {
        "https" => {}
        "http" if allow_private => {}
        "http" => {
            return Err(
                "registry url must be https (set HELDAR_REGISTRY_ALLOW_PRIVATE for http)".into(),
            )
        }
        s => return Err(format!("unsupported registry url scheme `{s}`")),
    }
    if allow_private {
        return Ok(());
    }
    let Some(host) = parsed.host_str() else {
        return Err("registry url has no host".into());
    };
    // `url` returns IPv6 literals bracketed (e.g. `[::1]`); strip the brackets before parsing so the
    // literal-IP guard actually fires for the v6 family. A hostname that resolves to a private IP (DNS
    // rebinding) is out of scope for v1 (admin-trusted URLs, redirects disabled); see the registry doc.
    let host_ip = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = host_ip.parse::<IpAddr>() {
        return reject_private(ip);
    }
    Ok(())
}

/// Reject loopback/private/link-local/unspecified literals. v4-mapped/compat v6 (`::ffff:127.0.0.1`)
/// is canonicalized to v4 first so it can't smuggle a private v4 past the v6 arm; native v6 also
/// rejects unique-local (`fc00::/7`) and link-local (`fe80::/10`).
fn reject_private(ip: IpAddr) -> Result<(), String> {
    let ip = match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    let bad = match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
        }
    };
    if bad {
        Err(format!("registry url resolves to a non-public address ({ip}); set HELDAR_REGISTRY_ALLOW_PRIVATE to allow"))
    } else {
        Ok(())
    }
}

/// Background loop: refresh remote registries on the configured cadence. No-op (parks) when the
/// registry is disabled or no URLs are configured, so it never tight-loops.
pub async fn run(svc: std::sync::Arc<CatalogService>) {
    if !svc.enabled || svc.urls.is_empty() {
        std::future::pending::<()>().await;
    }
    // `interval`'s first tick fires immediately, so the loop's first iteration is the initial fetch —
    // no separate pre-loop refresh (which would double-fetch on startup).
    let mut tick = tokio::time::interval(Duration::from_secs(svc.refresh_s));
    loop {
        tick.tick().await;
        svc.refresh().await;
    }
}

#[cfg(test)]
mod tests {
    use super::validate_registry_url;

    fn rejected(url: &str) -> bool {
        validate_registry_url(url, false).is_err()
    }

    #[test]
    fn rejects_literal_private_and_loopback_ips() {
        // IPv4 literals (incl. obfuscated forms the url crate normalizes).
        assert!(rejected("https://127.0.0.1/c.json"));
        assert!(rejected("https://10.0.0.5/c.json"));
        assert!(rejected("https://169.254.169.254/c.json")); // cloud metadata
        assert!(rejected("https://2130706433/c.json")); // decimal 127.0.0.1
        assert!(rejected("https://0.0.0.0/c.json"));
        // IPv6 literals — the family that used to bypass the guard entirely.
        assert!(rejected("https://[::1]/c.json")); // loopback
        assert!(rejected("https://[fd00::1]/c.json")); // unique-local
        assert!(rejected("https://[fe80::1]/c.json")); // link-local
        assert!(rejected("https://[::ffff:127.0.0.1]/c.json")); // v4-mapped loopback
        assert!(rejected("https://[::ffff:169.254.169.254]/c.json")); // v4-mapped metadata
        assert!(rejected("https://[::]/c.json")); // unspecified
    }

    #[test]
    fn allows_public_hosts_and_ips() {
        assert!(validate_registry_url("https://registry.example.com/c.json", false).is_ok());
        assert!(validate_registry_url("https://8.8.8.8/c.json", false).is_ok());
        assert!(validate_registry_url("https://[2606:4700::1111]/c.json", false).is_ok());
    }

    #[test]
    fn http_requires_allow_private_and_then_anything_passes() {
        assert!(rejected("http://registry.example.com/c.json")); // http blocked by default
        assert!(validate_registry_url("http://127.0.0.1:9400/c.json", true).is_ok());
        // local dev
    }
}
