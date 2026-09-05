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
import os
import subprocess
import sys
import tempfile
import zipfile
import zlib
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
        # THE FILE MUST BE ONE ARCHIVE, AND EVERY BYTE OF IT MUST BELONG TO THAT ARCHIVE.
        #
        # Checked before the entry names, because it decides WHICH archive the names come from.
        #
        # A zip is read from the back: the End Of Central Directory record at the tail names the
        # directory, and the directory names the entries. So `cat forged.zip genuine.zip` produces a
        # file that Python's zipfile — and `unzip`, and any seeking reader — sees as the genuine
        # bundle alone, while 7z's default mode and ANY streaming read (`cat bundle | bsdtar -x`,
        # `… | tar -x`) walk the local headers from the front and see the forged one. Demonstrated:
        # the verifier said VALID against the appliance's real key id three times running while a
        # streamed extraction wrote `cam_EVIL` and "FORGED CLIP - plate ABC-999" to disk.
        #
        # No amount of care about entry NAMES reaches this: the forged archive's names never appear
        # in the directory the verifier reads. The invariant that does reach it is structural — a
        # streaming reader and a seeking reader must see the same archive — and it holds exactly
        # when every byte of the file is accounted for by the central directory.
        state, problem = reject_concatenated_archive(args.bundle, zf)
        if problem:
            # The STATE matters as much as the refusal. This check runs first, so without it a
            # damaged clip would be reported as "not a bundle" rather than as altered evidence —
            # and an investigator holding a tampered-with recording needs MODIFIED, which names
            # what happened, not MALFORMED, which says the file is unreadable.
            out("MODIFIED" if state == MODIFIED else "MALFORMED", problem)
            return state

        names = zf.namelist()

        # THE ARCHIVE'S SHAPE IS CHECKED BEFORE ANY OF ITS CONTENT.
        #
        # The first version of this resolved manifest keys through a normalising dict:
        #
        #     norm = {n[2:] if n.startswith("./") else n: n for n in names}
        #
        # which strips ONE leading "./" and nothing else, and silently ignored any entry it did not
        # recognise. Both halves were exploitable, and together they were a working forgery:
        #
        #   * `././manifest.json` and `media/./clip.mp4` are invisible to that mapping but collapse
        #     onto the real paths when ANY extractor writes them out. A bundle carrying both the
        #     genuine `manifest.json` (which the verifier hashed and reported VALID) and a forged
        #     `././manifest.json` (which unzip, bsdtar, Finder and zipfile.extractall all wrote over
        #     it) verified clean while naming a different camera on disk. The forged tree even
        #     passed `sha256sum -c hashes.sha256`, because the attacker rewrote that too.
        #   * unlisted extra entries were accepted outright, so a fabricated second camera angle and
        #     an "EXHIBIT-B.txt" could be carried inside a VALID signed evidence package.
        #   * `names` was a `set`, and CPython randomises str hashing per process, so when two
        #     spellings did collide the verdict flipped between VALID and MODIFIED across runs on
        #     the identical file. A forensic tool that answers differently on alternate runs is
        #     unusable regardless of which answer is right.
        #
        # So: REFUSE, do not normalise. Normalising is what created the gap — it invents a mapping
        # from what is in the archive to what the verifier checks, and every such mapping is a place
        # the extractor can disagree. The manifest names an exact set of files; the archive must
        # contain exactly that set plus its own three metadata files, each spelled exactly once, in
        # a form no extractor will reinterpret.
        problem = reject_unsafe_names(names)
        if problem:
            out("MALFORMED", problem)
            return MALFORMED

        for required in ("manifest.json", "signature.json"):
            if required not in names:
                out("MALFORMED", f"the bundle has no {required}")
                return MALFORMED

        manifest_bytes = zf.read("manifest.json")
        try:
            manifest = json.loads(manifest_bytes)
            signature = json.loads(zf.read("signature.json"))
        except (json.JSONDecodeError, UnicodeDecodeError) as e:
            out("MALFORMED", f"manifest or signature is not valid JSON: {e}")
            return MALFORMED

        # `json.loads` happily returns a list, a number or a string. Every `.get()` below assumed an
        # object, so a manifest of `[1,2,3]` raised AttributeError — an uncaught traceback, which
        # exits 1, which IS the MODIFIED code. A caller branching on exit codes would have read a
        # crash as "this evidence was altered": a false accusation produced by a malformed file.
        if not isinstance(manifest, dict):
            out("MALFORMED", f"manifest.json is a {type(manifest).__name__}, not a JSON object")
            return MALFORMED
        if not isinstance(signature, dict):
            out("MALFORMED", f"signature.json is a {type(signature).__name__}, not a JSON object")
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

        # The archive must contain EXACTLY the manifest's files plus this format's own three, each
        # spelled once. An unlisted entry is not "extra harmless content": it is a file that will be
        # written to disk beside the attested ones, indistinguishable to whoever opens the folder,
        # and covered by no signature.
        expected = set(files) | {"manifest.json", "signature.json", "hashes.sha256"}
        sizes = {i.filename: i.file_size for i in zf.infolist()}
        unexpected = sorted(
            n for n in names
            if n not in expected and not is_dir_entry(n, expected, sizes.get(n, 0))
        )
        if unexpected:
            out("MALFORMED",
                "the archive carries entries the signed manifest does not list: "
                + ", ".join(unexpected)
                + " — refusing rather than ignoring them, because they extract into the same "
                  "directory as the attested files and nothing signed says they do not belong")
            return MALFORMED

        missing, modified, checked = [], [], 0
        for rel, entry in sorted(files.items()):
            if rel not in names:
                missing.append(rel)
                continue
            want = (entry or {}).get("sha256")
            if not isinstance(want, str) or not want:
                out("MALFORMED", f"the manifest gives no usable sha256 for {rel}")
                return MALFORMED
            h = hashlib.sha256()
            try:
                with zf.open(rel) as fh:
                    for chunk in iter(lambda: fh.read(1 << 20), b""):
                        h.update(chunk)
            except (zipfile.BadZipFile, EOFError, OSError) as e:
                # The zip's own CRC or its compressed stream is broken. This IS a content failure and
                # belongs with the others rather than in the catch-all: an investigator needs to know
                # WHICH file would not come out, and "could not be processed" does not say that.
                modified.append(f"{rel} (unreadable: {e})")
                checked += 1
                continue
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
        if "hashes.sha256" in names:
            side = {}
            try:
                sidecar = zf.read("hashes.sha256")
            except (zipfile.BadZipFile, EOFError, OSError) as e:
                out("MODIFIED", f"hashes.sha256 will not come out of the archive ({e})")
                return MODIFIED
            for line in sidecar.decode("utf-8", "replace").splitlines():
                parts = line.split(None, 1)
                if len(parts) == 2:
                    side[parts[1].strip()] = parts[0].strip()
            if side != {rel: e["sha256"] for rel, e in files.items()}:
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
        print(f"  produced by: {describe_producer(manifest)}")
        for limit in manifest.get("attestation", {}).get("limits", []):
            print(f"  note: {limit}")
        return VALID


