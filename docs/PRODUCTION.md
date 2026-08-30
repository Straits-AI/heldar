# Production hardening & security checklist

Heldar ships two postures from the same binary:

- **LAN appliance (default):** a trusted single network. Auth is off, the session cookie isn't `Secure`,
  camera credentials are plaintext at rest. These defaults optimise zero-friction onboarding and are
  appropriate when only trusted operators can reach the box.
- **Internet-exposed (this doc):** reachable from the public internet via the WebRTC rendezvous. The
  permissive LAN defaults are now a liability, so you harden by **configuration** — the code does not
  change behaviour, you opt in. Start from [`.env.production.example`](../.env.production.example).

**Docker:** `deploy/compose.prod.yml` bundles the kernel knobs below (auth on, `Secure` cookie, strict
mode, short sessions) on top of the private full image — `docker compose -f deploy/compose.yml -f
deploy/compose.prod.yml up -d` after `docker login ghcr.io`. Put `HELDAR_SECRET_KEY` +
`HELDAR_CORS_ORIGINS` in `.env`. A flashed DVR/appliance uses native systemd instead (not Docker).

**Pin the release (#112).** Every tagged release ships `heldar-release-manifest.json`, which records
the source commit, the migration ceiling those binaries actually carry, the image digests and the
sha256 of every deployment file. Download it from the release — not from `main` — and verify before
and after an upgrade:

```bash
gh release download "$V" -p heldar-release-manifest.json
gh attestation verify heldar-release-manifest.json --repo Straits-AI/heldar   # it is signed too
HELDAR_DB=/var/lib/heldar/heldar.db ./scripts/verify_release_manifest.py heldar-release-manifest.json
```

The verifier fails closed on a modified deployment file and — the one that matters on an upgrade — on
a database written by a NEWER release than the binaries you are about to run. Migrations only go
forward, so an older binary opening a newer schema is not a clean refusal, it is a binary reading a
database it does not understand. Check this **before** stopping services, not after.

The floating quickstart (`latest`, files from `main`) stays exactly as it is; it is the right thing
for evaluation and the wrong thing for a recorder.

**Keep secrets out of the environment (#126).** Every deployment secret resolves from the first
source that supplies it:

| Order | Source | Notes |
|---|---|---|
| 1 | `NAME` | the environment variable, unchanged — nothing moves on upgrade |
| 2 | `NAME_FILE` | a path whose contents are the secret; what Docker/Compose/Kubernetes secrets already produce |
| 3 | `$CREDENTIALS_DIRECTORY/NAME` | a systemd credential, via `LoadCredential=` — 0400 in tmpfs, readable only by the service |

Applies to `HELDAR_SECRET_KEY`, `HELDAR_SECRET_KEY_OLD`, `HELDAR_BOOTSTRAP_ADMIN_PASSWORD` and
`HELDAR_SMTP_PASSWORD` — the variables that hold a secret VALUE.

`HELDAR_CP_TLS_CLIENT_KEY` is deliberately **not** in that list: it holds a *path*, and `_FILE`
resolution would substitute the key's contents where a filename belongs. A systemd credential is
already a path (`$CREDENTIALS_DIRECTORY/NAME`), which that variable accepts as-is.

```yaml
# compose: the master key as a file rather than an env var
services:
  core:
    environment:
      HELDAR_SECRET_KEY_FILE: /run/secrets/heldar_master_key
    secrets: [heldar_master_key]
secrets:
  heldar_master_key:
    file: ./secrets/master.key      # (umask 077; openssl rand -base64 32 > secrets/master.key)
```

```ini
# systemd: no secret in the unit file, and none in the environment
[Service]
LoadCredential=HELDAR_SECRET_KEY:/etc/heldar/master.key
```

**Why bother, precisely.** An environment variable is readable from `/proc/<pid>/environ` by anyone
who can read it, and appears in `docker inspect` output, shell history and crash dumps. A file
narrows that to whoever can read a file the service user owns — so create it with `umask 077`, or
the default umask leaves it world-readable and you have moved the exposure rather than closed it.

**What this does not fix.** It does not hide anything from root, and nothing at this layer can. It
also does **not** stop a child process reading the secret: `HELDAR_SECRET_KEY_FILE` is itself
inherited, and a same-uid child can simply open the path. The credentials-in-ffmpeg-argv exposure is
a separate problem and is still open — see #126.

**A named source that cannot be read is fatal at boot**, deliberately. An operator who set
`HELDAR_SECRET_KEY_FILE` asked for encryption at rest; falling through to "no key" would store every
camera credential in plaintext while the deployment believed they were sealed, and the failure would
stay invisible until someone read the database. The trailing newline your editor adds is trimmed;
an empty file is an error, not a choice to use no key.

**No HTTP secret provider, on purpose.** A recorder must not become synchronously dependent on a
remote service after boot — a network blip during a reconnect would stop recording. A sidecar that
writes the secret to a path is already supported by source (2), and that is the integration shape
for Vault, cloud secret stores or a customer KMS.

**Container hardening:** add `deploy/compose.hardened.yml` as a third overlay —
`-f compose.yml -f compose.prod.yml -f compose.hardened.yml`. `compose.prod.yml` hardens the
*application* posture (auth, cookies, sessions); this hardens the *container*: read-only root
filesystems, all capabilities dropped, `no-new-privileges`, PID/CPU/memory ceilings, bounded logs and
explicit tmpfs mounts. The stack runs `network_mode: host` for camera discovery, multicast and WebRTC,
which removes the network namespace as a boundary and makes these process-level ones matter more.

Two services are deliberately **not** read-only. MediaMTX
generates a self-signed TLS keypair into its working directory at startup, and the AI worker
downloads model weights on first use — the MediaMTX and nginx exceptions come from observed boot
failures; the AI one is reasoned from how that image resolves `HOME` and has not been exercised
through a real model download. nginx keeps exactly three capabilities (`CHOWN`, `SETUID`,
`SETGID`) because its entrypoint chowns its cache dirs before dropping to the worker user — not
`NET_BIND_SERVICE`, since it listens on :8080.

Verified by booting mediamtx, core and web: core healthy with a read-only rootfs and `/data` still
writable, rootfs writes refused, the dashboard served, and the web→core proxy answering. The `ai`
profile was not booted — it is opt-in and pulls model weights — so treat its settings as reasoned
rather than proven.

The kernel **fails loud** to stop you shipping an unsafe internet deployment. It treats the box as
internet-exposed when **any** remote path is configured — the WebRTC rendezvous
(`HELDAR_REMOTE_RENDEZVOUS_URL`), an overlay network (`HELDAR_OVERLAY_ENABLED`), or control-plane
self-registration (`HELDAR_CP_URL` + `HELDAR_PUBLIC_BASE_URL`) — and then **refuses to boot with auth
off**. It warns (or, under `HELDAR_STRICT_PROD=true`, refuses) on a non-`Secure` cookie, no idle timeout,
an over-long session TTL, a localhost **or wildcard (`*`)** CORS allowlist, plaintext camera credentials,
or an empty dial-out bearer (`HELDAR_CP_TOKEN`) while a rendezvous is configured.

## Deployment-mode ladder

The same binary spans four postures. Pick the one that matches how the box is reachable, and configure
to its row. The ladder is a **guide**, not a switch: the defaults below are what ship today, and moving
up the ladder is done by setting the vars, not by flipping a single mode flag.

| Mode | Auth | Bind (`HELDAR_API_HOST`) | Reachable from | Extra hardening | Boot behavior |
| --- | --- | --- | --- | --- | --- |
| **DEV** | off | `127.0.0.1` (loopback) | this host only | none needed | boots quietly |
| **COMMISSIONING** | off | `0.0.0.0` (LAN) | the whole LAN | *temporary* — enable auth before you rely on the box | boots with a **loud boxed WARNING** (see below) |
| **PRODUCTION-LAN** | **on** | `0.0.0.0` (LAN) | the whole LAN | `HELDAR_AUTH_COOKIE_SECURE=true` + HTTPS in front, `HELDAR_SECRET_KEY` set (encrypted creds) | boots clean |
| **PRODUCTION-REMOTE** | **on** | `0.0.0.0` behind the rendezvous/overlay | the internet | all of PRODUCTION-LAN **+** TLS, `HELDAR_STRICT_PROD=true`, locked CORS, short sessions, idle timeout | auth-off **refuses to boot**; strict mode turns the soft warnings into refusals |

### The LAN-trust startup warning (COMMISSIONING)

With **auth off** and the API bound to a **non-loopback** address (`0.0.0.0`, a LAN IP — anything other
than `127.0.0.1` / `::1` / `localhost`), the box trusts **every device on the LAN as full admin**: any
compromised camera, IoT gadget, or laptop on the same network reaches the NVR API with admin rights and
no login. This is the intentional first-run/commissioning posture, so the box still **boots** — but it
now emits a prominent, boxed startup `WARN` naming the concrete risk and the exact remediation:

- **Require login:** `HELDAR_AUTH_ENABLED=true` (recommended — moves you to PRODUCTION-LAN), or
- **Local dev only:** `HELDAR_API_HOST=127.0.0.1` (loopback, no LAN exposure — DEV).

This warning is separate from the internet-exposure refusal above: it fires on a plain LAN bind with
**no** remote path configured (the case the exposure check can't see). It does **not** fire when auth is
on, nor when the bind is loopback-only.

### Optional: `HELDAR_DEPLOYMENT_MODE` (opt-in tightening)

Setting `HELDAR_DEPLOYMENT_MODE=production` (also accepts `production-lan` / `production-remote`)
**escalates** the LAN-trust warning into a **hard boot refusal**: a production box with auth off on a
non-loopback bind will not start until you set `HELDAR_AUTH_ENABLED=true` (or bind loopback). Leaving the
var **unset** (or `dev` / `commissioning`) preserves today's behavior exactly — warn-and-boot — so this
is purely opt-in and never breaks the `docker compose up -d` commissioning flow.

### Upgrading a box (prebuilt binary)

Each tagged release attaches static `heldar-core` binaries (x86_64 + aarch64). To upgrade a box in place:

    REPO=Straits-AI/heldar                 # licensed/full deployments substitute their own release repo
    ARCH=$(uname -m)                       # x86_64 or aarch64
    V=vX.Y.Z
    curl -fsSLO "https://github.com/$REPO/releases/download/$V/heldar-core-$V-$ARCH-linux-musl"
    curl -fsSLO "https://github.com/$REPO/releases/download/$V/heldar-core-$V-$ARCH-linux-musl.sha256"
    sha256sum -c "heldar-core-$V-$ARCH-linux-musl.sha256"
    systemctl stop heldar-core
    install -m 0755 "heldar-core-$V-$ARCH-linux-musl" /usr/local/bin/heldar-core   # the path your systemd ExecStart= launches — check the unit file with: systemctl cat heldar-core
    systemctl start heldar-core

(For a non-systemd install, substitute your deployment's own stop/start scripts.)

The static musl binary needs only the `ffmpeg` CLI present (it links no ffmpeg libraries). Open-core
self-hosters pull the equivalent asset from the public `Straits-AI/heldar` releases.

## Kernel checklist

| Control | Var | LAN default | Production | Why |
| --- | --- | --- | --- | --- |
| Require auth | `HELDAR_AUTH_ENABLED` | `false` | **`true`** | Off = every request is a synthetic admin. Boot is refused if a rendezvous is set while off. |
| TLS cookie | `HELDAR_AUTH_COOKIE_SECURE` | `false` | **`true`** | Sends the session cookie only over HTTPS. |
| Session lifetime | `HELDAR_SESSION_TTL_HOURS` | `12` | **`4`** or less | Bounds a stolen token's absolute window. |
| Idle timeout | `HELDAR_SESSION_IDLE_TIMEOUT_MIN` | `0` (off) | **`30`** | Expires an unused session before its TTL. |
| Brute-force lockout | `HELDAR_LOGIN_MAX_FAILURES` / `_LOCKOUT_MIN` | `5` / `15` | keep | Locks an account after N consecutive failures (per-account; complements the rendezvous per-IP limit). Admin clears via `POST /api/v1/users/{id}/unlock` or any user edit. |
| Credential encryption | `HELDAR_SECRET_KEY` | unset (plaintext) | **set** | base64 of 32 bytes (`openssl rand -base64 32`). Camera passwords are sealed with AES-256-GCM; existing plaintext rows are sealed on next boot. A wrong/missing key fails loud — ciphertext is never fed to ffmpeg. |
| CORS | `HELDAR_CORS_ORIGINS` | `localhost:5173` | **lock** | Empty (same-origin) or the dashboard origin only; a `localhost` or `*` entry is flagged. |
| Archive cap | `HELDAR_ARCHIVE_DIR_MAX_BYTES` | `50 GiB` | tune | Caps the cumulative size of on-demand `.zip` exports so they can't fill the disk and push the retention sweeper into evicting live recordings. Each export also requires free-disk headroom and shares the backup concurrency limit. |
| Strict mode | `HELDAR_STRICT_PROD` | `false` | **`true`** | Turns the guardrail warnings above into hard boot failures. |
| Deployment mode | `HELDAR_DEPLOYMENT_MODE` | unset | **`production`** | Opt-in. `production*` turns the auth-off-on-LAN startup warning into a hard boot refusal. Unset = warn-and-boot (unchanged). |

**Authentication floor (automatic).** With `HELDAR_AUTH_ENABLED=true`, a router-level middleware
rejects unauthenticated requests to the **entire `/api/v1` surface** — kernel, apps, and verticals —
before any handler runs. Only `/api/v1/auth/login` and `/api/v1/auth/logout` are reachable pre-auth;
`/healthz`, `/readyz`, and `/metrics` sit outside `/api/v1` and are unaffected. This is
defence-in-depth: a route is authenticated by default even if a handler forgets to check, so a new
endpoint cannot accidentally ship publicly. It is authentication only — each handler still enforces
its own **role** (RBAC) on top. No configuration; it follows `HELDAR_AUTH_ENABLED`.

## Rendezvous Worker (`apps/edge`) checklist

Set these as Cloudflare secrets (`wrangler secret put <NAME>`):

- `BOX_ENROLL_SECRET` — the **primary** box-auth secret: mints per-box, **site-bound** tokens
  (`cd apps/edge && npm run mint` → prints a fresh **UUID site id** + the token; set them as the box's
  `HELDAR_SITE_ID` / `HELDAR_CP_TOKEN` — the token is accepted only for its own site). Site ids are
  **opaque UUIDs by design**: they appear in dashboard URLs (`/app/?site=…`), referrers, and browser
  history, so a guessable/customer-named id enables existence-probing and targeted login attempts.
  Revoke a box by adding its `site_id` to the `REVOKED_SITES` var, or rotate the secret (revokes all →
  re-mint). See `apps/edge/README.md` (the private rendezvous Worker).

  **Renaming an existing site to a UUID:** `npm run mint` (new UUID + site-bound token) → update the
  box's `HELDAR_SITE_ID` + `HELDAR_CP_TOKEN` → restart the kernel → verify login at the new
  `/app/?site=<uuid>` → add the OLD id to `REVOKED_SITES` and redeploy the Worker (the old URL dies).
  Sessions/users are untouched (the site id only routes the rendezvous).
- `RENDEZVOUS_SECRET` — signs viewing tickets.
- `RELAY_CAP_SECRET` — signs dashboard relay capabilities (a separate key).
- `TURN_API_TOKEN` — Cloudflare Realtime TURN credential minting.
- `TURNSTILE_SECRET` *(optional)* — enables a Cloudflare Turnstile bot challenge on the dashboard login.
  Pair with the public `TURNSTILE_SITE_KEY` var (also passed to the dashboard build as
  `VITE_TURNSTILE_SITE_KEY`); unset = no challenge.

**Never set `ALLOW_OPEN_BOX_AUTH`** — it is a dev-only escape hatch that opens box auth when neither
`BOX_TOKEN` nor `BOX_ENROLL_SECRET` is set. The Worker logs a loud warning if it is ever active.

The Worker rate-limits the ticket-gated `/api/v1/rtc/*` endpoints per IP, so a leaked view link is a
**bounded** rather than unmetered TURN/transcode faucet, and it warns once if any configured secret
looks too short. Generate each secret with `openssl rand -base64 32`.

## Network & data

- **Camera segmentation:** keep cameras on an isolated VLAN reachable only by the box. RTSP credentials
  travel in the URL (standard RTSP); prefer RTSPS where the camera supports it.
- **Host firewall:** the box listens on `0.0.0.0` by design, so restrict inbound to only the ports you
  need (the API, RTSP/WHEP, SSH) from the trusted segment with a default-deny firewall. A flashed
  appliance ships LAN defaults and a first-boot login banner reminding you to do this before widening
  the network. Example (`nftables`):

  ```
  table inet filter {
    chain input {
      type filter hook input priority 0; policy drop;
      ct state established,related accept
      iif "lo" accept
      tcp dport { 22 } accept                 # SSH (management)
      tcp dport { 8080 } accept               # Heldar API (adjust to HELDAR_API_PORT)
      tcp dport { 8554, 8888, 8889 } accept   # RTSP / HLS / WHEP signaling (MediaMTX)
      udp dport 8189 accept                   # WebRTC ICE media — note: UDP, not TCP
    }
  }
  ```

- **Disk:** recordings are bounded (`HELDAR_MAX_RECORDINGS_GB` + `HELDAR_MIN_FREE_DISK_GB`, runtime-tunable
  via `PUT /api/v1/system/retention`) so they can't fill the disk; orphaned (unindexed) segment files are
  reclaimed on a slow sweep, and on-demand exports are capped (`HELDAR_ARCHIVE_DIR_MAX_BYTES`). See
  [`sizing.md`](sizing.md).
- **Backups & key rotation:** see the section below.

## Backups & key rotation

- **Database snapshot.** The SQLite DB holds users, the audit trail, camera config, and sealed camera
  credentials — and only video is otherwise backed up, so the DB is your single point of data loss. Take
  a consistent online snapshot (never a naive `cp` of a live DB, which can capture a torn file):

  ```
  heldar-core backup-db /var/backups/heldar-$(date +%F).db
  ```

  Schedule it with the bundled systemd timer (`infra/systemd/heldar-db-backup.{service,timer}`). Store the
  copy encrypted and **separately from `HELDAR_SECRET_KEY`** — the key is what keeps the sealed credentials
  in it useless to a thief.

- **Back up the evidence-signing key** (`$HELDAR_DATA_DIR/evidence-signing-key.pkcs8`, #118) with the
  appliance. Bundles already exported stay verifiable by anyone who recorded the key id, but a box
  that loses this key can no longer sign under the identity recipients have already pinned — and it
  will generate a NEW one on next boot, so the next bundle verifies against a key nobody expects.
  Unlike `HELDAR_SECRET_KEY` it is not interchangeable: it *is* the appliance's identity, not a
  configured secret. See [`docs/EVIDENCE.md`](EVIDENCE.md).

- **Rotating `HELDAR_SECRET_KEY`.** Losing or changing the key bricks recording (camera passwords fail to
  decrypt — fail-closed, by design), and restoring a DB onto a box with a different key does the same. To
  rotate, set the current key as `HELDAR_SECRET_KEY_OLD` and the new key as `HELDAR_SECRET_KEY`, then:

  ```
  heldar-core rekey-secrets
  ```

  It re-seals every camera credential from the old key to the new one (idempotent; safe to re-run).
  **If you lose the key with no `HELDAR_SECRET_KEY_OLD` to recover from, you must re-enter every camera
  password.** Keep the key in a password manager or secret store, not only on the box.

- **Converting a legacy DB to `auto_vacuum=INCREMENTAL`.** A `heldar.db` created before the storage
  size-cap feature starts in `auto_vacuum` mode `NONE`, so the size cap (`HELDAR_MAX_DB_GB`, default `4`)
  can't shrink the file even after rows are pruned. Heldar converts it once, automatically, in the
  **background** after boot (`HELDAR_DB_AUTOVACUUM_CONVERT`, default `true`) — the server binds and
  serves reads/`/healthz` immediately regardless of DB size or conversion state. The conversion is a full
  `VACUUM`: it holds a write lock for its duration (reads stay up; writes stall until it finishes), so on
  a large legacy DB you may prefer to run it during a maintenance window instead:

  ```
  HELDAR_DB_AUTOVACUUM_CONVERT=false   # skip the automatic background attempt
  # stop the server
  heldar-core convert-autovacuum       # forces the conversion synchronously (server stopped)
  # start the server
  ```

  Once a conversion completes, the size cap begins reclaiming space on the next retention sweep — not
  necessarily instantly, since pre-existing pool connections pick up the converted mode within a sweep or
  on the next restart. **If you set the flag `false` and never run the CLI, a legacy DB stays unconverted
  and the size cap cannot shrink the file.**

  You can also run the conversion **online from the dashboard** (System → Database limit → "Convert /
  reclaim", admin-only) — the same background conversion as the automatic path, no restart. Set the DB
  cap there too. The `convert-autovacuum` CLI remains the zero-contention path when you want the server
  stopped.

## Further hardening (not yet built in)

These are deliberately out of the current scope — track them for higher-assurance deployments:

- At-rest encryption of **recorded footage** (segments are stored unencrypted; rely on disk/volume
  encryption — LUKS/BitLocker — today).
- **Camera credentials in the ffmpeg command line.** Passwords are encrypted at rest
  (`HELDAR_SECRET_KEY`), but the recorder/sampler/publisher decrypt them at spawn time and pass the
  RTSP URL (`rtsp://user:pass@host/…`) as an ffmpeg `-i` argument, so a **local shell user on the
  appliance** can read them from `ps`/`/proc/<pid>/cmdline` while a stream is active. ffmpeg has no
  separate credential option, and its only file/stdin input path (the `concat` demuxer) drops the
  `-rtsp_transport tcp` the recorder relies on — there is no ffmpeg-native fix that preserves the
  streaming semantics. Mitigate at the OS level on multi-user hosts: mount `/proc` with
  `hidepid=2` (e.g. `/etc/fstab`: `proc /proc proc defaults,hidepid=2 0 0`) so a user can't read
  another user's `cmdline`. Single-operator appliances (the default posture) are unaffected — there
  is no second unprivileged local account to read the argv.
- A pluggable external **secret-store** backend (Vault / cloud secrets) for `HELDAR_SECRET_KEY` and camera
  credentials, instead of an env var + DB column.
- RTSPS **enforcement** and audit-log **retention** tuning for privacy/compliance regimes.
