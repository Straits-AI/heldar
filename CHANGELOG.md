# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- **The hardened container profile is now verified rather than assumed, and every service reports
  health.** `deploy/compose.hardened.yml` was booted and checked by hand once; nothing re-checked it
  afterwards. Container hardening rots in a way that looks like nothing — a capability added back to
  fix a crash, a service that gains a writable path, an overlay dropped from the deploy command — and
  the stack keeps working perfectly, which is exactly why nobody notices.

  `scripts/check_hardened_profile.py` audits the **rendered** Compose configuration (not the overlay
  files, so what it checks is what Compose actually produces) for dropped capabilities,
  `no-new-privileges`, read-only roots, tmpfs flags and bounds, log limits, CPU/memory/PID ceilings,
  health checks, and MediaMTX's admin port staying on loopback. Output is `heldarctl doctor`'s Finding
  shape. Exemptions are declared with their reasons, so a service losing `read_only` by accident still
  fails. CI runs it against the hardened stack *and* the base stack without the overlay, requiring the
  second to be **rejected** — a checker that passed everything would otherwise look identical to a
  hardened deployment.

  Health checks added for `web` (busybox `wget`; the alpine image has no curl) and for the AI worker,
  which serves no HTTP and now publishes a heartbeat with its own staleness deadline — liveness, not
  readiness. The deadline is computed by the worker from its timeout, retry and poll settings rather
  than hardcoded, because a fixed bar is wrong in both directions: 90 s left 0.6 s of margin against a
  retry-exhausted poll (which is what a kernel outage causes, the very thing the check should stay
  quiet about), while an operator raising the uncapped poll interval past it would mark a healthy
  worker unhealthy forever. MediaMTX cannot have a check at all: its image is a single layer whose
  entrypoint is the bare binary, with no shell to exec. That is recorded as a declared exemption
  rather than left as an apparent oversight.

  The profile checker also refuses `privileged: true`, host PID/IPC/cgroup/user namespaces, and
  bind-mounts of the Docker socket or host root — each of which makes every other control in the
  overlay meaningless — and treats a `0` CPU/memory/PID limit as the absence of a ceiling that Docker
  understands it to be. It reads tmpfs mounts in both Compose syntaxes. All found by adversarial
  review of the first version, which reported a privileged, docker-socket-mounted, host-root-mounted
  service as fully hardened.

  Also added `noexec` to the nginx `conf.d` tmpfs, which the new check found missing.

- **Container images are scanned before they are published, not after.** The filesystem scan reads
  the source tree and says nothing about the base image the product ships on, so a digest bump could
  drag in a fixable HIGH and hand it to everyone pulling `latest` with nothing looking wrong.

  `docker-open.yml` now builds the amd64 half locally, scans it, and only then runs the real
  multi-arch build and push. The ordering is the point — a scan after the push reports on something
  already handed out — and `scripts/check_trivy_gate.py` asserts a pushing build step still follows
  the scan so the two cannot be reordered back. It also holds every Trivy invocation in the repo to
  `exit-code: "1"`, so a report-only scan cannot be introduced anywhere.

  `ci.yml` scans the same images under the same policy on any PR touching an image input, moving the
  answer to the PR that causes it rather than to tag time. arm64 is not scanned: for a pinned base
  digest the OS package set is identical across architectures.

- **A failing weekly security scan now files an issue instead of reporting to nobody.** The Monday
  cron re-scans unchanged code and is the only mechanism that catches an advisory published *after* a
  release — there is no PR for that finding, because nothing was merged to cause it. But a scheduled
  run has no PR to turn red and no author to notify, so a red cron sat in the Actions tab until
  somebody happened to look. The one class of finding nothing else could catch was the one nothing
  surfaced.

  `advisory-report` opens an issue when the weekly run fails, updates that same issue on later
  failures rather than filing a fresh one every Monday, and closes it when a run comes back clean. It
  identifies its own issue by a marker in the body, not by title or label, so it can never adopt — or
  close — an issue somebody wrote by hand.

  `scripts/check_weekly_report.py` enforces coverage rather than configuration: every job in
  `security.yml` must be in the reporter's `needs`, so adding a scanner without listing it fails the
  build instead of silently going unreported. Anything not `success` counts as failing — cancelled
  and skipped included, since a skipped scan has cleared nothing.

- **A release can no longer be published from a commit whose security scan failed.** Nothing
  connected the two: `release.yml` and `docker-open.yml` triggered on a tag push, and the tag could
  sit on any commit at all. Publishing is the irreversible step — a crates.io version can never be
  replaced, and an image tag, `latest` above all, is pulled by everyone who trusts it.

  Both workflows now begin with a `security-gate` job, and every other job depends on it. The lookup
  is by commit SHA, since `security.yml` does not run on tag pushes — so what is checked is the run
  from when that commit landed on main, and tagging a commit that never reached main blocks. The most
  recent completed run wins: the weekly cron re-scans unchanged code, so a newly published advisory
  turns a green commit red and must block a release rather than be overridden by an older success.

  The gate fails closed — no run, an API error, or an unrecognised `enforce` value all block. A
  `workflow_dispatch` dry run reports without blocking. Its decision logic is driven against a
  stubbed API in CI rather than first executing during someone's release.

