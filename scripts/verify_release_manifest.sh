#!/usr/bin/env bash
# Verify a deployment against its release manifest (#112). Fails CLOSED.
#
# Usage: verify_release_manifest.sh <manifest.json> [deploy-dir]
#
# Checks, in the order that matters:
#   1. the deployment files on disk are byte-for-byte the ones this release shipped
#   2. the running kernel's schema is not AHEAD of what this release supports
#
# (2) is the one that matters on an upgrade. Migrations only go forward, so a database written by a
# NEWER release cannot be safely served by an older binary — and the failure mode without this check
# is not a clean refusal, it is an old binary reading a schema it does not understand.
set -euo pipefail
MANIFEST="${1:?usage: verify_release_manifest.sh <manifest.json> [deploy-dir]}"
DEPLOY="${2:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/deploy}"
DB="${HELDAR_DB:-}"

fail=0
say(){ echo "$*"; }
bad(){ echo "FAIL $*"; fail=1; }
ok(){ echo "PASS $*"; }

command -v python3 >/dev/null || { echo "python3 required"; exit 2; }

say "== artifacts =="
CHECKED=0
while IFS=$'\t' read -r name want; do
  CHECKED=$((CHECKED+1))
  f="$DEPLOY/$name"
  if [ ! -f "$f" ]; then bad "$name is missing from $DEPLOY"; continue; fi
  got="$(sha256sum "$f" | cut -d' ' -f1)"
  if [ "$got" = "$want" ]; then ok "$name"; else
    bad "$name has been modified — manifest says $want, on disk $got"
  fi
done < <(python3 -c '
import json, sys
m = json.load(open(sys.argv[1]))
for k, v in m.get("artifacts", {}).items():
    sys.stdout.write(k + "\t" + v["sha256"] + "\n")' "$MANIFEST")

# A verifier that checked NOTHING must not report success. The first version of this loop had a
# Python syntax error, produced no rows, and printed RESULT: PASS — a green answer from a check that
# never ran, which is the exact failure this whole file exists to prevent.
if [ "$CHECKED" = 0 ]; then
  bad "the manifest listed no artifacts to verify — refusing to report success on an empty check"
fi

say "== schema ceiling =="
CEIL="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["migrations"]["kernel_max"])' "$MANIFEST")"
if [ -n "$DB" ] && [ -f "$DB" ]; then
  # sqlx records applied migrations in _sqlx_migrations; the highest one is the schema this database
  # is actually at.
  APPLIED="$(python3 -c '
import sqlite3,sys
try:
    c=sqlite3.connect(sys.argv[1])
    r=c.execute("SELECT MAX(version) FROM _sqlx_migrations").fetchone()[0]
    print(r if r is not None else 0)
except Exception:
    print(-1)' "$DB")"
  if [ "$APPLIED" = "-1" ]; then
    say "SKIP database has no migration table yet (fresh install)"
  elif [ "$APPLIED" -gt "$CEIL" ]; then
    bad "database is at migration $APPLIED but this release supports at most $CEIL — it was written by a NEWER release, and this binary cannot safely serve it"
  else
    ok "database at $APPLIED, within this release's ceiling of $CEIL"
  fi
else
  say "SKIP no HELDAR_DB set — artifact check only"
fi

if [ "$fail" = 1 ]; then
  echo "RESULT: FAIL"
  exit 1
fi
echo "RESULT: PASS"
