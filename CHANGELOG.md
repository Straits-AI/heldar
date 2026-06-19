# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.7] — 2026-06-19

### Changed (breaking: schema reset)

- **Consolidated the 24 incremental kernel migrations into a single `0001_init.sql` baseline.** Done
  pre-1.0 with no production deployments, so the migration history was reset rather than preserved:
  **fresh databases only** — a database created by an earlier `0.1.x` cannot be upgraded across this
  collapse and must be recreated.
- **Removed the vestigial multi-tenant scaffold** (`tenants` table + `sites.tenant_id`). Heldar is
  single-tenant-per-deployment (each customer runs their own DVR); the tenant layer was never written
  or read. `sites` stays (single-org multi-site is real).

### Added

- **Per-camera AI decode priority** (`cameras.priority`): under fps-budget pressure the sampler now
  favors high-priority cameras (e.g. an ANPR gate lane) and sheds the lowest-priority first, instead
  of degrading every camera equally / blinding cameras in arbitrary order.
- **Edge-side fleet self-registration** (`services/fleet_register`): with `HELDAR_CP_URL` +
  `HELDAR_SITE_ID` + `HELDAR_PUBLIC_BASE_URL` set, a node POSTs its identity to a fleet control plane on
  boot and on a heartbeat, so it joins the fleet with no static config. Opt-in — unset means the node
  never phones home (the LAN/overlay default).
- **mTLS to the control plane** (`HELDAR_CP_TLS_CLIENT_CERT` / `_KEY` / `_CA`): the edge can present a
  client certificate and verify the control plane's server certificate when registering.

## [0.1.6] — 2026-06-18

First-principles stress/adversarial pass over the kernel's critical paths (concurrency, failure
injection, property tests). Twenty-four invariants tested; the eleven that didn't hold are fixed.

### Fixed

- **Retention (data loss):** the sweep unlinked a segment file and then deleted its row without
  re-checking the lock predicate — an evidence-hold or export read-lock committing mid-sweep could be
  destroyed. Deletion is now a TOCTOU-safe conditional `DELETE` (file unlinked only when the row is
  actually removed). Also fixed over-pruning: the quota/size loops deleted a whole batch past budget;
  they now stop the instant budget is met.
- **Auth (lockout):** the "last active admin" guard was check-then-act; concurrent demotions could
  drain admins to zero. Now an atomic single-statement guard.
- **Recordings:** `fetch_segments_in_range` could silently drop the newest segments on long ranges
  (false timeline gap); now keyset-paginated. Segments can no longer overlap in time (predecessor
  clamped on index).
- **Export read-locks:** clip/snapshot/backup released the segment read-lock only on the happy path;
  a cancelled or panicking export leaked it (footage un-prunable). Now an RAII guard releases on every
  outcome.
- **Clip honesty:** exports report `covered_seconds` + `gaps[]` instead of silently bridging real
  recording gaps.

### Added

- **Durable perception fan-out:** consumer fan-out happened after commit, best-effort — a crash
  dropped the notification. A new drainer replays un-fanned `outbox` batches, made exactly-once-safe by
  an at-most-once `consumer_fanout` claim per `(consumer, camera_id, frame_id)` (no consumer code
  changed). Migrations 0022 (incident index) + 0023 (fan-out durability).

### Hardened

- Ingest body bounded before deserialization; the incidents roll-up is indexed + `LIMIT`ed; transient
  DB busy/saturation maps to `503 Retry-After` instead of `500`.

## [0.1.5] — 2026-06-18

### Added

- **`heldar_kernel::env`** — a shared public module of env-parsing helpers (`var`, `var_or`,
  `parse_or`, `parse_bool`) with one consistent empty/whitespace + bool-truthiness policy. The
  generic app crates (`heldar-entry`, `heldar-movement`, `heldar-search`) now import `parse_or`
  from it instead of each carrying a byte-identical private copy.

### Removed

