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

# --- the derive-your-own-image recipe vs the requirements it copies ---------------------------
# apps/ai/Dockerfile documents how to add the heavy model stacks:
#
#   RUN pip install --no-cache-dir "ultralytics>=X" "lap>=Y"
#
# Those floors are copied from requirements.txt, and Dependabot updates requirements files but never
# comments. Merging #78 (lap >=0.5 -> >=0.5.13) left that line telling operators to install a floor
# the project had already moved past — an instruction that is wrong in the file that teaches it.
dockerfile = read("apps/ai/Dockerfile")
reqs = read("apps/ai/requirements.txt")
if dockerfile and reqs:
    for pkg in ("ultralytics", "lap"):
        in_docs = re.search(rf'"{pkg}>=([0-9][^"]*)"', dockerfile)
        in_reqs = re.search(rf"^{pkg}>=([0-9]\S*)", reqs, re.M)
        if not in_docs or not in_reqs:
            problems.append(
                f"could not find a {pkg} floor in both apps/ai/Dockerfile and "
                f"apps/ai/requirements.txt — this check's parser has drifted"
            )
            continue
        if in_docs.group(1) != in_reqs.group(1):
            problems.append(
                f"apps/ai/Dockerfile tells operators to install {pkg}>={in_docs.group(1)}, but "
                f"apps/ai/requirements.txt declares >={in_reqs.group(1)}. The documented recipe is "
                f"the one people actually run."
            )

# --- the supply-chain policy table vs the files it describes -----------------------------------
# docs/SUPPLY-CHAIN.md lists every pinned base image by concrete tag. That makes it a document which
# goes stale precisely when someone merges the bump it describes — and it is the document that tells
# people how to bump safely, so a reader following it is following a version the project left behind.
#
# Four of its six rows were wrong when this check was written, three from one afternoon of
# dependency merges.
policy = read("docs/SUPPLY-CHAIN.md")
if policy:
    table = re.search(r"^## What is pinned\n\n(\|.*?\n)\n", policy, re.S | re.M)
    rows = re.findall(r"^\|\s*([^|]+?)\s*\|\s*`([^`]+)`\s*\|", table.group(1), re.M) if table else []
    rows = [(w, p) for w, p in rows if ":" in p or "/" in p]
    if len(rows) < 5:
        problems.append(
            f"docs/SUPPLY-CHAIN.md: parsed only {len(rows)} pinned images from the policy table — "
            f"this check's parser has drifted and would pass vacuously"
        )
    for where, pin in rows:
        files = [f.strip().strip("`") for f in where.split(",")]
        files = [f.split(" ")[0].strip("`") for f in files]
        found = False
        looked = []
        for f in files:
            txt = read(f)
            if txt is None:
                continue
            looked.append(f)
            if re.search(rf"(?:FROM|image:)\s+{re.escape(pin)}[@\s]", txt):
                found = True
                break
        if looked and not found:
            problems.append(
                f"docs/SUPPLY-CHAIN.md names {pin} for {', '.join(looked)}, which does not pin that "
                f"image. The policy document is describing a version the tree has moved past."
            )

print(f"checked {4} pinned-in-two-places dependencies")
if problems:
    print(f"\n{len(problems)} problem(s):")
    for p in problems:
        print(f"  {p}")
    sys.exit(1)
print("RESULT: PASS")
