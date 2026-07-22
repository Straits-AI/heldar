#!/usr/bin/env bash
# Enforce the cross-app READ SEAM (DESIGN-PRINCIPLES #9): a consuming app must read a peer app's table
# through the owner's published `*_read` contract view, never the base table. The OWNER reads its own
# base table freely — this only flags a peer reading someone else's base table.
#
# Word boundary `\b` after the table name means `entry_events_read` (the view) does NOT match
# `entry_events\b`, so only base-table reads are caught.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0

# entry_events is owned by heldar-entry → movement + search must use entry_events_read.
if hits=$(grep -rnE '\b(FROM|JOIN)[[:space:]]+entry_events\b' \
    crates/heldar-movement/src crates/heldar-search/src 2>/dev/null); then
  echo "::error:: cross-app read of base table 'entry_events' — use the entry_events_read contract view:"
  echo "$hits"
  fail=1
fi

# breach_alerts is owned by heldar-movement → search (the only cross-app consumer) must use breach_alerts_read.
if hits=$(grep -rnE '\b(FROM|JOIN)[[:space:]]+breach_alerts\b' \
    crates/heldar-search/src 2>/dev/null); then
  echo "::error:: cross-app read of base table 'breach_alerts' — use the breach_alerts_read contract view:"
  echo "$hits"
  fail=1
fi

if [ "$fail" = 0 ]; then
  echo "read-seam lint: OK (no consumer reads a peer's base table directly)"
fi
exit "$fail"