- The deprecated single-URL alerting webhook (`HELDAR_ALERT_WEBHOOK_URL` / the `alert_webhook_url`
  config + the legacy `app_state` migration path + the orphaned `app_state` table). It was
  superseded by webhook subscriptions. **Upgrade note:** a deployment that set
  `HELDAR_ALERT_WEBHOOK_URL` must recreate it as a webhook subscription via
  `POST /api/v1/webhooks` (or the dashboard's Webhooks panel).

### Internal

- CI now gates the open (`--no-default-features`) build, the lean-appliance guarantee (no `wasmi`
  by default), the `wasm` feature, RUSTSEC advisories, and the web typecheck. A fail-closed
  proprietary-code gate aborts the open-repo generator if any BakerySense surface survives stripping.

## [0.1.4] — 2026-06-17

### Added

- **Plugin store** — a browsable catalog (`GET /api/v1/registry`) with Core / Proprietary /
  Community / Import shelves, built from a bundled open catalog plus optional **signed remote
  registries** (detached Ed25519, verified server-side against pinned keys; fail-closed). New
  **Plugins** dashboard page to browse + install + uninstall. `scripts/sign-catalog.sh` + an example
  registry are included.
- **Sandboxed Wasm plugins** — install headless, capability-zero `DetectionConsumer` plugins (any
  language compiled to wasm32) loaded from a local directory and run in a [wasmi](https://wasmi-labs.github.io/)
  sandbox with no ambient authority (no filesystem/network/clock), bounded by fuel + memory + table +
  event/log caps, with per-plugin failure isolation. Behind an **off-by-default `wasm`** server feature,
  so the default appliance never links a Wasm runtime. Reference guest in `examples/wasm-plugin`.
- **System status** dashboard panel surfacing remote-access overlay reachability, disk/array (SMART/
  RAID) health, and the live-transcode engine; an **Audit log** viewer; a guarded camera **Reboot**
  control; and a **mobile navigation** menu.
- Optional ANPR plate-OCR backend via `apps/ai/requirements-anpr.txt`.

### Changed

- The recorded-media plane (`/media/*`) is now **authenticated** when `HELDAR_AUTH_ENABLED=true`
  (it was previously served without auth).
- `GET /api/v1/events/types` now also returns event types observed at runtime (plugin/app-emitted), not
  just the static taxonomy.
- Retention now prunes the `webhook_deliveries`, `recording_gaps`, `search_log`, and `bakery_reports`
  ledgers (previously unbounded).

## [0.1.3] — 2026-06-16

### Added

- **Dynamic module platform** — the dashboard builds its nav rail + routes from live
  truth (`GET /api/v1/modules`) instead of a hardcoded list, so only loaded modules
  appear. Each compiled app declares a `ModuleManifest`
  (`heldar_kernel::modules`); the composing binary collects them into
  `AppState.modules`.
  - **Sidecar plugins** — install out-of-process plugins (any language) at runtime
    with no rebuild. `POST /api/v1/modules` (admin) mints a least-privilege scoped
    API key + a webhook subscription and reverse-proxies `/m/{id}/*` to the
    sidecar's own UI + API (single-origin micro-frontend); `DELETE` reverses all
    three. A `/heldar/health` probe loop badges reachability. New **Plugins**
    dashboard page to install / list / uninstall. Reference template at
    `examples/hello-module` + the SPI guide in the docs.
- **Webhook subscriptions** — a generic, signed event-delivery substrate that
  supersedes the single-URL alerting webhook. Each subscription is an independent
  at-least-once deliverer with an event-type/severity filter, an optional
  HMAC-SHA256 signing secret (`X-Heldar-Signature`), and a per-delivery ledger.
  `GET /api/v1/events/types` exposes the event taxonomy.
- **One-URL deploy** — `heldar-core` serves the built dashboard itself
  (`HELDAR_WEB_DIR`), so the whole product is one binary at one URL.

### Changed

- Uniform route authorization — the `Principal` capability guard is applied across
  all kernel routes (a no-op when `HELDAR_AUTH_ENABLED` is false).

## [0.1.2] — 2026-06-15

### Added

- **Camera Configuration** — vendor-abstracted camera management, configure cameras
  directly from Heldar over HikVision ISAPI. HTTP Digest auth (RFC 2617) is
  hand-rolled, so no new dependencies were added.
  - Device info readout and per-channel video configuration
    (codec / resolution / fps / bitrate / GOP).
  - Time and NTP synchronization.
  - ONVIF enablement and user provisioning.
  - OSD configuration and device reboot.
  - Bulk "apply to all cameras" endpoint with actions
    `enable_onvif | sync_time | set_ntp | set_video`.
  - `CameraConfigPanel` and `BulkConfigPanel` dashboard UI.

## [0.1.1] — 2026-06-15

### Added

- Per-crate READMEs for the published crates.
- Full DVR feature set: durable evidence-lock + incident API, per-camera storage
  quota, optional audio recording, scheduled snapshots, per-camera recording
  schedules, event-triggered recording (pre/post-roll), segment-spanning HLS
  playback, backup/archival (SFTP / FTP / NAS / S3 + on-demand zip), ONVIF
  Profile S + PTZ, dual/mirror recording, ANR edge re-fill, and delegated HA/ops
  items (SMART/RAID health, `/readyz` quorum probe, VAAPI/NVENC transcode flag,
  fleet outbox).

### Changed

- Scrubbed proprietary-reference material from the published source and docs.

## [0.1.0] — 2026-06-15

### Added

- Initial open-core release: the domain-agnostic media/perception kernel plus the
  generic reference apps (access control, movement intelligence, semantic search)
  and the composing server. Apache-2.0.

[0.1.4]: https://github.com/Straits-AI/heldar/releases/tag/v0.1.4
[0.1.3]: https://github.com/Straits-AI/heldar/releases/tag/v0.1.3
[0.1.2]: https://github.com/Straits-AI/heldar/releases/tag/v0.1.2
[0.1.1]: https://github.com/Straits-AI/heldar/releases/tag/v0.1.1
[0.1.0]: https://github.com/Straits-AI/heldar/releases/tag/v0.1.0
