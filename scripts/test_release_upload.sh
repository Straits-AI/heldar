#!/usr/bin/env bash
# Controls for scripts/release_upload.sh (#115).
#
# Run: ./scripts/test_release_upload.sh
#
# A fake `gh` stands in for the release, so these need no network, no token and no real tag. The
# case that matters is the third: republishing DIFFERENT bytes under a tag consumers have already
# pinned. `--clobber` did exactly that, silently, on every upload in release.yml.
#
# The second case is the reason this is a comparison rather than a blanket refusal: a release
# workflow that cannot be re-run after an infrastructure flake is its own hazard, so identical bytes
# must stay idempotent.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$HERE/release_upload.sh"
FAIL=0

run_case(){ # <name> <published|NONE> <local> <want-rc> [<want-text>]
  local name="$1" published="$2" local_content="$3" want="$4" want_text="${5:-}"
  local d; d="$(mktemp -d)"
  mkdir -p "$d/bin" "$d/store"
  printf '%s' "$local_content" > "$d/asset.bin"
  [ "$published" != "NONE" ] && printf '%s' "$published" > "$d/store/asset.bin"
  cat > "$d/bin/gh" <<'GH'
#!/usr/bin/env bash
STORE="$(dirname "$0")/../store"
case "$1 $2" in
  "release view")   ls "$STORE" 2>/dev/null | grep -q . || exit 1; ls "$STORE" ;;
  "release download")
    name=""; dir=""
    while [ $# -gt 0 ]; do
      case "$1" in --pattern) name="$2"; shift;; --dir) dir="$2"; shift;; esac; shift
    done
    [ -f "$STORE/$name" ] || exit 1
    cp "$STORE/$name" "$dir/$name" ;;
  "release upload")
    shift 3
    for f in "$@"; do [ "$f" = "--clobber" ] && continue; cp "$f" "$STORE/$(basename "$f")"; done ;;
  *) exit 0 ;;
esac
GH
  chmod +x "$d/bin/gh"
  PATH="$d/bin:$PATH" bash "$SCRIPT" v1.0.0 "$d/asset.bin" >"$d/out" 2>&1
  local rc=$?
  local out; out="$(cat "$d/out")"
  if [ "$rc" != "$want" ]; then
    echo "  FAIL  $name (rc=$rc want=$want): $(echo "$out" | tail -2 | tr '\n' ' ')"; FAIL=1
  elif [ -n "$want_text" ] && ! echo "$out" | grep -qF "$want_text"; then
    echo "  FAIL  $name — wanted '$want_text' in: $(echo "$out" | tail -2 | tr '\n' ' ')"; FAIL=1
  else
    echo "  ok    $name"
  fi
  rm -rf "$d"
}

run_case "a new asset uploads"                               NONE      "bytes-A" 0
run_case "an identical re-upload stays idempotent"           "bytes-A" "bytes-A" 0 "identical bytes"
run_case "DIFFERENT bytes under an existing tag are REFUSED" "bytes-A" "bytes-B" 1 "DIFFERENT bytes"

# Fails CLOSED: an existing asset that cannot be fetched to compare is a refusal, not a pass. This is
# the branch that would otherwise turn a network blip into a silent overwrite.
d="$(mktemp -d)"; mkdir -p "$d/bin" "$d/store"; printf 'x' > "$d/asset.bin"
cat > "$d/bin/gh" <<'GH'
#!/usr/bin/env bash
case "$1 $2" in
  "release view")     echo "asset.bin" ;;
  "release download") exit 1 ;;
  *)                  exit 0 ;;
esac
GH
chmod +x "$d/bin/gh"
PATH="$d/bin:$PATH" bash "$SCRIPT" v1.0.0 "$d/asset.bin" >"$d/out" 2>&1
rc=$?
if [ "$rc" = 1 ] && grep -qF "could not be downloaded to compare" "$d/out"; then
  echo "  ok    an uncomparable existing asset is refused, not overwritten"
else
  echo "  FAIL  an uncomparable existing asset should be refused (rc=$rc)"; FAIL=1
fi
rm -rf "$d"

[ "$FAIL" = 0 ] && echo "" && echo "all release-upload controls behaved as specified"
exit $FAIL
