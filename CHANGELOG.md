# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Tests

- **Characterization tests for the two least-tested critical paths** (recorder supervision + backup
  execution), which previously had healthy-looking test *counts* dominated by pure-helper tests. The
  supervision *decision* logic is now pinned by extracting three behavior-preserving pure functions
  from `run_supervise`/`run_event_supervise` — `next_backoff` (reconnect-storm guard), `extend_trigger_window`
  (a trigger only extends, never shrinks, an event window), and `event_recheck_secs` (post-roll stops on
  time, never busy-spins) — plus a test of `build_record_command` that pins the ffmpeg stream-copy/segmenting
  args and the credential/injection-safety guarantee (the stream URL stays a single argv element). Backup
  copy execution is now covered by a real filesystem round-trip: `copy_local`'s fs loop was extracted to a
  testable `copy_segments_to_dir` (byte-identical copy, vanished-source skip) and `dir_size_bytes` is
  pinned. (Full ffmpeg-child-lifecycle coverage still needs a fake-ffmpeg harness — tracked follow-up.)

### Added

- **Versioned app-schema migrations.** The composed apps (entry / movement / search + the proprietary
  bakery vertical) previously self-installed a single `CREATE TABLE IF NOT EXISTS` blob with no
  versioning, so a shipped schema change silently no-opped on an already-booted box. They now use the
  kernel's new `db::run_app_migrations` — numbered, append-only migrations under
  `crates/heldar-<app>/migrations/NNNN_*.sql`, applied + recorded atomically and tracked per-component
  in a shared `_heldar_app_migrations` table (so apps don't collide with the kernel's `sqlx::migrate!`
  or each other on the one SQLite database). A checksum guard rejects editing an already-applied
  migration. `0001_init` is the original idempotent blob, so existing boxes upgrade with no data loss.
  To evolve a schema: add `migrations/NNNN_*.sql` + a line to that app's `MIGRATIONS` array.

### Security

- **Shared SSRF egress guard** (`heldar-kernel::net_guard`): every server-initiated outbound HTTP
  request now goes through one guard instead of only the plugin-registry fetcher having one. It rejects
  the cloud-metadata/link-local range (`169.254.169.254`, `fe80::/10`) and the unspecified/broadcast
  addresses on every deployment, gates loopback + RFC1918/ULA behind a per-sink `EgressPolicy`
  (LAN-appliance sinks keep reaching cameras / the local MediaMTX / localhost sidecars), and disables
  HTTP redirect-following (the bypass that let a public host `302` the box to an internal URL). Wired
  into webhook create/update + delivery, ONVIF probe `device_url`, and sidecar `base_url`; the registry
  fetcher was refactored onto the same guard. Closes the authenticated SSRF / metadata-oracle findings
  on those sinks. Camera ISAPI/ONVIF and the sidecar reverse-proxy share the redirect-disabled client.
- **Control-plane authorization.** The fleet API previously had NO application-layer authz (mTLS was the
  only gate; plain-HTTP default was fully open). It now supports an operator bearer token
  (`HELDAR_CP_ADMIN_TOKEN`): when set, every operator-facing route (dashboard JSON + node/event/alert
  listings + all alert-rule/route CRUD) requires `Authorization: Bearer <token>` (constant-time compared);
  the node-facing register route keeps its mTLS-CN gate. Unset = open (LAN/overlay default, unchanged —
  hardening is opt-in, so the daemon warns rather than refusing to boot). The embedded NOC dashboard
  prompts for the token and sends it. Alert-route SSRF was also closed (public-https-only validation +
  redirect-disabled client, `heldar-control-plane::net_guard`) and 500s no longer leak raw error strings.
- **Control-plane CRL hot-reload.** The CRL (and cert/key/CA) were loaded once at boot, so revoking a
  compromised node cert needed a redeploy. They're now re-read on a background cadence (~30s) and the
  TLS config is rebuilt + swapped when a file changes, so a re-issued CRL revokes a node on new
  connections without a restart (a rebuild failure keeps the previous good config; boot still validates
  the initial one).
