#!/usr/bin/env python3
"""Report deviations from the hardened container profile (issue #116).

`deploy/compose.hardened.yml` was booted and checked by hand, once. Nothing has re-checked it since,
and container hardening rots in a particular way: a service gains a writable path, someone adds a
capability back to fix a crash, an overlay stops being layered in the deploy command — and the stack
keeps working perfectly, which is exactly why nobody notices. The posture is only observable if
something looks.

This reads a RENDERED compose configuration — `docker compose -f ... config` — rather than the
overlay files. Re-implementing Compose's merge semantics here would mean auditing a posture that
Compose might not actually produce, and being subtly wrong about it in the safe direction is the
worst possible outcome for a security check.

Output is `heldarctl doctor`'s Finding shape (code / severity / resource / detail / remediation), so
`--format json` can be consumed by the same tooling. Severities follow doctor's convention: Blocking
means the boundary an operator believes in is not there; Warning means degraded but running; Info
records something that could not be checked, because "unverified" is not "fine".
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent

# Services that legitimately keep a writable root filesystem, and why. An exemption is DECLARED
# here, so a service losing `read_only` by accident still fails — the difference between a decision
# and a regression is that somebody wrote the decision down.
READ_ONLY_EXEMPT = {
    "mediamtx": "generates a self-signed TLS keypair into its working directory at startup",
    "ai": "downloads model weights to the home-directory cache on first use of an optional profile",
}

# Capabilities a service may add back, having proved it needs them. Anything else is a finding.
CAP_ADD_ALLOWED = {
    "web": {"CHOWN", "SETUID", "SETGID"},  # nginx entrypoint chowns cache dirs, then drops privilege
}

# Services with no in-container health check, and why. mediamtx is a single-layer image whose
# entrypoint is the bare binary: no shell, no curl, nothing a `test:` could exec.
HEALTHCHECK_EXEMPT = {
    "mediamtx": "single-layer image with no shell or HTTP client; liveness is observed by core "
                "through its API on 127.0.0.1:9997 and surfaced via /readyz",
}

REQUIRED_TMPFS_FLAGS = ("noexec", "nosuid", "nodev")

# Host paths that hand over the box no matter what else is set. A container with the docker socket
# can start a privileged sibling; one with host root can edit anything the kernel reads.
FATAL_BIND_SOURCES = {
    "/var/run/docker.sock": "the Docker socket is root-equivalent control of the host",
    "/run/docker.sock": "the Docker socket is root-equivalent control of the host",
    "/": "the entire host filesystem",
    "/etc": "the host's system configuration",
    "/proc": "the host's process tree",
    "/sys": "the host's kernel interfaces",
    "/dev": "the host's devices",
}

# Namespaces that, if shared with the host, remove the boundary the rest of this file is protecting.
HOST_NAMESPACE_KEYS = {
    "pid": "the host process tree — every process on the box is visible and signallable",
    "ipc": "the host IPC namespace — shared memory belonging to other services",
    "userns_mode": "the host user namespace — uid mapping stops isolating anything",
    "cgroup": "the host cgroup namespace",
}


def _is_unlimited(value) -> bool:
    """Docker reads 0 (and "0", "0b", "0.0") as no limit at all."""
    text = str(value).strip().lower().rstrip("bkmgt")
    try:
        return float(text or "0") == 0.0
    except ValueError:
        return False


def finding(code, severity, detail, remediation, resource=None):
    f = {"code": code, "severity": severity, "detail": detail, "remediation": remediation}
    if resource is not None:
        f["resource"] = resource
    return f


def _tmpfs_entries(svc: dict) -> list[str]:
    """Every tmpfs mount, in BOTH Compose syntaxes.

    Reading only the short-form `tmpfs:` key was a real gap: a service mounting scratch space as
    `volumes: [{type: tmpfs, ...}]` produced no findings at all, with no noexec and no size bound.
    A tool whose premise is "an undeclared exemption is a regression" cannot have an undeclared
    blind spot of its own.
    """
    t = svc.get("tmpfs") or []
    entries = [t] if isinstance(t, str) else list(t)

    for v in svc.get("volumes") or []:
        if not isinstance(v, dict) or v.get("type") != "tmpfs":
            continue
        # Normalise the long form into the short form's "target:opt,opt" shape so one set of checks
        # covers both, rather than two implementations that can disagree.
        opts = []
        tmpfs_opts = v.get("tmpfs") or {}
        if tmpfs_opts.get("size") is not None:
            opts.append(f"size={tmpfs_opts['size']}")
        # The long form expresses these as booleans/flags rather than a comma string.
        for flag in REQUIRED_TMPFS_FLAGS:
            if tmpfs_opts.get(flag) or flag in str(tmpfs_opts.get("mode", "")):
                opts.append(flag)
        entries.append(f"{v.get('target', '?')}:{','.join(opts)}")
    return entries


def check_service(name: str, svc: dict) -> list[dict]:
    out: list[dict] = []

    # --- the settings that make every other setting moot -------------------------------------
    # Checked FIRST and hardest. `privileged: true` grants every capability and every device back,
    # so a service can carry cap_drop ALL, no-new-privileges and a read-only root and still be
    # root on the box. A checker that reports that service as hardened is worse than no checker:
    # it converts "nobody looked" into "we verified it".
    if svc.get("privileged"):
        out.append(finding(
            "compose_privileged", "blocking",
            f"{name} runs privileged, which restores every capability and every host device "
            f"regardless of cap_drop, no-new-privileges or a read-only root",
            "Remove `privileged: true` and grant only the specific devices or capabilities a boot "
            "proves are needed", name))

    for key, what in HOST_NAMESPACE_KEYS.items():
        val = str(svc.get(key) or "")
        if val == "host":
            out.append(finding(
                "compose_host_namespace", "blocking",
                f"{name} sets {key}: host, sharing {what}. The stack already gives up the network "
                f"namespace for camera discovery; giving up this one too leaves no boundary at all",
                f"Remove `{key}: host` unless a documented requirement needs it", name))

    for v in svc.get("volumes") or []:
        src = v.get("source") if isinstance(v, dict) else str(v).split(":")[0]
        if not src:
            continue
        why = FATAL_BIND_SOURCES.get(str(src).rstrip("/") or "/")
        if why:
            rw = "ro" not in str(v) if not isinstance(v, dict) else not v.get("read_only")
            out.append(finding(
                "compose_fatal_bind_mount", "blocking",
                f"{name} bind-mounts {src} ({why}){' read-write' if rw else ''}; a read-only root "
                f"filesystem means nothing next to it",
                "Remove the mount, or narrow it to the specific path the service needs", name))

    # --- no-new-privileges -------------------------------------------------------------------
    sec = svc.get("security_opt") or []
    if "no-new-privileges:true" not in sec:
        out.append(finding(
            "compose_no_new_privileges_missing", "blocking",
            f"{name} does not set no-new-privileges, so a setuid binary inside it can still "
            f"escalate despite the dropped capabilities",
            "Add `security_opt: [\"no-new-privileges:true\"]` in deploy/compose.hardened.yml",
            name))

    # --- capabilities ------------------------------------------------------------------------
    cap_drop = {c.upper() for c in (svc.get("cap_drop") or [])}
    if "ALL" not in cap_drop:
        out.append(finding(
            "compose_caps_not_dropped", "blocking",
            f"{name} does not drop ALL capabilities; it keeps Docker's default set, which "
            f"includes CHOWN, SETUID, SETGID, NET_RAW and more",
            "Add `cap_drop: [\"ALL\"]`, then add back only what a boot proves is needed",
            name))

    allowed = CAP_ADD_ALLOWED.get(name, set())
    unexpected = {c.upper() for c in (svc.get("cap_add") or [])} - allowed
    if unexpected:
        out.append(finding(
            "compose_unexpected_capability", "blocking",
            f"{name} adds back {sorted(unexpected)}, which is not in its reviewed allowlist "
            f"({sorted(allowed) or 'none'})",
            "Prove the capability is required and add it to CAP_ADD_ALLOWED with the error it "
            "fixes, or remove it",
            name))

    # --- read-only root ----------------------------------------------------------------------
    if not svc.get("read_only"):
        if name in READ_ONLY_EXEMPT:
            out.append(finding(
                "compose_read_only_exempt", "info",
                f"{name} keeps a writable root filesystem: {READ_ONLY_EXEMPT[name]}",
                "Re-test with read_only if the underlying reason ever goes away", name))
        else:
            out.append(finding(
                "compose_root_writable", "blocking",
                f"{name} has a writable root filesystem, so anything that lands in the container "
                f"can persist itself for the life of that container",
                "Set `read_only: true` and give it a tmpfs or named volume for what it must write",
                name))

    # --- tmpfs hygiene -----------------------------------------------------------------------
    for entry in _tmpfs_entries(svc):
        missing = [f for f in REQUIRED_TMPFS_FLAGS if f not in entry]
        if missing:
            out.append(finding(
                "compose_tmpfs_flags_missing", "warning",
                f"{name} mounts {entry.split(':')[0]} without {', '.join(missing)}",
                "A scratch directory is the first place a dropper writes; mount it "
                "noexec,nosuid,nodev and give it a size", name))
        if "size=" not in entry:
            out.append(finding(
                "compose_tmpfs_unbounded", "warning",
                f"{name} mounts {entry.split(':')[0]} with no size limit, so it can consume host "
                f"memory until the box stops recording",
                "Add `size=` to the tmpfs options", name))

    # --- bounded logs ------------------------------------------------------------------------
    opts = ((svc.get("logging") or {}).get("options")) or {}
    if not opts.get("max-size") or not opts.get("max-file"):
        out.append(finding(
            "compose_logs_unbounded", "warning",
            f"{name} has no max-size/max-file, so its logs can fill the same disk the recordings "
            f"need — a camera flapping at 3am fills it fastest",
            "Set logging.options.max-size and max-file", name))

    # --- resource ceilings -------------------------------------------------------------------
    limits = (((svc.get("deploy") or {}).get("resources") or {}).get("limits")) or {}
    for key, why in (("cpus", "a runaway can starve the recorder"),
                     ("memory", "a leak takes the host down with it"),
                     ("pids", "a fork bomb needs no privileges at all")):
        if key not in limits:
            out.append(finding(
                "compose_resource_limit_missing", "warning",
                f"{name} sets no {key} limit; {why}",
                f"Set deploy.resources.limits.{key}", name))
        elif _is_unlimited(limits[key]):
            # Presence is not a ceiling. Docker reads 0 as "unlimited", so `pids: 0` satisfies a
            # key-presence check while leaving the fork bomb exactly as effective.
            out.append(finding(
                "compose_resource_limit_unlimited", "warning",
                f"{name} sets {key}={limits[key]!r}, which Docker treats as UNLIMITED; {why}",
                f"Give deploy.resources.limits.{key} a real ceiling", name))

    # --- health ------------------------------------------------------------------------------
    if not svc.get("healthcheck"):
        if name in HEALTHCHECK_EXEMPT:
            out.append(finding(
                "compose_healthcheck_exempt", "info",
                f"{name} has no in-container health check: {HEALTHCHECK_EXEMPT[name]}",
                "None; this is a property of the image, not of the deployment", name))
        else:
            out.append(finding(
                "compose_healthcheck_missing", "warning",
                f"{name} has no health check, so a wedged process is indistinguishable from a "
                f"working one and dependents start against it anyway",
                "Add a healthcheck that probes what the service actually does", name))
    elif (svc.get("healthcheck") or {}).get("disable"):
        out.append(finding(
            "compose_healthcheck_disabled", "warning",
            f"{name} explicitly disables its health check",
            "Remove `disable: true`", name))

    return out


def check_mediamtx_config(path: Path) -> list[dict]:
    """The admin API must stay on loopback.

    The stack runs `network_mode: host`, so there is no network namespace between this port and the
    camera VLAN. A MediaMTX API on 0.0.0.0 lets anything that can reach the box reconfigure paths
    and read stream credentials.
    """
    if not path.exists():
        return [finding("mediamtx_config_missing", "warning",
                        f"{path} not found, so its admin binding could not be checked",
                        "Point --mediamtx at the config the deployment actually mounts")]
    cfg = yaml.safe_load(path.read_text()) or {}
    out = []
    for key, what in (("apiAddress", "control API"), ("metricsAddress", "metrics"),
                      ("pprofAddress", "pprof profiler")):
        addr = cfg.get(key)
        if addr is None:
            continue
        host = str(addr).rsplit(":", 1)[0]
        if host not in ("127.0.0.1", "localhost", "[::1]"):
            out.append(finding(
                "mediamtx_admin_exposed", "blocking",
                f"MediaMTX {what} listens on {addr}; with host networking that is reachable from "
                f"every network the box is on, including the camera VLAN",
                f"Bind {key} to 127.0.0.1", "mediamtx"))
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--config", required=True,
                    help="rendered compose config ('docker compose -f ... config'), or - for stdin")
    ap.add_argument("--mediamtx", default=str(ROOT / "deploy/mediamtx.yml"))
    ap.add_argument("--format", choices=("text", "json"), default="text")
    args = ap.parse_args()

    raw = sys.stdin.read() if args.config == "-" else Path(args.config).read_text()
    services = (yaml.safe_load(raw) or {}).get("services") or {}
    if not services:
        print("::error::rendered config has no services — did `docker compose config` succeed?",
              file=sys.stderr)
        return 2

    findings = [f for name, svc in sorted(services.items()) for f in check_service(name, svc)]
    findings += check_mediamtx_config(Path(args.mediamtx))

    if args.format == "json":
        print(json.dumps({"findings": findings}, indent=2))
    else:
        for f in findings:
            mark = {"blocking": "::error::", "warning": "::warning::", "info": "  info: "}[f["severity"]]
            print(f"{mark}[{f['code']}] {f['detail']}")
            if f["severity"] != "info":
                print(f"    -> {f['remediation']}")
        counts = {s: sum(1 for f in findings if f["severity"] == s)
                  for s in ("blocking", "warning", "info")}
        print(f"\n{len(services)} service(s): "
              f"{counts['blocking']} blocking, {counts['warning']} warning, {counts['info']} info")

    # Info never fails: it records what was not checked, and treating "unverified" as a failure is
    # as wrong as treating it as a pass. Warnings do not block either — a degraded box still records.
    return 1 if any(f["severity"] == "blocking" for f in findings) else 0


if __name__ == "__main__":
    sys.exit(main())
