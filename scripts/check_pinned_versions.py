#!/usr/bin/env python3
"""Two places that pin the same thing must agree.

Run: python3 scripts/check_pinned_versions.py

`scripts/setup_caddy.sh` installs Caddy for the TLS end-to-end test; `deploy/compose.tls.yml` pins
the Caddy image the appliance actually ships. They are the same dependency written twice, and
nothing compared them.

That is not theoretical. Dependabot #87 bumped the compose pin to 2.11.4 and left the script at
2.10.2, so the required `playwright e2e (HTTPS + auth)` check went GREEN having installed the OLD
Caddy — a passing check about a version the change was not making. The two releases in between
touched URI rewriting and placeholder expansion in injected queries, which is precisely the
`handle_path` + query-token path deploy/Caddyfile is built on.

A green check that did not exercise the change is worse than a red one: it is a claim nobody
re-examines.
"""

import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
problems = []


def read(rel):
    p = os.path.join(ROOT, rel)
    return open(p).read() if os.path.isfile(p) else None


def one(pattern, text, what, rel):
    """Exactly one match, or the parser has drifted from the file and must say so."""
    found = re.findall(pattern, text)
    if len(found) != 1:
        problems.append(
            f"{rel}: expected exactly one {what}, found {len(found)} — this check's parser has "
            f"drifted from the file it reads, which would make it pass vacuously"
        )
        return None
    return found[0]


# --- Caddy: the e2e installer vs the shipped image ----------------------------------------------
script = read("scripts/setup_caddy.sh")
compose = read("deploy/compose.tls.yml")
if script and compose:
    script_v = one(r'VERSION="\$\{CADDY_VERSION:-([0-9][^}"]*)\}"', script, "CADDY_VERSION default",
                   "scripts/setup_caddy.sh")
    image_v = one(r"image:\s*caddy:([0-9][^@\s]*)@sha256:", compose, "pinned caddy image",
                  "deploy/compose.tls.yml")
    if script_v and image_v and script_v != image_v:
        problems.append(
            f"Caddy is pinned twice and they disagree: scripts/setup_caddy.sh installs {script_v} "
            f"(this is what the TLS e2e check exercises) but deploy/compose.tls.yml ships "
            f"{image_v}. Bump both, or the check is green about the wrong version."
        )

# --- MediaMTX: the dev binary vs the two composed images ----------------------------------------
# setup_mediamtx.sh deliberately tracks the LATEST release rather than a pin, so there is nothing to
# compare it against. The two compose files, however, pin the same image and must not drift from
# each other — the dev stack and the shipped appliance running different MediaMTX builds is the
# silent divergence the pinning policy exists to prevent.
root_compose = read("docker-compose.yml")
deploy_compose = read("deploy/compose.yml")
if root_compose and deploy_compose:
    a = one(r"image:\s*bluenviron/mediamtx:([^@\s]+)@sha256:", root_compose, "mediamtx image",
            "docker-compose.yml")
    b = one(r"image:\s*bluenviron/mediamtx:([^@\s]+)@sha256:", deploy_compose, "mediamtx image",
            "deploy/compose.yml")
    if a and b and a != b:
        problems.append(
            f"MediaMTX differs between the dev stack ({a}, docker-compose.yml) and the shipped "
            f"appliance ({b}, deploy/compose.yml)"
        )

print(f"checked {2} pinned-in-two-places dependencies")
if problems:
    print(f"\n{len(problems)} problem(s):")
    for p in problems:
        print(f"  {p}")
    sys.exit(1)
print("RESULT: PASS")
