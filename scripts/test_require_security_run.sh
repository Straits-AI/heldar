#!/usr/bin/env bash
# Controls for .github/actions/require-security-run (issue #114).
#
# This gate only ever runs during a release, so without this file its first execution would be the
# moment someone is trying to ship — and the failure mode that matters is the silent one: an API
# hiccup or an empty response read as "scan is green". Every case below drives the ACTUAL script
# extracted from action.yml, against a stubbed `gh`.
set -uo pipefail
cd "$(dirname "$0")/.."

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Extract the real script — never a copy, or this tests something that is not shipped.
python3 - "$TMP/gate.sh" <<'PY'
import sys, yaml
a = yaml.safe_load(open(".github/actions/require-security-run/action.yml"))
step = next(s for s in a["runs"]["steps"] if s.get("id") == "check")
open(sys.argv[1], "w").write(step["run"])
PY
[ -s "$TMP/gate.sh" ] || { echo "FAIL: could not extract the gate script"; exit 1; }

mkdir -p "$TMP/bin"
cat > "$TMP/bin/gh" <<'STUB'
#!/usr/bin/env bash
# Stub: emits whatever $GH_STUB_PAYLOAD holds, or fails when it is the literal "ERROR".
[ "${GH_STUB_PAYLOAD:-}" = "ERROR" ] && exit 1
printf '%s' "${GH_STUB_PAYLOAD:-[]}"
STUB
chmod +x "$TMP/bin/gh"
export PATH="$TMP/bin:$PATH"

pass=0 fail=0
run_case() {  # name, payload, enforce, expected_exit, expected_grep
  local name="$1" payload="$2" enforce="$3" want_exit="$4" want_text="$5"
  local out
  out=$(GH_STUB_PAYLOAD="$payload" REPO=o/r SHA=deadbeef WORKFLOW=security.yml \
        TIMEOUT_MINUTES=0 ENFORCE="$enforce" GITHUB_OUTPUT="$TMP/out.txt" GH_TOKEN=x \
        bash "$TMP/gate.sh" 2>&1)
  local got_exit=$?
  if [ "$got_exit" -eq "$want_exit" ] && grep -qi -- "$want_text" <<<"$out"; then
    echo "  ok    $name"; pass=$((pass + 1))
  else
    echo "  FAIL  $name (exit $got_exit, wanted $want_exit; looking for '$want_text')"
    sed 's/^/          /' <<<"$out" | tail -5; fail=$((fail + 1))
  fi
}

SUCCESS='[{"status":"completed","conclusion":"success","url":"http://x"}]'
FAILURE='[{"status":"completed","conclusion":"failure","url":"http://x"}]'
CANCELLED='[{"status":"completed","conclusion":"cancelled","url":"http://x"}]'
# Newest first, as the API returns it: a later failure must beat an earlier success, because the
# weekly cron re-scans unchanged code and a new advisory has to block a release.
NEWER_FAILURE='[{"status":"completed","conclusion":"failure","url":"http://new"},
                {"status":"completed","conclusion":"success","url":"http://old"}]'

run_case "a green run for this SHA publishes"          "$SUCCESS"   true  0 "succeeded for deadbeef"
run_case "a failed run blocks"                          "$FAILURE"   true  1 "concluded 'failure'"
run_case "a cancelled run blocks — it is not a pass"    "$CANCELLED" true  1 "concluded 'cancelled'"
run_case "NO run at all blocks"                         '[]'         true  1 "never.*been security-scanned"
run_case "an API failure blocks, never passes"          "ERROR"      true  1 "never.*been security-scanned"
run_case "the newest completed run wins over an older success" "$NEWER_FAILURE" true 1 "concluded 'failure'"
run_case "enforce=false reports without blocking"       "$FAILURE"   false 0 "not enforcing"
run_case "a typo in enforce still enforces"             "$FAILURE"   ohno  1 "concluded 'failure'"
run_case "an empty enforce still enforces"              "$FAILURE"   ""    1 "concluded 'failure'"

echo
if [ "$fail" -ne 0 ]; then
  echo "$fail of $((pass + fail)) security-gate controls FAILED"; exit 1
fi
echo "all $pass security-gate controls passed"
