# Security Policy

## Supported versions

Security fixes land on `main` and ship in the **latest tagged release**. Older releases are not
patched — upgrade to the latest release (prebuilt binaries are attached to each GitHub release; see
the upgrade steps in [docs/PRODUCTION.md](./docs/PRODUCTION.md)).

## Reporting a vulnerability

Please do **not** open a public issue for vulnerabilities. Report them privately via
[GitHub Private Vulnerability Reporting](https://github.com/Straits-AI/heldar/security/advisories/new)
on `Straits-AI/heldar`. We will acknowledge your report within **7 days**.

## Scope

The kernel is designed for **LAN-first** deployment: the permissive defaults (auth off, plaintext
camera credentials) are intentional for a trusted single network, not vulnerabilities in themselves.
For anything internet-exposed, the hardening posture in [docs/PRODUCTION.md](./docs/PRODUCTION.md)
is the baseline — reports that defeat that hardened configuration (auth/RBAC bypass, session or
media-URL forgery, data exposure across the app seams) are very much in scope.