- **The Trivy gate now proves it can fail.** `exit-code: "1"` was absent for a long stretch: the scan
  produced SARIF, findings piled up on the code-scanning dashboard, and every PR showed green. The
  flag is set now — but nothing demonstrated it was still set and still doing something, and a gate
  whose failure path has never executed is a gate nobody has tested.

  The workflow now scans two fixtures with the same action, version and verdict-affecting inputs as
  the real scan: a deliberately vulnerable one that must fail, and a clean one that must pass. The
  second is what makes the first mean anything — a scanner red on everything would otherwise satisfy
  the test. `scripts/check_trivy_gate.py` compares the self-test's configuration against the real
  scan rather than keeping its own copy, so it cannot drift into testing a gate nobody ships, and it
  requires `scripts/fixtures` to stay in `skip-dirs` so the seeded pins never fail an unrelated PR.

  The vulnerable fixture pins four old packages, not one: a single advisory can be re-scored below
  HIGH and quietly retire the check. If it ever scans clean, the fixture has gone stale.

- **The blocking dependency gate now covers the AI stacks that actually ship.** `pip-audit` examined
  `requirements-core.txt` and nothing else, so torch, opencv, the CUDA wheels and PaddleOCR — by a
  wide margin the largest and fastest-moving native trees in the product — were outside it entirely.
  Pointing the audit at the requirements files would not have fixed that either: they carry open
  floors (`ultralytics>=8.4.115`), so the answer changes with whatever the resolver picks that
  morning, and a gate whose verdict drifts is not a gate.

  Each documented install profile (`core`, `detect`, `anpr`, `embed`) now has a committed, fully
  pinned lock in `apps/ai/constraints/`, compiled for linux x86_64 / Python 3.12 — what the shipped
  image and the box both run, and the only platform on which the `nvidia-*` wheels are even present.
  All four are audited on every PR and weekly, and all four are clean today. `scripts/check_ai_locks.py`
  fails the build when a lock is missing, unpinned, orphaned, or stale against the requirements it
  was compiled from, so a requirements edit cannot leave the gate auditing last month's tree.

  Accepted advisories go in `security/dependency-exceptions.json` and must name the component, whether
  our code reaches the vulnerable path, the compensating control, an owner, a follow-up issue and an
  expiry. CI fails from the expiry date onward and rejects a date more than 180 days out. The audit
  takes its suppression list from that file alone — a hardcoded advisory id anywhere in the security
  workflow fails its own test. The register is empty, which is the correct state.

  Regenerate locks with `./scripts/lock_ai_profiles.sh` after any requirements change. CI never
  regenerates: a lock produced by CI is a lock nobody reviewed.

- **Revoking an API key now stops the backup transfer it left running.** `POST /backup/policies/{id}/trigger`
  answers 202 and detaches the copy, which keeps moving footage for up to `HELDAR_BACKUP_JOB_TIMEOUT_S`
  (default an hour) — and for `sftp`/`ftp`/`s3` destinations, off the box entirely, where no later
  guard can reach it. Every authorization for that job was made once, at request time, and the job row
  carried no way back to the credential that made it, so revocation — the operator saying "this
  credential is compromised" — did nothing to bytes already in flight. Narrowing `scope_cameras` was
  the same defect against the camera boundary: the job went on shipping a camera the credential no
  longer held.

  `backup_jobs` now records **who ordered the job** (`created_by`, `created_by_kind`; kernel migration
  0015) and the transfer re-checks that credential before the first byte and every few seconds after,
  aborting with the reason recorded on the job. Withdrawn means revoked, deactivated, deleted, expired,
  or re-scoped off a camera the job covers. Files already copied are left in place — they are backups,
  not spoils.

  **No behaviour change** for the scheduler (it holds no principal, so its jobs record no creator),
  for boxes running with auth disabled, or for jobs created before the upgrade. A database read failure
  during the re-check lets the transfer continue, loudly: the recorder shares that SQLite and a busy
  timeout must not destroy a backup.

### Documented

- **How long a scope decision lasts**, per surface, in `docs/ACCESS-CONTROL.md`. `/media/*` re-authorizes
  every single request, so a clip, playback session or archive URL is not a bearer capability and a
  re-scope bites mid-scrub; AI leases re-derive their candidate list from the current scope on every
  renew. The exception is **live view**: MediaMTX authorizes from a signed URL token and consults no
  credential, so a revoked key keeps streaming for up to `HELDAR_LIVEVIEW_TOKEN_TTL_SECS` (default
  3600 s) — and an established WebRTC session is not bounded by it at all. Lower the TTL if revocation
  needs to bite promptly.
