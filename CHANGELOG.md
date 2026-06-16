# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.1.2]: https://github.com/Straits-AI/heldar/releases/tag/v0.1.2
[0.1.1]: https://github.com/Straits-AI/heldar/releases/tag/v0.1.1
[0.1.0]: https://github.com/Straits-AI/heldar/releases/tag/v0.1.0
