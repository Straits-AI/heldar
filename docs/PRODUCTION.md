# Production hardening & security checklist

Heldar ships two postures from the same binary:

- **LAN appliance (default):** a trusted single network. Auth is off, the session cookie isn't `Secure`,
  camera credentials are plaintext at rest. These defaults optimise zero-friction onboarding and are
  appropriate when only trusted operators can reach the box.
- **Internet-exposed (this doc):** reachable from the public internet via the WebRTC rendezvous. The
  permissive LAN defaults are now a liability, so you harden by **configuration** — the code does not
  change behaviour, you opt in. Start from [`.env.production.example`](../.env.production.example).

The kernel **fails loud** to stop you shipping an unsafe internet deployment: when a rendezvous is
configured (`HELDAR_REMOTE_RENDEZVOUS_URL`) it refuses to boot with auth off, and warns (or, under
`HELDAR_STRICT_PROD=true`, refuses) on a non-`Secure` cookie, no idle timeout, an over-long session TTL,
a localhost CORS allowlist, or plaintext camera credentials.

## Kernel checklist

| Control | Var | LAN default | Production | Why |
| --- | --- | --- | --- | --- |
| Require auth | `HELDAR_AUTH_ENABLED` | `false` | **`true`** | Off = every request is a synthetic admin. Boot is refused if a rendezvous is set while off. |
| TLS cookie | `HELDAR_AUTH_COOKIE_SECURE` | `false` | **`true`** | Sends the session cookie only over HTTPS. |
| Session lifetime | `HELDAR_SESSION_TTL_HOURS` | `12` | **`4`** or less | Bounds a stolen token's absolute window. |
| Idle timeout | `HELDAR_SESSION_IDLE_TIMEOUT_MIN` | `0` (off) | **`30`** | Expires an unused session before its TTL. |
| Brute-force lockout | `HELDAR_LOGIN_MAX_FAILURES` / `_LOCKOUT_MIN` | `5` / `15` | keep | Locks an account after N consecutive failures (per-account; complements the rendezvous per-IP limit). Admin clears via `POST /api/v1/users/{id}/unlock` or any user edit. |
| Credential encryption | `HELDAR_SECRET_KEY` | unset (plaintext) | **set** | base64 of 32 bytes (`openssl rand -base64 32`). Camera passwords are sealed with AES-256-GCM; existing plaintext rows are sealed on next boot. A wrong/missing key fails loud — ciphertext is never fed to ffmpeg. |
| CORS | `HELDAR_CORS_ORIGINS` | `localhost:5173` | **lock** | Empty (same-origin) or the dashboard origin only. |
| Strict mode | `HELDAR_STRICT_PROD` | `false` | **`true`** | Turns the guardrail warnings above into hard boot failures. |

## Rendezvous Worker (`apps/edge`) checklist

Set these as Cloudflare secrets (`wrangler secret put <NAME>`):

- `BOX_TOKEN` — the bearer the box presents (`== HELDAR_CP_TOKEN`). Box auth is **fail-closed** without it.
- `RENDEZVOUS_SECRET` — signs viewing tickets.
- `RELAY_CAP_SECRET` — signs dashboard relay capabilities (a separate key).
- `TURN_API_TOKEN` — Cloudflare Realtime TURN credential minting.
- `TURNSTILE_SECRET` *(optional)* — enables a Cloudflare Turnstile bot challenge on the dashboard login.
  Pair with the public `TURNSTILE_SITE_KEY` var (also passed to the dashboard build as
  `VITE_TURNSTILE_SITE_KEY`); unset = no challenge.

**Never set `ALLOW_OPEN_BOX_AUTH`** — it is a dev-only escape hatch that opens box auth when `BOX_TOKEN`
is unset. The Worker logs a loud warning if it is ever active.

## Network & data

- **Camera segmentation:** keep cameras on an isolated VLAN reachable only by the box. RTSP credentials
  travel in the URL (standard RTSP); prefer RTSPS where the camera supports it.
- **Disk:** recordings are bounded (`HELDAR_MAX_RECORDINGS_GB` + `HELDAR_MIN_FREE_DISK_GB`, runtime-tunable
  via `PUT /api/v1/system/retention`) so they can't fill the disk. See [`sizing.md`](sizing.md).
- **Backups:** the SQLite DB holds sealed camera credentials (with `HELDAR_SECRET_KEY` set) and the audit
  log — back it up encrypted, and store `HELDAR_SECRET_KEY` separately from the DB.

## Further hardening (not yet built in)

These are deliberately out of the current scope — track them for higher-assurance deployments:

- At-rest encryption of **recorded footage** (segments are stored unencrypted; rely on disk/volume
  encryption — LUKS/BitLocker — today).
- A pluggable external **secret-store** backend (Vault / cloud secrets) for `HELDAR_SECRET_KEY` and camera
  credentials, instead of an env var + DB column.
- RTSPS **enforcement** and audit-log **retention** tuning for privacy/compliance regimes.
