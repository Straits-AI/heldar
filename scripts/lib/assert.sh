# Assertions for the validate_* scripts.
#
# Those scripts logged what happened and left a human to notice whether it was right — `expect 0 —
# debounced` was a note in a transcript, not a check. Run unattended in CI that is a gate which
# cannot fail: it exits 0 whatever the API answers.
#
# `FAIL ` markers are the contract with the runner (validate_subsystems.sh and ci.yml both grep for
# them), because these scripts historically do not exit non-zero on a bad assertion.
ASSERT_FAILED=0

# assert_eq <expected> <actual> <what>
assert_eq(){
  if [ "$1" = "$2" ]; then
    log "PASS $3 (= $2)"
  else
    log "FAIL $3 — expected [$1], got [$2]"
    ASSERT_FAILED=1
  fi
}

# assert_contains <haystack> <needle> <what>
assert_contains(){
  case "$1" in
    *"$2"*) log "PASS $3" ;;
    *) log "FAIL $3 — [$2] not present in [$1]"; ASSERT_FAILED=1 ;;
  esac
}

# assert_ge <actual> <minimum> <what>
assert_ge(){
  if [ "$1" -ge "$2" ] 2>/dev/null; then
    log "PASS $3 ($1 >= $2)"
  else
    log "FAIL $3 — expected at least $2, got $1"
    ASSERT_FAILED=1
  fi
}

# Call at the end so the script's own exit code agrees with its markers.
assert_summary(){
  if [ "$ASSERT_FAILED" = 1 ]; then
    log "RESULT: FAIL"
    return 1
  fi
  log "RESULT: PASS"
  return 0
}