- **A camera-confined search no longer loses the caller's own rows to a page of other cameras'.**
  `heldar-search`'s executor fetched each source `ORDER BY <ts> DESC LIMIT fetch_cap` and applied
  `plan.cameras` afterwards in Rust, so the cap bounded rows EXAMINED rather than rows returned:
  newer rows from cameras the caller did not name ate the page and the caller's own older in-window
  matches never reached the filter — unreachable afterwards, since these routes carry no offset or
  cursor. For a camera-scoped credential that list is its scope, so this was the scope layer denying
  the caller its own data. The same ordering made `truncated` report the FLEET's in-window volume
  beside a `count` of 0. The confinement is now pushed into each source query (deduped, and skipped
  above 1000 distinct ids so an absurd caller-supplied list degrades to the Rust filter instead of
  exhausting SQLite's variable ceiling).

- **Movement's audit entries name the camera they are about, so a scoped operator can see its own
  work.** `GET /api/v1/audit` filters on `audit_log.subject_camera_id` and hides NULL subjects from a
  camera-scoped reader; `heldar-movement` audited every action with an empty `detail`, so every
  movement act — including a breach acknowledged on the reader's own camera — was visible only to a
  fleet-wide credential. Breach ack/resolve and the person search now carry their camera. Acts naming
  TWO cameras (link create/delete, candidate confirm/reject) deliberately still carry none: a single
  column cannot say "both ends", and naming either one would disclose adjacency to that end's holder.

### Fixed

- **The AI worker no longer reports failure every time it shuts down cleanly.** `TaskRunner`
  subclasses `threading.Thread` and stored its stop flag as `self._stop` — but `Thread._stop` is a
  real CPython method, which `join()` calls (through `_wait_for_tstate_lock`) the moment a thread has
  finished. So the runners stopped exactly as designed, logged `stopped`, and then the join that
  reaped them raised `TypeError: 'Event' object is not callable`. The worker exited 1 and systemd
  marked the unit `failed` on every single `systemctl stop`, for as long as the code has existed.

  The cost was diagnostic, not operational: a graceful shutdown was indistinguishable from a crash,
  in a unit that also gets OOM-killed for real. Renamed to `_stop_event`; `stop()` and the pacing
  sleep are unchanged. Found on the live box, tearing the stack down.

  Reproduces on **Python 3.12 and earlier only** — CPython 3.13 removed `Thread._stop`, so on a newer
  interpreter the shadowing is inert. The box runs 3.12. This is also why the guard cannot ask the
  running interpreter what a `Thread` member is: asked on 3.13+, it finds nothing and reports success.

- **A whitespace-padded camera id could create a self-link.** `POST /api/v1/movement/links` compared
  the raw `from_camera`/`to_camera` while the insert bound the trimmed values, so
  `{"from_camera":"cam_a","to_camera":" cam_a"}` stored the `cam_a → cam_a` link the guard forbids.

### Testing

- **`heldar-movement` is driven end to end** (`crates/heldar-server/tests/movement_scope_e2e.rs`):
  every route, against credentials minted through the real `POST /api/v1/api-keys` — unscoped, scoped
  to both ends of a link, to one end, and to neither — asserting no cross-camera read or act and no
  false deny on the credential's own cameras. It records per route whether the scope filter answered
  or an unholdable capability refused, so the seven movement routes the route census could only list
  as named debt are now actually exercised.

## [0.5.0] - 2026-08-13

Closes the last of the re-audit's code blockers. **Two breaking changes need a decision before you
upgrade** — read Breaking and Upgrading first.

### Breaking

- **Sidecar plugin UIs no longer share the console's origin.** The iframe sandbox granted
  `allow-same-origin`, and plugins are served through the `/m/{id}` proxy — on the console's own
  origin — so a plugin could reach the parent DOM and call the kernel API with the operator's session
  cookie, far beyond its own minted key. An imported plugin was effectively first-party code whatever
  its manifest said. The frame now runs in an **opaque origin**: no parent DOM, no storage, and no
  session cookie on its requests. A `postMessage` host bridge mediates, confining every request to
  that plugin's own `/m/{id}/` root and forwarding only `content-type`.

  **Plugin UIs that called `/api/v1/*` directly, or touched the parent document, will stop working.**
  Move them to the bridge (a copy-pasteable shim is in the module-system guide), or fetch kernel data
  server-side in the sidecar with its own capability-scoped key.

- **Deleting a camera now requires admin, and refuses while evidence is held.** It purges recordings,
  so it is no longer a manager-level action. A camera with evidence-locked segments returns 409 with
  the count and how to release the hold.

### Security

- **Interactive media jobs have a concurrency ceiling** (`HELDAR_MEDIA_JOB_CONCURRENCY`, default 3),
  covering playback session builds, clip exports and snapshots. Each forks ffmpeg and does heavy disk
  I/O; unbounded, they starve the recorder — the one process that must never miss. A caller that
  cannot get a slot gets a 503 telling it to retry, rather than being queued indefinitely. Recording
  itself deliberately takes no permit.

- **Backup destination credentials and webhook signing secrets are sealed at rest.** They were masked
  as `***` in API responses but written to SQLite in the clear, so a stolen database or a copied DB
  backup yielded SFTP/FTP passwords, S3 secret keys and HMAC keys outright — the masking only ever
  protected a shoulder-surfer, not the file. They are sealed with the existing `HELDAR_SECRET_KEY`,
  unsealed at the point of use, migrated on boot, and covered by `rekey-secrets`.

  An unsealable credential degrades rather than corrupting: a webhook delivers UNSIGNED with an error
  logged (the alert still arrives), and a backup credential reads as unset so the destination fails
  its own validation with an actionable message. Neither hands ciphertext to a subprocess.

### Tests

- **The upgrade path is qualified in CI.** A previous release's binary boots against a fresh database
  and seeds rows through its own API; the current build then boots on that same database. It is the
  only test that exercises the migration chain against a database an older release actually wrote.
  The upgraded box must serve, not merely boot.

- **Backups are proven to restore.** Seed, online snapshot via `backup-db`, destroy the database and
  its WAL/SHM, restore, verify. A control boot on the wiped directory asserts zero rows first —
  without it, a restore that silently did nothing would still pass.

- **`apps/web` gains a unit lane** (vitest, CI-gated). It had none; it guards the sidecar bridge's
  containment check, which decides what a sandboxed plugin may ask the host to fetch.

### Upgrading

- **Plugin authors must migrate** — see Breaking. This is the only change that requires action from
  anyone other than an operator.
- **Nothing needs re-configuring.** Existing credentials are sealed automatically on first boot when
  `HELDAR_SECRET_KEY` is set; without a key, behaviour is unchanged (plaintext at rest, as before).
- **Rotate keys with `heldar-core rekey-secrets`**, which now covers webhook and backup credentials as
  well as camera passwords. A rotation that moved only camera passwords would leave the others sealed
  under a retired key, and webhooks would silently start delivering unsigned.
- **Gate operators:** the v0.4.0 note still applies — a below-threshold ANPR commit-on-prune records a
  guard-review event instead of opening the barrier.

## [0.4.1] - 2026-08-12

### Fixed

- **The container image builds again.** The Rust builder base was pinned to 1.85.1 to match the
  workspace MSRV, but the locked dependency tree moved past it — `icu_*` 2.2 requires rustc 1.86 —
  so the v0.4.0 image build failed and no `0.4.0` images were published. The base now tracks what the
  build actually needs rather than the MSRV. CI builds with `stable`, so the two toolchains disagreed
  and nothing caught it until a tag ran the image build, which is the only job using that base.
  v0.4.0's crates and musl binaries are unaffected and remain published; only the container images
  were missing, and they ship as `0.4.1`.

## [0.4.0] - 2026-08-12

A security and production-qualification release. Two external audits drove it, and most of the work
is closing gaps they found rather than adding features. **Read "Breaking" and "Upgrading" before
deploying** — the authorization model changed, one gate behaviour changed, and the published kernel
API lost items.

### Breaking

- **`Principal` carries capabilities and a camera scope, and `can_view()` is gone.** It returned
  `true` for every authenticated principal, so a machine credential could read cameras, footage,
  playback and search. It was deleted rather than narrowed, so the compiler enumerated all 85 call
  sites instead of leaving a half-applied check that reads as protection it is not. Downstream
  compositions that construct a `Principal` or call `can_view()` must be updated.
- **`net_guard::egress_client` is removed**, and `pinned_egress_client` / `resolve_validate_pin`
  return `Result` instead of falling back to a default client. The fallback was fail-OPEN: it handed
  back a redirect-following, unpinned client at exactly the moment the guard failed to build.
- **A below-threshold ANPR commit-on-prune no longer opens the barrier.** It still writes the entry
  event — the audit record is the point of commit-on-prune — but marks it for guard review. Without
  this, a single accepted plate read still opened the gate ~30s later on prune, so hardening the vote
  path alone left the actuation capability intact. **Sites where vehicles pass quickly will see
  barriers stop auto-opening for reads that never reached `min_votes`.**

### Security

- **Capability-scoped machine credentials.** API keys carry an explicit capability grant, an optional
  camera scope and an expiry, instead of a role alone. Keys minted before this release keep exactly
  today's reach under the default tier; `HELDAR_MACHINE_AUTH=enforce` narrows the `integration` role
  to what a real AI worker calls. No key is bricked and none needs re-minting.
- **AI ingest is bound to a server-issued lease and per-frame ticket**, so the kernel derives the
  camera, task type and frame id from the ticket rather than trusting the request body.
- **`source = "camera_native"` can no longer be asserted through the ingest API.** Provenance is a
  parameter of the ingest path and the attributes blob is rewritten before insert, so the value the
  barrier treats as authoritative is server-authored on every path — including the crash-replay
  fan-out, which bypasses the HTTP handler entirely. This rewrite is unconditional in every tier.
- **The recorded-media plane is authorized, not just authenticated.** `/media/*` resolved a principal
  and discarded it, so any authenticated credential read every recording, clip, snapshot and backup
  archive. Each subtree now requires its capability, archives are admin-only, and recordings enforce
  camera scope.
- **Server-initiated egress resolves and pins DNS.** Only literal-IP hosts were validated, so a
  hostname resolving to loopback/RFC1918/`169.254.169.254` passed and connected. Every A/AAAA record
  is now validated and the validated addresses pinned into the client. `EgressPolicy::PUBLIC` became
  an allowlist of globally routable addresses — "not private" still admitted CGNAT, benchmarking,
  documentation and reserved space.
- **The sidecar reverse proxy is guarded per request**, not only at registration, with a bounded
  response and origin-authority headers (`Set-Cookie`, HSTS, `Access-Control-Allow-*`) stripped.
- **Empty `HELDAR_CORS_ORIGINS` means same-origin only.** It meant allow-ANY, while the production
  example shipped it empty and documented it as same-origin — so following the production template
  allowed every origin, and the strict-prod guardrail never caught it because it only looked for `*`.
- **Ingest dedup keys are namespaced by provenance**, so a client cannot claim a kernel producer's
  `(camera_id, frame_id)` and have the genuine camera-native read swallowed as a redelivery.
- **Auth-off on a non-loopback bind warns loudly**, and `HELDAR_DEPLOYMENT_MODE=production*` turns it
  into a boot refusal.

### Fixed

- **Live view works behind TLS.** MediaMTX serves HLS/WebRTC on plaintext ports and the kernel
  rewrote only the host, so an HTTPS dashboard handed the browser `http://host:8888/…` — blocked as
  mixed content. With `HELDAR_MEDIA_SAME_ORIGIN` the kernel emits origin-relative URLs and the
  reverse proxy routes them.
- **Multi-worker AI deployments no longer collapse to one node.** Lease acquisition had no shard, so
  the first worker took every task and renewed it indefinitely. Leasing now uses the same assignment
  task discovery hands out.
- **Playback covers the whole selected minute.** The `to` control is minute-granular, so a `to` of
  "now" floored to `:00` and excluded footage recorded in the current partial minute — reporting no
  footage while the recorder was actively writing.
- **An empty playback window is reported as "no footage", not "failed to open"**, so a quiet window
  no longer reads as a malfunction.
- **The from-source quickstart works in any clone.** `scripts/run_stack.sh` resolved its paths from a
  hardcoded `/home/soh/cctv`, so for everyone else it started nothing — and, having no precondition
  checks, still printed `stack up: …` and slept for 30 minutes. It now resolves paths relative to the
  script, honours `HELDAR_DATA_DIR`, and fails loudly on a missing binary, MediaMTX, or dashboard
  deps. The same hardcoded root is gone from `smoke_web.sh` and every `validate_*.sh`, whose reports
  now land in the repo's `data/`; the camera they exercise is overridable with `CAM=`.

- **`scripts/setup_mediamtx.sh` runs outside Linux/x86.** It parsed the release tag with `grep -oP`
  (GNU-only — it failed outright on macOS/BSD) and always downloaded `linux_amd64`. It now detects
  OS/arch (linux + darwin; amd64/arm64/armv7/armv6) and parses the tag portably. `MEDIAMTX_TAG=`
  pins a release.

- **Synthetic-camera harnesses publish after the core, not before.** Since publish authorization moved
  to the kernel (`authMethod: http` → `/internal/mediamtx-auth`), starting an ffmpeg publisher before
  heldar-core is up gets a 401 and the publisher exits immediately — so `validate.sh`, `smoke_web.sh`
  and the Playwright `e2e_stack.sh` were all exercising cameras that never streamed. The publishers now
  start after the API is healthy, and `validate.sh` aborts if its camera dies.

- **The Playwright e2e stack actually records.** `e2e_stack.sh` runs the core on `:8011`, but
  `mediamtx.yml` pins the kernel auth callback to `:8000`, so MediaMTX asked a dead port and denied
  every publish *and* read. It now starts MediaMTX from a port-adjusted copy of the config. Two further
  portability fixes: `wait "${PIDS[-1]}"` needs bash ≥ 4.3 and aborted under `set -u` on macOS's bash
  3.2 (it now waits on the core PID), and the `fuser -k` port cleanup falls back to `lsof` where
  `fuser` has no `-k`.

### Added

- **A TLS reference deployment** (`deploy/compose.tls.yml` + `Caddyfile`), with Let's Encrypt and LAN
  self-signed modes, and cleartext services pinned to loopback.
- **CI runs the integration suites**: Playwright (HTTP and a second HTTPS + auth-enabled stack),
  synthetic-camera media validation, RBAC, and ingest-provenance validation at both enforcement
  tiers — plus npm audit, pip-audit, gitleaks and Trivy.
- **Container bases, MediaMTX and Caddy are pinned to tag + digest**, with a supply-chain policy in
  `docs/SUPPLY-CHAIN.md` and dependabot covering the docker/compose/pip ecosystems.

### Documentation

- Docs no longer describe the retired generated-tree model. `LICENSING.md`, `DESIGN-PRINCIPLES.md` #8,
  the open-core and module-system pages (plus their `es`/`zh-Hans` translations), `REMOTE-ACCESS.md`,
  `PRODUCTION.md` and the `heldar-server` composition-root comments said the public repo was *generated
  from a private monorepo* and that `main.rs` was substituted per build — which contradicted
  CONTRIBUTING and would have told a contributor their PR gets regenerated away. They now describe the
  real model: this repo is the source of truth, and a private product composes its own binary against
  `heldar_server::run(impl Verticals)`.
- **Architecture decision records are published** under [`docs/adr/`](docs/adr): ADR 0003 (remote
  viewing over WebRTC; retiring the mobile app and kernel-managed WireGuard) and ADR 0004 (edge nodes
  stay on SQLite rather than porting to Postgres). Shipped code and docs cited these as the design
  records while the directory did not exist here, so `ARCHITECTURE.md` §21 and `REMOTE-ACCESS.md` now
  link to a document a reader can actually open. Decisions that are wholly about the commercial tier
  stay unpublished, and the index says so.
- `services/embeddings.rs` credited the no-vector-DB/no-ANN choice to ADR 0004, which is the
  SQLite-versus-Postgres brief; no ADR records that decision, so the rationale now stands on its own.
- CONTRIBUTING notes that `issue #NN` references predating 2026-07 point at the tracker used before
  development moved into the open, and do not match this repo's issue numbers.
- README/CONTRIBUTING dev setup now includes the dashboard `npm ci` step that `run_stack.sh` requires,
  and the README no longer describes the production overlay as switching to a private image (it is an
  open hardening overlay).

### Upgrading

- **Nothing is required to keep working.** Existing API keys, the auth-disabled LAN default, and
  ticketless AI workers all behave as before.
- **`HELDAR_INGEST_PROVENANCE` defaults to `warn` and is deliberately NOT promoted by
  `HELDAR_DEPLOYMENT_MODE`.** Requiring frame tickets is a client protocol change: enforcing it
  rejects every worker that does not yet mint one. Run `warn` (it names, once per hour per
  credential, exactly who would break), upgrade those workers, then set `enforce`.
- **`HELDAR_MACHINE_AUTH` does auto-promote to `enforce` under `HELDAR_DEPLOYMENT_MODE=production*`.**
  It is server-side only, and the enforced expansion keeps every endpoint a real AI worker calls.
- **Gate operators:** see the commit-on-prune change under Breaking before upgrading a barrier site.

## [0.3.1] - 2026-07-31

### Features

- **Zone-aware semantic retrieval** (#77): `POST /api/v1/search/semantic` gains an optional
  `zone` filter — *"red car in the patio zone"* — ranking only crops whose bbox ground point
  falls inside the zone's polygon, using the zone engine's exact containment semantics
  (geometric per-candidate test during the scan; deliberately NOT a `zone_events` time-window
  join, which would mis-attribute across the embedding task's separate track-id space). The zone
  pins its camera; conflicts are a 400. The Semantic tab gains a per-camera Zone select, the
  response echoes the zone, the search-log plan snapshot records it (and `planner::sanitize`
  clears the field on the structured/NL paths so it can never silently no-op there).

- **Cross-camera movement candidates gain an optional appearance signal** (#51). When an `embedding`
  AI task is indexing crops, each proposed vehicle ReID candidate now carries an additive
  `appearance_score` — the cosine similarity of the two appearances' CLIP crops, from the kernel
  embeddings store, via the new `services::embeddings::appearance_similarity` seam (temporal-spatial
  join, ≥2 vectors per side, best-pairwise-cosine within a shared model). It is a secondary signal
  shown next to the plate-anchored score — never fused into the ranking, absent (not zero) when
  embeddings don't exist, and off unless `HELDAR_MOVEMENT_APPEARANCE_SCORING=true`. Person appearance
  stays out of scope (the class is not embedded by default).
- **Semantic search polish** (#53): the Semantic tab exposes the `label` filter the API already
  accepted; the default CLIP model becomes `ViT-B-32-quickgelu` (open_clip's recommended pairing for
  the `openai` tag) on both the analyzer and the query worker — the emitted model id becomes
  `open_clip/ViT-B-32-quickgelu/openai`, so operators upgrading re-index by letting the stride
  repopulate as old vectors age out; and the semantic response omits the `detection` field when a hit
  has no correlated detection row rather than emitting `null`.

## [0.3.0] - 2026-07-17

### Docs & community

- **Production-readiness sweep** (docs, UI/UX, open repo). A seven-surface audit + fix pass: updated every
  doc that drifted from the kernel-owned live transcode / `live_warm` / runtime transcode-engine setting /
  worker-`?worker_id=`-sharding / per-box-enrollment / versioned-app-migration / `*_read`-contract-view
  changes (README, ARCHITECTURE, ROADMAP, PRODUCTION, REMOTE-ACCESS, OBSERVABILITY, AI-WORKERS,
  ACCESS-CONTROL, the env templates, and all three website locales — en/zh-Hans/es); replaced the dead
  `HELDAR_ALERT_WEBHOOK_URL` guidance with webhook subscriptions and swept renumbered migration paths.
  UI: role-gated the last ungated camera/AI/zone controls for viewers, made `AuthGate` distinguish a Core
  outage from a logout, fixed a Playback spinner-forever + error-swallowing bug, keyed CameraDetail by id
  (no cross-camera stale state), and replaced `window.prompt` incident tagging with an inline datalist.
  Open repo: added `SECURITY.md`, `CODE_OF_CONDUCT.md`, issue/PR templates; split the docker prod overlay
  into an open hardening overlay + a private full-image overlay; the appliance image now builds and serves
  the dashboard (`HELDAR_WEB_DIR`); hardened the open-repo generator's leak gates (Makefile + root files in
  scope, `campus[-.]`/`release.sh`/`docs/agents` handled) and its allowlist. ROADMAP gained a
  "Post-Stage-7 platform work" section, and the genuinely-open items are now tracked as GitHub issues.

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
  pinned. The ffmpeg-child lifecycle is now covered too: two integration tests drive `run_supervise`
  against a fake ffmpeg (a missing binary → `camera_status.state='error'`; `false` as a crashing child →
  `reconnect_count` bump + a `camera_offline` event), closing the last recorder gap.

### Added

- **CLIP semantic retrieval — search by free text or image (#38).** The Search module gains a
  **Semantic** tab: describe a moment ("red pickup truck") or drop in an image and get back
  similarity-ranked detection crops (score, label, camera, timestamp, crop thumb) with a one-click
  Playback deep-link (±60 s around the hit — Playback now auto-opens from URL params). Under it: a new
  `embedding` worker task (own YOLO+ByteTrack; vehicles by default, the person class deliberately
  excluded) embeds tracked detection crops through open_clip (ViT-B/32) on a per-track stride —
  with static-object suppression (`static_suppression`/`static_epsilon`/`static_refresh_seconds`,
  mirroring the zone engine) so a parked car doesn't accrete near-identical vectors every stride — and
  POSTs them to a new kernel store (`POST /api/v1/ai/embeddings`, migration `0010`, idempotent per
  (camera, frame, track), crop thumbs served from `/media/snapshots/`); query vectors ride a
  **pull-only** `embed_queries` queue the worker polls ~1 s (`GET /api/v1/ai/embed-queries` + a result
  POST — the existing tasks poll is untouched, so deployed workers keep working);
  `POST /api/v1/search/semantic` (heldar-search) awaits the vector and the kernel ranks by brute-force
  cosine (SQL camera/label/time prefilter, top-k heap, 100k-candidate scan cap surfaced as
  `truncated`), logging to the search log (never the image bytes) and auditing plate-like text queries
  like any other identity query — results are explicitly framed as **similarity-ranked, not facts**
  (the proof ladder marks the ranking as the fallible inference). Self-bounding: embeddings ride the
  detections retention TTL (crop thumbs unlinked with the rows — also on size-cap sheds and camera
  deletes), query rows are deleted when their search returns (max 16 in flight; excess enqueues get a
  retryable 503), and the DB size cap sheds transient query rows, then embeddings, then detections
  (disposable data first). The
  CLIP stack is an optional extra (`apps/ai/requirements-embed.txt`); without an embedding-capable
  worker running, embedding tasks degrade to the safe placeholder and semantic search answers
  `503` + `Retry-After` ("embedding worker offline") instead of pretending.
- **Per-camera "Keep live warm" (`live_warm`, migration `0006`).** A warm camera's live publisher runs
  persistently instead of on-demand, so live view starts instantly — the product replacement for
  box-side warming scripts (no more editing an `.sh` to change which cameras are warm). Dashboard: a
  "Warm live" toggle on the camera page + an Add-Camera checkbox; API: `live_warm` on the camera
  create/update surface. The reconcile loop boots warm cameras at startup and self-heals MediaMTX
  restarts.
- **Live-transcode engine is now a runtime setting.** `GET/PUT /api/v1/system/transcode` (admin-gated,
  audited) overrides the `HELDAR_LIVE_TRANSCODE_ENGINE` env default via the settings table (same
  precedence as the disk caps); the System page gains a "Live transcode" panel with a software/vaapi/
  nvenc picker and device-node hardware detection. Running publishers switch engines within ~30s (the
  reconcile loop restarts drifted configs); `/api/v1/system` now reports the effective engine.
- **AI worker task sharding (multi-worker per node).** A single node ran one worker; two workers both
  pulled every task from `/ai/tasks` and redid the same inference (N× GPU for 1× throughput). Workers now
  send a stable `?worker_id=` (the reference `worker.py` defaults to `<hostname>:<pid>`, override with
  `HELDAR_AI_WORKER_ID`), which doubles as a liveness heartbeat (`ai_workers` table, migration `0004`); the
  kernel returns only that worker's deterministic modulo shard of the tasks, so launching N workers on one
  host splits the load. A worker that stops polling for >60s is dropped and its tasks reassigned. Fully
  backward-compatible: omit `worker_id` (a single/legacy worker) and you get the whole list; a new worker
  against an old kernel is unaffected. Rebalance overlap is deduped by the outbox `frame_id`.
- **Cross-app read seam (contract views).** App crates used to read each other's SQLite tables via raw
  SQL on the shared pool with no compiler-visible dependency, so a column rename in the producer silently
  broke a distant consumer at runtime. Now the owning app publishes a stable `*_read` SQL view
  (`entry_events_read` owned by entry; `breach_alerts_read` owned by movement) exposing exactly the
  columns peers may read, and all five cross-app reads (search→entry_events, search→breach_alerts,
  movement's reid self-join + plate-history + breach.rs) go through the views. A base-column rename is a
  producer-local migration that redefines the view; each producer ships a `tests/read_contract.rs`
  drift-guard so the break surfaces in the OWNER's CI (same PR) instead of at runtime in a consumer. Zero
  new Cargo edges (the "apps depend only on the kernel" principle is preserved), views are Postgres-
  portable, and `scripts/check-read-seam.sh` (wired into CI) forbids a consumer from reading a peer's
  base table directly. See DESIGN-PRINCIPLES #9. (Chosen over inter-app crate deps, which couldn't
  express the cross-owner self-join and would have broken the dependency-graph principle.)
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

- **Live view: the kernel now owns the preview transcode (MediaMTX `runOnDemand` removed).** The live
  HEVC→H.264 preview relied on MediaMTX exec'ing an ffmpeg command (`runOnDemand`) — but the recommended
  docker-compose deployment runs the official `bluenviron/mediamtx` image, which ships **no ffmpeg**, so
  on-demand live view silently never worked there (the live box masked it with a hand-rolled host-side
  warming script). The publisher ffmpeg is now spawned and supervised by the kernel itself
  (`services/live_publisher.rs`, mirroring the recorder manager: structured argv, bounded stderr, restart
  with backoff, restart-on-config-drift, teardown on disable/delete), publishing to a **plain** MediaMTX
  path — MediaMTX never execs anything, so live view works identically in docker, native, and mixed
  topologies. Legacy `runOnDemand` path configs are patched clean on contact. On-demand publishers are
  reaped only when MediaMTX confirms zero readers and no demand for `HELDAR_LIVE_IDLE_CLOSE_SECS` (60s).
  `liveview` now also refuses disabled cameras (mirrors the remote-bridge guard) and waits (bounded ~8s)
  for the stream to become ready so the first player request succeeds. All per-camera mutators (the
  PATCH/DELETE hook, the reconcile loop's step, viewer demand) are serialized on a per-camera lock and
  re-read the row inside it, eliminating the hook-vs-loop TOCTOU entirely; an engine change pokes an
  immediate reconcile pass, so running publishers switch within seconds.

- **Remote grid: disabled/offline cameras are marked unavailable, not streamed.** Opening a disabled or
  down camera in the remote multi-camera grid used to start a WHEP session that could only 404 (no
  publisher), producing a burst of failed requests. The box now advertises each camera's `enabled` + `state`
  in the rendezvous catalog (`camera_catalog`, mirroring the health routes' disabled-override), the `heldar`
  Worker forwards those fields, and the grid renders an "unavailable / disabled" tile without opening a
  session for cameras that can't stream. Defense-in-depth: `bridge_to_local_whep` also refuses to bridge a
  disabled camera. Backward-compatible — an older box that advertises only `id`+`name` is treated as
  streamable (unchanged). Both ends must be deployed for the tile to appear (box rebuild + Worker deploy).
- **`consumer_fanout` retention index (perf).** Added an index on `consumer_fanout(fanned_at)` (migration
  `0005`) so the retention prune (`DELETE … WHERE fanned_at < ? LIMIT N`, `delete_aged_in_batches`) is an
  index range scan, not a full-table scan. Without it, pruning a large fan-out backlog held the single
  SQLite writer for seconds per batch and starved live writes (recording/camera_status/detection inserts).
  Surfaced on the live box: ~1.9M rows, retention batches taking 2–15s each and a 15s `camera_status` stall;
  the index turned the delete subquery into a covering-index scan and the stalls disappeared. Complements
  the fan-out retention fix below (that stopped the *leak*; this makes *pruning it* cheap).
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