def reject_concatenated_archive(path: str, zf: zipfile.ZipFile):
    """(state, message). `message` is "" when a STREAMING and a SEEKING reader see the same archive.

    THIS IS CHECKED DIRECTLY, NOT BY PROXY, AND THE DIFFERENCE IS THE WHOLE POINT.

    The previous version asserted an arithmetic stand-in: every byte of the file is covered by an
    entry the central directory names — entries tiling contiguously from 0 to the directory, the
    directory ending at the end-of-archive record, that record ending at EOF. It was defeated, and
    the defeat is instructive: byte contiguity is a WEAKER property than "the two readers agree".

    An attacker inflated one entry's *central-directory* compressed size and hid a complete local
    file header plus a forged `media/clip.mp4` inside the slack that created. The cursor still landed
    exactly on the central directory, so every byte was accounted for and the check passed — but a
    front-to-back inflater stops at the DEFLATE end-of-stream, not at the declared size, so it read
    the slack as the next member. The verifier said VALID three runs running while `cat bundle |
    tar -x` wrote "FORGED CLIP - plate ABC-999" to disk.

    The arithmetic was also wrong in the other direction, and that mattered more in practice: it read
    the classic 32-bit end-of-archive record, so any ZIP64 archive was refused. Since the appliance's
    own `zip -r -q -X -D` emits ZIP64 above 4 GiB, and a multi-hour export passes 4 GiB easily, the
    check accused the appliance's own genuine evidence of being a forgery. ZIP64 data descriptors are
    24 bytes rather than 16, which broke a second shape. Both were false refusals of real evidence —
    a verifier that rejects genuine evidence fails the investigator exactly as badly as one that
    accepts a forgery.

    So this now does the obvious thing instead of the clever one: parse the file from byte 0 the way
    a streaming extractor does, hashing what it would write, and compare that against what the
    central directory says. Equal maps and nothing left over means no reader can be shown a different
    archive than the one verified. It needs no EOCD arithmetic, so ZIP64 is simply not its problem,
    and it takes the inflater's own stopping point as the truth, so a declared size cannot lie.

    Cost: the archive is decompressed twice, once here and once when the manifest's hashes are
    checked. That is the price of the guarantee and it is worth paying for an evidence document.
    """
    try:
        streamed, entries_end = stream_view(path)
    except StreamProblem as e:
        return e.state, str(e)

    seeking = {}
    for info in zf.infolist():
        if info.filename.endswith("/") and info.file_size == 0:
            continue
        h = hashlib.sha256()
        try:
            with zf.open(info) as fh:
                for chunk in iter(lambda: fh.read(1 << 20), b""):
                    h.update(chunk)
        except (zipfile.BadZipFile, EOFError, OSError) as e:
            return MODIFIED, f"{info.filename!r} will not come out of the archive ({e})"
        seeking[info.filename] = h.hexdigest()

    if streamed != seeking:
        only_stream = sorted(set(streamed) - set(seeking))
        only_seek = sorted(set(seeking) - set(streamed))
        differ = sorted(k for k in set(streamed) & set(seeking) if streamed[k] != seeking[k])
        detail = []
        if only_stream:
            detail.append(f"a front-to-back reader also finds {', '.join(only_stream)}")
        if only_seek:
            detail.append(f"a front-to-back reader never reaches {', '.join(only_seek)}")
        if differ:
            detail.append(f"the two readers get different content for {', '.join(differ)}")
        return MALFORMED, ("this file is two different archives depending on how it is opened — "
                           + "; ".join(detail)
                           + ". Whatever this verifier attested, an extractor may write "
                             "something else")

    with open(path, "rb") as fh:
        fh.seek(entries_end)
        if fh.read(4) != b"PK\x01\x02":
            return MALFORMED, (f"the entries end at byte {entries_end}, where the archive's "
                               f"directory does not begin — bytes belong to neither")
        size = os.path.getsize(path)
        at, comment_len = classic_eocd(fh, size)
        if at is None:
            return MALFORMED, "could not locate the end of the archive"
        if at + 22 + comment_len != size:
            return MALFORMED, f"{size - (at + 22 + comment_len)} bytes follow the end of the archive"
    return VALID, ""


