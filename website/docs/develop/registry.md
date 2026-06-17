---
id: registry
title: Plugin Registry
sidebar_label: Plugin Registry
sidebar_position: 3
---

# Plugin registry

The **Plugin Store** browses a *catalog* of available plugins and cross-references it with what is
loaded. A catalog comes from two kinds of source:

- the **bundled** first-party catalog, compiled into the binary — always available, even offline, and
  trusted by construction;
- optional **remote** registries (signed JSON documents at admin-configured URLs) — this is how the
  proprietary and community shelves are populated, without baking anything into the binary.

Installing a sidecar entry funnels through the [sidecar register flow](./sidecar-plugins.md); the
catalog only does discovery. Compiled modules show as *Included* / *Contact* — they are build-time, not
runtime-installable.

## Catalog format (`heldar-catalog/v1`)

```json
{
  "format": "heldar-catalog/v1",
  "name": "Acme Registry",
  "issued_at": "2026-06-16T00:00:00Z",
  "expires_at": "2026-12-16T00:00:00Z",
  "entries": [
    {
      "id": "weather-overlay",
      "name": "Weather Overlay",
      "publisher": "Acme Plugins",
      "kind": "community",
      "summary": "Overlay local weather on the wall.",
      "description": "Longer copy shown in the detail drawer.",
      "version": "1.0.0",
      "icon": "module",
      "homepage": "https://example.com/weather-overlay",
      "categories": ["overlay"],
      "install": {
        "type": "sidecar",
        "image": "ghcr.io/acme/weather-overlay:1.0.0",
        "default_base_url": "http://127.0.0.1:9300",
        "subscribes": ["*"],
        "role": "viewer"
      }
    }
  ]
}
```

| Field | Meaning |
| --- | --- |
| `kind` | `core` / `proprietary` / `community` — picks the shelf and badge. |
| `install.type` | `sidecar` (runtime-installable, pre-fills the register form) or `builtin` (compiled; CTA only). |
| `install.default_base_url` | sidecar only: the URL the install form pre-fills (operator-editable). |
| `install.subscribes` / `role` | sidecar only: event types to receive + the minted key's role. |
| `install.image` | sidecar only: an informational deploy hint — the kernel never pulls or runs it. |
| `install.availability` / `contact` | builtin only: `open` / `commercial` + a contact for the CTA. |

The dashboard cross-references each entry against live state and renders one of: **Available**,
**Installed**, **Included**, **Not in build**, **Unreachable**.

## Trust model

A remote catalog is only trusted if its **detached Ed25519 signature** verifies against a **pinned
public key**. Signing covers the *exact* catalog bytes (no JSON canonicalization), mirroring the
webhook signer.

- The `<catalog-url>.sig` artifact sits next to the catalog: `{ "alg": "ed25519", "key_id": "...",
  "signature": "<base64 raw 64-byte sig>" }`.
- Verification runs **server-side** against the compile-time pinned keys plus any operator keys in
  `HELDAR_REGISTRY_TRUSTED_KEYS`. The browser never sees a key and never verifies — so a forged catalog
  can never paint a fake **Verified** badge.
- It is **fail-closed**: an unverified remote source contributes **zero** entries (set
  `HELDAR_REGISTRY_ALLOW_UNVERIFIED=true` to relax for a trusted internal registry).
- The bundled catalog is trusted by construction (it *is* the binary), so its entries are always
  verified — the badge is honest even offline.

A **Verified** badge means *the listing was signed by a pinned publisher key* — not that the plugin's
code is safe. A sidecar still runs out-of-process with a least-privilege minted key.

## Sign + publish

```bash
openssl genpkey -algorithm ed25519 -out registry.pem            # once; keep the private key secret
openssl pkey -in registry.pem -pubout -outform DER | tail -c 32 | base64   # the pinnable public key
./scripts/sign-catalog.sh catalog.json registry.pem my-key      # -> catalog.json.sig
```

Host `catalog.json` + `catalog.json.sig` over HTTPS, pin the public key
(`HELDAR_REGISTRY_TRUSTED_KEYS=my-key:<base64>`), and set `HELDAR_REGISTRY_URLS` to the catalog URL.
A runnable end-to-end example lives in
[`examples/registry`](https://github.com/Straits-AI/heldar/tree/main/examples/registry).

## Configuration

| Env | Default | Purpose |
| --- | --- | --- |
| `HELDAR_REGISTRY_ENABLED` | `true` | Master switch for remote-registry fetching (bundled catalog always loads). |
| `HELDAR_REGISTRY_URLS` | *(empty)* | Comma-separated catalog URLs. Empty = no phone-home. |
| `HELDAR_REGISTRY_REFRESH_S` | `900` | Background refresh cadence. |
| `HELDAR_REGISTRY_FETCH_TIMEOUT_S` | `10` | Per-fetch timeout. |
| `HELDAR_REGISTRY_TRUSTED_KEYS` | *(empty)* | Extra pinned keys, `key_id:base64pubkey,...`. |
| `HELDAR_REGISTRY_ALLOW_UNVERIFIED` | `false` | Surface unverified remote entries (badged unverified). |
| `HELDAR_REGISTRY_ALLOW_PRIVATE` | `false` | Allow http / private/loopback registry URLs (SSRF guard). |

Remote fetches use a dedicated client with redirects disabled, an HTTPS-only default, a 2 MiB body cap,
and literal private/loopback-IP rejection. Hostname→private-IP rebinding is out of scope for v1 (URLs
are admin-configured and redirects are off).