- **MediaMTX per-user video gating.** The browser streams directly from MediaMTX, so the kernel's
  `can_view` was bypassable. MediaMTX now uses HTTP external-auth against a kernel endpoint
  (`/internal/mediamtx-auth`): reads require a short-lived, path-scoped, kernel-minted HMAC token (minted
  in `/liveview`, carried on the WHEP URL and every HLS request via a custom hls.js loader) when kernel
  auth is enabled, and fall back to a LAN/private/overlay source-IP check when auth is disabled (so a
  port-forwarded box still doesn't serve the internet). Publishing stays loopback-only. WebRTC/WHEP (the
  primary path) authorizes once and streams indefinitely; the token endpoint + mint are unit-tested.
  _Needs live end-to-end verification against a running MediaMTX before release (the HLS-segment-token
  loader and the auth callback can't be exercised in CI); the auth-ON HLS **fallback** re-auths per
  segment, so a session outliving the token TTL falls back to WebRTC or a `/liveview` refresh, and Safari
  native-HLS-only is a known degraded corner._
- **FFmpeg argument injection** via camera fields closed: `validate_stream_url` now rejects whitespace/
  control characters (the camera stream URL is interpolated into the MediaMTX `runOnDemand` command
  string), the `address` field is validated the same way, and `anr_replay_url_template` is held to the
  stream-URL scheme allow-list (blocks `file:`/`gopher:` → `ffmpeg -i`).
- **Encryption-at-rest is fully wired.** Enabling `HELDAR_SECRET_KEY` previously *broke* all camera
  config/ONVIF/PTZ (the sealed `enc:v1:…` blob was sent as the auth password) and discovery-onboarded
  cameras stored their password in plaintext. ISAPI, ONVIF probe + PTZ, the ONVIF-persisted stream URL,
  and discovery auto-add now seal/unseal camera credentials consistently.
- **Auth lifecycle**: a password reset now revokes the user's active sessions (a stolen session no longer
  survives the remediation); the login-lockout counter is an atomic SQL increment (a parallelized
  attacker can no longer exceed `login_max_failures` via a lost-update race). `/metrics` and
  `/api/v1/site` now require an authenticated principal (open in LAN mode, gated when auth is on).
- **Exposure detection**: `HELDAR_INTERNET_EXPOSED=true` lets an operator declare exposure for the
  reverse-proxy / port-forward / public-cloud-bind cases the automatic remote-path detection can't see,
  so the auth-off boot refusal + hardening guardrails still fire.
- **Sidecar reverse-proxy** now forwards the authenticated caller's identity + role (`X-Heldar-User`/
  `-Role`/`-Principal-Kind`, with client-supplied `x-heldar-*` stripped to prevent spoofing) so a plugin
  can enforce its own authorization across the proxy boundary.
- **Backup credentials no longer leak via argv.** rclone destination secrets (S3 access/secret keys,
  SFTP/FTP password) are passed to the rclone child via backend env vars (`RCLONE_S3_SECRET_ACCESS_KEY`,
  `RCLONE_SFTP_PASS`, …) instead of the on-the-fly connection string, so they no longer appear in the
  world-readable `/proc/<pid>/cmdline`; the password-obscure step now feeds the plaintext on stdin
  (`rclone obscure -`) rather than as an arg. _Needs a live backup test against S3/SFTP to confirm the
  on-the-fly `:s3:` remote honors `RCLONE_S3_*`._

### Fixed

- **Self-bounding storage — closed the leaks.** `consumer_fanout` is now pruned by retention (it grew
  forever and defeated the DB size cap, which only sheds `detections`); exported clips and the mirror
  (dual-DVR) directory are reclaimed by mtime; unresolved movement `breach_alerts` (plate PII) age out at
  the retention ceiling. Clip/mirror reclamation runs *before* the disk-free floor so the floor no longer
  evicts real recordings to make room for un-reclaimable clips.