class StreamProblem(Exception):
    """Something a front-to-back reader cannot make sense of. Always a refusal.

    Carries the STATE, because the two causes are different accusations: a container that does not
    parse is MALFORMED, while content that will not decompress is MODIFIED — the bytes of the
    evidence changed, which is the thing an investigator needs told.
    """

    def __init__(self, message, state=MALFORMED):
        super().__init__(message)
        self.state = state


def stream_view(path: str):
    """({name: sha256}, offset where the entries end) as a FRONT-TO-BACK reader sees the file.

    This is deliberately a separate implementation from `zipfile`'s: the whole question is whether
    two independent readers agree, and asking the same reader twice cannot answer it.
    """
    seen = {}
    with open(path, "rb") as fh:
        off = 0
        while True:
            fh.seek(off)
            head = fh.read(30)
            if len(head) < 30 or head[:4] != b"PK\x03\x04":
                break
            flags = int.from_bytes(head[6:8], "little")
            method = int.from_bytes(head[8:10], "little")
            csize = int.from_bytes(head[18:22], "little")
            name_len = int.from_bytes(head[26:28], "little")
            extra_len = int.from_bytes(head[28:30], "little")
            raw_name = fh.read(name_len)
            try:
                name = raw_name.decode("utf-8" if flags & 0x800 else "cp437")
            except UnicodeDecodeError:
                raise StreamProblem(f"an entry name in the archive body is not decodable: {raw_name!r}")
            if name in seen:
                raise StreamProblem(f"a front-to-back reader meets {name!r} twice")
            extra = fh.read(extra_len)
            if csize == 0xFFFFFFFF:
                # ZIP64: the 32-bit field holds a sentinel and the real size is in extra record
                # 0x0001. Only STORED entries need it — for deflate the inflater finds the end
                # itself — but reading 0xFFFFFFFF bytes as a length is how the appliance's own
                # >4 GiB exports got refused, so it is resolved rather than trusted.
                csize = zip64_compressed_size(extra)
                if csize is None:
                    raise StreamProblem(f"{name!r} declares a ZIP64 size with no ZIP64 extra record")
            data_at = off + 30 + name_len + extra_len

            fh.seek(data_at)
            h = hashlib.sha256()
            if method == 8:
                # The INFLATER decides where the data ends, not the declared size. That is the
                # asymmetry the forgery exploited, so the truth is taken from the stream itself.
                dec = zlib.decompressobj(-15)
                consumed = 0
                while not dec.eof:
                    chunk = fh.read(1 << 20)
                    if not chunk:
                        raise StreamProblem(
                            f"the compressed data for {name!r} ends mid-stream", MODIFIED
                        )
                    try:
                        h.update(dec.decompress(chunk))
                    except zlib.error as e:
                        raise StreamProblem(
                            f"the compressed data for {name!r} is not readable ({e})", MODIFIED
                        )
                    consumed += len(chunk)
                consumed -= len(dec.unused_data)
            elif method == 0:
                if flags & 0x08 and csize == 0:
                    raise StreamProblem(
                        f"{name!r} is stored with its length only in a trailing descriptor; a "
                        f"streaming reader cannot know where it ends, so neither can this verifier"
                    )
                left = csize
                while left:
                    chunk = fh.read(min(left, 1 << 20))
                    if not chunk:
                        raise StreamProblem(f"the data for {name!r} ends early")
                    h.update(chunk)
                    left -= len(chunk)
                consumed = csize
            else:
                raise StreamProblem(f"{name!r} uses compression method {method}, which an evidence "
                                    f"bundle does not use and this verifier will not guess at")

            if not (name.endswith("/") and consumed == 0):
                seen[name] = h.hexdigest()
            off = data_at + consumed

            if flags & 0x08:
                # A data descriptor follows, in one of four widths (with or without its optional
                # signature, classic or ZIP64 sizes). Rather than guess — guessing 16 where it was
                # 24 is what refused honest streamed bundles — take the width that lands on the next
                # real record.
                fh.seek(off)
                probe = fh.read(28)
                for width in (16, 24, 12, 20):
                    if probe[width:width + 4] in (b"PK\x03\x04", b"PK\x01\x02"):
                        off += width
                        break
                else:
                    raise StreamProblem(f"the data descriptor after {name!r} is not one this "
                                        f"verifier recognises")
    if not seen:
        return {}, 0
    return seen, off


