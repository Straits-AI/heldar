#!/usr/bin/env python3
"""Verify a Heldar evidence bundle (#118). Offline. Fails CLOSED.

Usage:
  verify_evidence_bundle.py <bundle.heldar-evidence> [--key <base64-public-key>|--key-id <sha256:...>]

Needs python3 and openssl. It does NOT need Heldar, the appliance that produced the bundle, its
database, or a network — that is the property that makes a bundle evidence rather than a export.

WHAT "VALID" MEANS HERE, AND WHAT IT DOES NOT.

A VALID result says: this bundle's manifest was signed by the holder of the named key, and every
file the manifest lists is present and unchanged. It says nothing about whether that key belongs to
the appliance you think it does. Pass --key or --key-id with a value you obtained OUT OF BAND to
turn that into a real statement; without one the result is UNKNOWN-KEY, because a bundle carrying
its own public key proves only that whoever made it also made the key.

It also says nothing about WHEN the bundle was made. The appliance stamps its own clock.

EXIT CODES — distinct so a caller can branch on them, per #118's acceptance criteria:
  0 VALID              signature good, every hash matches, key matched the one you supplied
  1 MODIFIED           a file's content does not match the manifest, or the signature does not verify
  2 MISSING            the manifest lists a file the bundle does not contain
  3 UNKNOWN-KEY        structurally sound and self-consistent, but the key is unverified/mismatched
  4 UNSUPPORTED        the bundle's format version is not one this verifier understands
  5 MALFORMED          not a bundle: unreadable zip, absent manifest, unparseable JSON
"""

import argparse
import base64
import hashlib
import json
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

VALID, MODIFIED, MISSING, UNKNOWN_KEY, UNSUPPORTED, MALFORMED = 0, 1, 2, 3, 4, 5

SUPPORTED = {"heldar-evidence/1"}

# An Ed25519 SubjectPublicKeyInfo is a fixed 12-byte prefix followed by the raw 32-byte key. Building
# it here means openssl can read the key without the bundle having to carry a PEM, and without this
# script depending on a Python crypto library that an investigator's machine may not have.
SPKI_PREFIX = bytes.fromhex("302a300506032b6570032100")


def out(state: str, msg: str) -> None:
    print(f"{state}: {msg}")


