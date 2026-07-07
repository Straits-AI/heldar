#!/usr/bin/env bash
# Sign a Heldar plugin-registry catalog.
#
# Emits a detached Ed25519 signature over the EXACT bytes of the catalog file, as <catalog>.sig — the
# artifact Heldar Core fetches alongside the catalog and verifies against a pinned public key. The
# private key NEVER lives in the repo; keep it in your release infrastructure / a secret store.
#
#   ./scripts/sign-catalog.sh catalog.json ed25519-private-key.pem straits-ai-registry-2026
#
# Generate a keypair once:
#   openssl genpkey -algorithm ed25519 -out registry.pem
# Print the 32-byte raw PUBLIC key (base64) to pin in TRUSTED_KEYS (or HELDAR_REGISTRY_TRUSTED_KEYS):
#   openssl pkey -in registry.pem -pubout -outform DER | tail -c 32 | base64
set -euo pipefail

CATALOG="${1:?usage: sign-catalog.sh <catalog.json> <ed25519-private-key.pem> <key_id>}"
KEY="${2:?missing ed25519 private key (PEM)}"
KEY_ID="${3:?missing key_id (must match a pinned TRUSTED_KEYS entry)}"

SIG_RAW="$(mktemp)"
trap 'rm -f "$SIG_RAW"' EXIT

# PureEdDSA over the raw message bytes (no pre-hash) — matches ring::signature::ED25519 verification.
openssl pkeyutl -sign -rawin -inkey "$KEY" -in "$CATALOG" -out "$SIG_RAW"
SIG_B64="$(base64 -w0 < "$SIG_RAW" 2>/dev/null || base64 < "$SIG_RAW" | tr -d '\n')"
SHA="$(openssl dgst -sha256 -hex "$CATALOG" | awk '{print $NF}')"

cat > "${CATALOG}.sig" <<JSON
{
  "alg": "ed25519",
  "key_id": "${KEY_ID}",
  "signature": "${SIG_B64}",
  "catalog_sha256": "${SHA}"
}
JSON

echo "wrote ${CATALOG}.sig (key_id=${KEY_ID})"