def zip64_compressed_size(extra: bytes):
    """The compressed size from a local header's ZIP64 extra record (0x0001), or None."""
    at = 0
    while at + 4 <= len(extra):
        hid = int.from_bytes(extra[at:at + 2], "little")
        n = int.from_bytes(extra[at + 2:at + 4], "little")
        body = extra[at + 4:at + 4 + n]
        # In a LOCAL header the record carries uncompressed then compressed size, both 8 bytes and
        # both always present (unlike the central-directory form, where fields are omitted when the
        # 32-bit slot sufficed).
        if hid == 0x0001 and len(body) >= 16:
            return int.from_bytes(body[8:16], "little")
        at += 4 + n
    return None


def classic_eocd(fh, size: int):
    """(offset, comment_len) of the last end-of-central-directory record, or (None, None).

    Only used for the "nothing follows the archive" check. The classic record is last even in a
    ZIP64 archive, so no ZIP64 parsing is needed here — and none is done, because the previous
    version's attempt to do arithmetic on this record is what refused honest ZIP64 bundles.
    """
    span = min(size, 66 * 1024)
    fh.seek(size - span)
    tail = fh.read(span)
    at = tail.rfind(b"PK\x05\x06")
    if at < 0 or len(tail) - at < 22:
        return None, None
    return size - span + at, int.from_bytes(tail[at + 20:at + 22], "little")