- **Retention no longer stalls ingest**: the high-rate table prunes (detections/outbox/consumer_fanout/
  events/webhook_deliveries) delete in bounded 5k-row batches (yielding between them) instead of one
  unbounded DELETE that held SQLite's single writer for the whole backlog.
- **Segment read-lock is now a reference count**, not a boolean — two overlapping clip/playback holders
  no longer release each other's lock, so retention can't delete footage mid-export.
- **Forensic search no longer claims completeness when truncated**: when a source hits its fetch cap the
  result is flagged `truncated` and the proof layer reports the count as a floor (`"At least N…"`, partial
  confidence) instead of an authoritative complete total.
- **Recording schedules**: reject a zero-length window (`time_start == time_end`) and an empty `days` set
  (both silently never recorded). tzdata is shipped in the Docker/appliance images and the effective local
  timezone is logged at boot, so `chrono::Local` schedules no longer silently fire in UTC.
- **Snapshot scheduler** advances `last_fired_at` on capture failure, so a persistently failing camera
  retries at the configured interval instead of hot-retrying every watcher tick.
- **Dashboard**: a plugin catalog's `homepage` URL is scheme-validated before rendering into an `href`
  (blocks `javascript:`/`data:` click-through); the AI task-type hint no longer advertises a `tracking`
  analyzer that doesn't exist.
- **Live view (WHEP)**: the SDP exchange (WHEP POST / rendezvous) now has an 8s timeout (via
  `AbortController` for the POST), so a media/relay server that accepts the socket but never answers no
  longer leaves the player stuck on "Connecting" with no HLS fallback. Fixed a resource leak where
  `close()` during the in-flight POST left the just-created MediaMTX WHEP session un-`DELETE`d. Also
  documented two residual risks accurately (rather than pretend-fixing them): the sidecar plugin iframe
  is served same-origin so `allow-scripts`+`allow-same-origin` does not isolate it from the console
  origin (real fix = a distinct plugin origin), and the ADR-0003 rendezvous exchange helper carries no
  viewing ticket and must not be wired into remote viewing until one is added.

### Changed

- **Runtime-loaded module frontends**: every dashboard module UI (entry / movement / search + the
  proprietary bakery vertical) now loads at **runtime** as a native-React ES bundle served by its own
  crate at `/api/v1/modules/{id}/ui` and mounted by `ModuleHost`, instead of being compiled into the
  dashboard. Modules share the shell's React + a shell SDK (`@heldar/shell` — api client, auth, ui kit,
  formatters) via an import map, so bundles stay tiny (~10–50 KB). This makes the dashboard SPA
  byte-identical for the open and full builds: the **`heldar-web-full` image is removed** (one
  `heldar-web` serves both), and the open-repo generator's per-file frontend stripping collapses to
  deleting a single self-contained `apps/web/src/modules/<vertical>` directory. Module UIs ride the
  existing remote-access relay (`/api/v1/*`) unchanged. See `website/docs/develop/module-system.md`.

## [0.2.0] — 2026-06-24

### Added

- **Remote access (WebRTC, browser-based)**: a deployment behind CGNAT dials OUT to a signaling + TURN
  rendezvous, and the **full `apps/web` dashboard** runs remotely (live multi-camera, recorded playback,
  config) under a two-gate auth model — an outer relay capability + the real kernel session, both
  HttpOnly cookies, with the box kernel as the sole RBAC authority. A standalone `/view` ticket link
  gives shareable per-site/per-camera viewing. TURN is operator-tunable via `HELDAR_WEBRTC_ICE_SERVERS`
  (bring-your-own STUN/TURN), else the rendezvous default. See `docs/REMOTE-ACCESS.md`.