def main() -> int:
    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument("bundle")
    ap.add_argument("--key", help="expected signing public key, base64 (obtained out of band)")
    ap.add_argument("--key-id", help="expected key id, sha256:<hex> (obtained out of band)")
    args = ap.parse_args()

    try:
        zf = zipfile.ZipFile(args.bundle)
    except (zipfile.BadZipFile, FileNotFoundError, IsADirectoryError, PermissionError) as e:
        out("MALFORMED", f"cannot open {args.bundle} as a bundle: {e}")
        return MALFORMED

    with zf:
        names = set(zf.namelist())
        # Zip entries can be written as "./manifest.json"; normalise once so every later lookup and
        # every membership test agrees on one spelling.
        norm = {n[2:] if n.startswith("./") else n: n for n in names}

        for required in ("manifest.json", "signature.json"):
            if required not in norm:
                out("MALFORMED", f"the bundle has no {required}")
                return MALFORMED

        manifest_bytes = zf.read(norm["manifest.json"])
        try:
            manifest = json.loads(manifest_bytes)
            signature = json.loads(zf.read(norm["signature.json"]))
        except (json.JSONDecodeError, UnicodeDecodeError) as e:
            out("MALFORMED", f"manifest or signature is not valid JSON: {e}")
            return MALFORMED

        fmt = manifest.get("format")
        if fmt not in SUPPORTED:
            # Reported before anything else is checked. A newer bundle may be self-consistent under
            # rules this verifier does not know, and "valid" from a verifier that did not understand
            # the document is the worst answer available.
            out("UNSUPPORTED", f"bundle format {fmt!r} — this verifier understands {sorted(SUPPORTED)}")
            return UNSUPPORTED

        # --- the signature, over the manifest bytes exactly as stored --------------------------
        pub_b64 = signature.get("public_key") or ""
        sig_b64 = signature.get("signature") or ""
        if signature.get("algorithm") != "ed25519" or not pub_b64 or not sig_b64:
            out("MALFORMED", "signature.json does not carry an ed25519 signature")
            return MALFORMED
        try:
            pub = base64.b64decode(pub_b64, validate=True)
            sig = base64.b64decode(sig_b64, validate=True)
        except Exception as e:
            out("MALFORMED", f"signature.json holds unreadable base64: {e}")
            return MALFORMED
        if len(pub) != 32:
            out("MALFORMED", f"public key is {len(pub)} bytes, expected 32")
            return MALFORMED

        verified = ed25519_verify(pub, sig, manifest_bytes)
        if verified is None:
            return MALFORMED
        if not verified:
            out("MODIFIED", "the signature does not verify over manifest.json — the manifest has "
                            "been altered since it was signed, or it was signed by a different key")
            return MODIFIED

        # The stated manifest hash must agree with the manifest actually present. A signature that
        # verifies against a manifest whose recorded hash points elsewhere is a document arguing
        # with itself.
        got = hashlib.sha256(manifest_bytes).hexdigest()
        stated = signature.get("manifest_sha256")
        if stated and stated != got:
            out("MODIFIED", f"signature.json records manifest_sha256 {stated} but the manifest "
                            f"present hashes to {got}")
            return MODIFIED

        # --- every file the manifest claims ----------------------------------------------------
        files = manifest.get("files")
        if not isinstance(files, dict) or not files:
            out("MALFORMED", "the manifest lists no files — it attests to nothing")
            return MALFORMED

        missing, modified, checked = [], [], 0
        for rel, entry in sorted(files.items()):
            if rel not in norm:
                missing.append(rel)
                continue
            want = (entry or {}).get("sha256")
            if not isinstance(want, str) or not want:
                out("MALFORMED", f"the manifest gives no usable sha256 for {rel}")
                return MALFORMED
            h = hashlib.sha256()
            with zf.open(norm[rel]) as fh:
                for chunk in iter(lambda: fh.read(1 << 20), b""):
                    h.update(chunk)
            checked += 1
            if h.hexdigest() != want:
                modified.append(rel)

        # A verifier that checked nothing must never report success. This bundle format cannot
        # legitimately reach here with zero files — `files` was already required to be non-empty —
        # so reaching it means the loop did not do what it says.
        if checked == 0 and not missing:
            out("MALFORMED", "no file was actually checked — refusing to report a result")
            return MALFORMED

        if modified:
            out("MODIFIED", "content differs from the signed manifest: " + ", ".join(modified))
            return MODIFIED
        if missing:
            out("MISSING", "the manifest lists files the bundle does not contain: " + ", ".join(missing))
            return MISSING

        # hashes.sha256 is a convenience for `sha256sum -c`. It is NOT the authority — the signed
        # manifest is — so if the two disagree, a reader using coreutils alone would be shown hashes
        # nobody signed. That is a modification, not a nit.
        if "hashes.sha256" in norm:
            side = {}
            for line in zf.read(norm["hashes.sha256"]).decode("utf-8", "replace").splitlines():
                parts = line.split(None, 1)
                if len(parts) == 2:
                    side[parts[1].strip()] = parts[0].strip()
            expected = {rel: e["sha256"] for rel, e in files.items()}
            if side != expected:
                out("MODIFIED", "hashes.sha256 disagrees with the signed manifest — a reader "
                                "verifying with sha256sum alone would be checking unsigned hashes")
                return MODIFIED

        # --- whose key was it ------------------------------------------------------------------
        key_id = signature.get("key_id") or ""
        computed_id = "sha256:" + hashlib.sha256(pub).hexdigest()
        if key_id and key_id != computed_id:
            out("MODIFIED", f"signature.json claims key_id {key_id} but its public key is {computed_id}")
            return MODIFIED

        if args.key:
            if args.key.strip() != pub_b64.strip():
                out("UNKNOWN-KEY", f"signed by {computed_id}, which is NOT the key you supplied")
                return UNKNOWN_KEY
        elif args.key_id:
            if args.key_id.strip() != computed_id:
                out("UNKNOWN-KEY", f"signed by {computed_id}, which is NOT the key id you supplied")
                return UNKNOWN_KEY
        else:
            out("UNKNOWN-KEY",
                f"internally consistent and signed by {computed_id}, but no expected key was given. "
                f"A bundle carrying its own public key proves only that whoever made the bundle also "
                f"made the key. Re-run with --key-id {computed_id} once you have obtained that value "
                f"from the appliance operator by some route other than this file.")
            return UNKNOWN_KEY

        out("VALID", f"{checked} files, signed by {computed_id}, {describe(manifest)}")
        for limit in manifest.get("attestation", {}).get("limits", []):
            print(f"  note: {limit}")
        return VALID


def describe(m: dict) -> str:
    cam = (m.get("camera") or {}).get("id", "?")
    media = m.get("media") or {}
    gaps = media.get("gaps") or []
    cov = media.get("covered_seconds")
    req = media.get("requested_seconds")
    s = f"camera {cam}, {media.get('requested_from')} → {media.get('requested_to')}"
    if cov is not None and req:
        s += f", {cov:.0f}s of {req:.0f}s recorded"
    if gaps:
        s += f", {len(gaps)} GAP(S) in the requested window"
    return s


def ed25519_verify(pub: bytes, sig: bytes, msg: bytes):
    """True/False, or None if openssl could not be used (reported as MALFORMED by the caller)."""
    with tempfile.TemporaryDirectory() as td:
        d = Path(td)
        (d / "key.der").write_bytes(SPKI_PREFIX + pub)
        (d / "sig").write_bytes(sig)
        (d / "msg").write_bytes(msg)
        try:
            r = subprocess.run(
                ["openssl", "pkeyutl", "-verify", "-pubin",
                 "-inkey", str(d / "key.der"), "-keyform", "DER",
                 "-rawin", "-in", str(d / "msg"), "-sigfile", str(d / "sig")],
                capture_output=True,
            )
        except FileNotFoundError:
            out("MALFORMED", "openssl is required to verify the signature and was not found")
            return None
        # openssl prints "Signature Verified Successfully" on success and exits non-zero on failure.
        # Both are checked: exit code alone has been known to be 0 on some builds for a malformed
        # invocation, and a verifier must not read "could not check" as "checked out".
        okd = r.returncode == 0 and b"Success" in r.stdout + r.stderr
        if r.returncode == 0 and not okd:
            out("MALFORMED", "openssl exited 0 without confirming the signature — refusing to "
                             f"interpret that as valid (stdout: {r.stdout[:200]!r})")
            return None
        return okd


if __name__ == "__main__":
    sys.exit(main())