def reject_unsafe_names(names: list) -> str:
    """A refusal message, or "" if every entry name is one no extractor will reinterpret.

    Everything here is about the gap between what THIS program reads out of the archive and what the
    operating system writes when someone unzips it. Any name where those two differ is a place a
    forged file can hide behind a verified one, so each is refused outright rather than resolved.
    """
    seen = set()
    for n in names:
        # A duplicate name means the archive holds two different byte streams for one path. Readers
        # disagree about which one wins — `zipfile` returns the last central-directory entry,
        # `unzip -p` concatenates both — so there is no single thing to attest to.
        if n in seen:
            return (f"the archive contains {n!r} more than once; readers disagree about which copy "
                    f"is the real one, so no signature over 'that file' means anything")
        seen.add(n)

        if not n or n != n.strip():
            return f"entry name {n!r} has leading or trailing whitespace"
        if n.startswith("/") or (len(n) > 1 and n[1] == ":"):
            return f"entry name {n!r} is an absolute path"
        if "\\" in n:
            return (f"entry name {n!r} contains a backslash, which is a path separator on Windows "
                    f"and a literal character here — the two extract differently")
        parts = n.split("/")
        # A trailing "" is the directory marker and is legitimate; anything else empty is "//".
        body = parts[:-1] if parts[-1] == "" else parts
        for p in body:
            if p in ("", ".", ".."):
                return (f"entry name {n!r} contains a {p!r} path component; it resolves to a "
                        f"different file than it is stored under, which is how a forged copy hides "
                        f"behind a verified one")
        # NUL and control characters truncate paths in some tools.
        if any(ord(c) < 0x20 for c in n):
            return f"entry name {n!r} contains a control character"
    return ""


def is_dir_entry(name: str, expected: set, size: int) -> bool:
    """A genuinely EMPTY directory marker for a directory an expected file actually lives in.

    `zip -r` writes `media/` and `metadata/`. They are tolerated because they carry nothing and
    create only directories that have to exist anyway. Both conditions are checked: a `media/` entry
    with content is not a directory marker, and a bare `anything/` would still appear as a folder in
    the investigator's extracted tree with nothing signed saying it belongs.
    """
    return name.endswith("/") and size == 0 and any(f.startswith(name) for f in expected)


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


def describe_producer(m: dict) -> str:
    """What made this bundle, for a reader deciding how much to trust the media.

    `ffmpeg_version` may be null: the appliance records what it could ask the binary, and null means
    it could not. That is reported as "unidentified" rather than omitted — a reader who is not told
    assumes it was recorded, and the manifest is signed, so silence here would be the appliance
    quietly declining to say something it appeared to promise.
    """
    p = m.get("producer") or {}
    heldar = p.get("heldar_version") or "unknown release"
    ffmpeg = p.get("ffmpeg_version") or "unidentified ffmpeg (the appliance could not ask it)"
    schema = p.get("schema_version")
    s = f"Heldar {heldar}, {ffmpeg}"
    if schema is not None:
        s += f", schema {schema}"
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
    # A LAST RESORT, NOT A SUBSTITUTE FOR THE CHECKS ABOVE.
    #
    # Every crash found in this file exited 1 — the same code as MODIFIED — so an unhandled type
    # error, an undecodable entry name, or a truncated deflate stream all reported themselves as
    # "the evidence was altered". That is a false accusation with the same exit code as a true one,
    # and a caller cannot tell them apart. Anything unanticipated is MALFORMED: this program did not
    # establish a state, and must not be read as having established one.
    try:
        sys.exit(main())
    except SystemExit:
        raise
    except BaseException as e:  # noqa: BLE001 - deliberately total
        out("MALFORMED", f"the bundle could not be processed ({type(e).__name__}: {e}) — no "
                         f"conclusion was reached about it, which is NOT the same as finding it "
                         f"unaltered or finding it tampered with")
        sys.exit(MALFORMED)
