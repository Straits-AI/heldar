# TLS reference deployment

A self-contained HTTPS front for a Heldar box, so the secure path is one command. This adds a
[Caddy](https://caddyserver.com) reverse proxy that terminates TLS on `:443` and proxies to the
existing dashboard — with **automatic Let's Encrypt** for a public domain, or a **self-signed cert**
for a LAN-only box. No manually-managed certificates, no separate operator proxy.

It layers on top of the production overlay. `compose.prod.yml` turns on the hardened posture (auth
required, `Secure` session cookie, strict-prod boot guardrails, short sessions) but terminates **no
TLS** — on its own it marks the session cookie `Secure` while serving plain HTTP, which browsers
then refuse to send back. This overlay closes that gap.

---

## The one command

```bash
cd deploy
docker compose -f compose.yml -f compose.prod.yml -f compose.tls.yml up -d
```

Three files, layered in order: base stack -> production hardening -> TLS. Order matters (later files
win). Everything the hardened posture needs (`HELDAR_SECRET_KEY`, `HELDAR_CORS_ORIGINS`) still comes
from `deploy/.env` exactly as documented for [`compose.prod.yml`](../docs/PRODUCTION.md) — this
overlay only adds the `caddy` service and two of its own env vars.

---

## Port model

| Where | Port | Purpose |
| --- | --- | --- |
| Host inbound | `443` | HTTPS — the only port a viewer connects to |
| Host inbound | `80` | ACME HTTP-01 challenge + automatic HTTP->HTTPS redirect |
| Loopback | `127.0.0.1:8080` | Caddy -> nginx (SPA + proxy). Cleartext, never leaves the host |
| Loopback | `127.0.0.1:8000` | nginx -> kernel (`/api`, `/media`, `/healthz`) |

Caddy runs with **host networking**, like the rest of the stack. This is deliberate: the media plane
(MediaMTX / WHEP / RTSP) needs host networking, and it lets Caddy reach the dashboard on `127.0.0.1`
with no bridge or published-port plumbing. The single cleartext hop (Caddy -> nginx) never leaves
loopback. Caddy never sits in front of the media ports — the WebRTC remote-access path keeps its own
host ports untouched.

---

## Two modes, selected by env

Put these in `deploy/.env` (alongside the production secrets). The `caddy` service reads them.

### Public domain — automatic Let's Encrypt

```dotenv
HELDAR_TLS_DOMAIN=cam.example.com
HELDAR_TLS_ISSUER=you@example.com
```

**Prerequisites:**

- A DNS **A/AAAA record** for `HELDAR_TLS_DOMAIN` pointing at this box's public IP.
- Inbound **`:80` and `:443` reachable from the internet** (Caddy uses HTTP-01 / TLS-ALPN-01 to
  prove domain control). If the box is behind CGNAT with no inbound port, public ACME **cannot
  work** — that is exactly the case the private remote-access path solves instead (see below).
- `HELDAR_TLS_ISSUER` set to a real email (Let's Encrypt uses it for expiry notices). Setting the
  email is what tells Caddy to use ACME rather than its internal CA.

Caddy provisions and renews the certificate automatically; certs persist on the `caddy-data` volume.
Also set `HELDAR_CORS_ORIGINS=https://cam.example.com` in `.env` so the kernel's CORS matches the
HTTPS origin.

### LAN — self-signed (`tls internal`, the default)

```dotenv
HELDAR_TLS_DOMAIN=https://192.168.1.50   # this box's LAN IP or hostname, https:// prefix
HELDAR_TLS_ISSUER=internal
```

`internal` uses Caddy's built-in CA to mint a local cert — no DNS, no public reachability, nothing to
buy. Browsers will show a certificate warning on first visit (expected for a private CA); accept it,
or install Caddy's root from the `caddy-data` volume (`/data/caddy/pki/authorities/local/root.crt`)
into the client trust store to make it clean. This is the right choice for a box only ever reached on
the local network. With no env set at all, the overlay defaults to `localhost` + `internal`, so a
bare `up -d` still comes up on self-signed HTTPS.

HSTS is sent in both modes; browsers ignore HSTS delivered for a bare IP address, so it only takes
effect for real hostnames.

---

## Security headers

`deploy/Caddyfile` adds dashboard response headers on every mode:

- `Strict-Transport-Security: max-age=31536000; includeSubDomains` (HSTS)
- `X-Content-Type-Options: nosniff`
- `X-Frame-Options: DENY`
- `Referrer-Policy: strict-origin-when-cross-origin`
- strips the `Server` header

---

## How it composes with `compose.prod.yml`

The `Secure` cookie flag comes from `HELDAR_AUTH_COOKIE_SECURE=true` in `compose.prod.yml`; the
kernel sets it **unconditionally from that env**, independent of the scheme on the internal loopback
hop. So terminating TLS at Caddy is all that's required for the `Secure` + `SameSite` cookie to be
delivered and returned correctly — nothing about the kernel or nginx needs to detect the outer HTTPS.

**No `apps/web/nginx.conf` change is needed.** Reasoning:

- The kernel already sits behind nginx on loopback (`127.0.0.1:8000`) in every deployment; adding
  Caddy in front of nginx does not change what the kernel sees.
- The `Secure` cookie is env-driven (above), not derived from the request scheme, so nginx does not
  need to forward `X-Forwarded-Proto` for cookies to work.
- Caddy already injects `X-Forwarded-For` / `X-Forwarded-Proto` / `Host` on its hop to nginx. The
  kernel's inbound HTTP handlers do not consume forwarded client-IP headers today (the only
  forwarded-header consumer is the outbound WebRTC rendezvous path), so there is nothing for nginx to
  pass through. If a future change makes the kernel log or rate-limit on real client IP, add
  `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;` and
  `proxy_set_header X-Forwarded-Proto $scheme;` to the `/api/` and `/media/` locations in
  `apps/web/nginx.conf` — but that is not required for this deployment to be correct.

---

## Relationship to the private remote-access path

This overlay is for boxes with a **public, internet-reachable address** (or a purely LAN audience).
It is the "operator-supplied reverse proxy" slot in [`docs/PRODUCTION.md`](../docs/PRODUCTION.md),
now shipped as a reference.

It is **not** a substitute for [`docs/REMOTE-ACCESS.md`](../docs/REMOTE-ACCESS.md). Most
home/small-site boxes are behind **CGNAT** with no inbound port — there, public ACME and a public
`:443` are impossible, and remote viewing instead uses the outbound-dialing WebRTC path described in
that document. Use this TLS overlay when you genuinely control an inbound public endpoint (a VPS, a
site with a static IP and port-forward, or an IPv6-reachable host), or for hardened LAN-only HTTPS.
The two paths are orthogonal and can coexist: a box can serve this TLS front on its LAN/public
interface while still reaching distant viewers over the remote-access path.

---

## Verify

```bash
docker compose -f compose.yml -f compose.prod.yml -f compose.tls.yml config -q   # validates the merge
docker compose -f compose.yml -f compose.prod.yml -f compose.tls.yml up -d
curl -kI https://<box-address>/healthz                                            # 200 over TLS
```