- **Recording disk-size limit**: the retention sweeper bounds recordings by a size cap
  (`HELDAR_MAX_RECORDINGS_GB`) and a free-disk floor (`HELDAR_MIN_FREE_DISK_GB`), evicting oldest-first
  so recordings can't fill the disk. Runtime-tunable without a restart via `GET`/`PUT
  /api/v1/system/retention` (admin) + a dashboard System-page panel.
- **HEVC / H.265+ recorded playback**: H.265+ recordings (≈4× smaller than H.264) play natively in-page
  on HEVC-capable browsers, with a clear note for the no-HEVC tail.
- **Production hardening** (opt-in; LAN-appliance defaults unchanged):
  - **Per-account login lockout** (`HELDAR_LOGIN_MAX_FAILURES` / `_LOCKOUT_MIN`): locks an account
    after N consecutive failed logins; admin clears via `POST /api/v1/users/{id}/unlock`.
  - **Camera-credential encryption at rest** (`HELDAR_SECRET_KEY`, AES-256-GCM): seals camera passwords;
    existing plaintext is re-sealed at boot; unset = plaintext (LAN appliance).
  - **Fail-loud startup guardrails**: an internet-exposed deployment refuses to boot with auth off, and
    warns — or refuses, under `HELDAR_STRICT_PROD=true` — on a non-`Secure` cookie, no idle timeout, an
    over-long session TTL, a localhost CORS allowlist, or plaintext credentials.
  - **Optional Cloudflare Turnstile** on the dashboard login (enforced only when configured).
  - `docs/PRODUCTION.md` checklist + `.env.production.example`.
- **Deployment paths**: a Docker dev stack (`docker-compose.yml`), a native systemd appliance engine
  (`infra/systemd/`), and an appliance-image scaffold (`scripts/build-appliance-image.sh`).
- **`scripts/release.sh`**: one-command release (bump → verify → regenerate open tree → tag → publish).

### Changed

- The `Principal` auth guard now spans the legacy read routes (cameras, live view, health, events,
  recordings, schedules): with `HELDAR_AUTH_ENABLED=true` the **entire** API requires a session.

### Fixed

- **Relay allowlist SSRF bypass**: the box-side relay canonicalizes the request path before the allowlist
  check (an encoded `%2e%2e` traversal could otherwise reach an off-surface endpoint), and caps relayed
  response bodies.
- Disabled cameras report `disabled` (not a stale `recording`/`error`) in the health table.
- `POST /api/v1/cameras/{id}/ai-tasks` is idempotent per `(camera, task_type, stream_profile)` — no
  duplicate detection tasks across restarts.

## [0.1.8] — 2026-06-19

### Added

- **Email/SMTP notifier** (off-by-default `smtp` cargo feature): relays matching events to configured
  recipients over SMTP (`HELDAR_SMTP_*`). The lean appliance build links no SMTP/TLS stack unless
  compiled with `--features smtp`. Webhooks stay the durable, UI-managed channel; this is a lightweight
  always-on inbox relay. Starts at boot (no backlog replay) and is at-most-once best-effort, so a dead
  relay can never wedge the loop.
- **Multi-camera synchronized playback** (dashboard): a Playback page that plays several cameras'
  recorded footage over a shared timeline with one transport (play/pause/seek/speed), clock-mastered so
  the views stay in lockstep within a small drift tolerance.
- **DVR-style multi-view camera wall** (dashboard): layout picker (1 / 2×2 / 3×3 / 4×4) with
  pagination; the chosen layout + page persist in the URL and `localStorage`.
- **Digital zoom + pan on live view** (dashboard): scroll/control-driven zoom on a live tile, plus live
  audio playback for cameras that opt in to audio recording.

### Changed

- The MediaMTX live-preview path now carries audio for cameras with audio recording enabled (was
  always muted with `-an`).

### Testing

- New Playwright UI e2e suite + a full-stack synthetic-camera harness (MediaMTX + synthetic RTSP
  cameras + core on an isolated port/DB) covering the dashboard, camera wall, single-camera, and
  synchronized-playback flows.

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
