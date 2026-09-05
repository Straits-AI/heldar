"""Controls for scripts/check_hardened_profile.py.

A hardening check that cannot fail is worse than none: it converts "nobody looked" into "we verified
it", which is the state this whole check exists to escape. Every control below removes exactly one
boundary from an otherwise-hardened service and asserts the checker names it, at the right severity.

Fixture-based on purpose. CI separately renders the REAL overlays with `docker compose config` and
runs the checker against them — both the hardened stack (must pass) and the base stack alone (must
FAIL). That pair is the end-to-end proof; these are the unit-level ones.
"""

from __future__ import annotations

import copy
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_hardened_profile as mod  # noqa: E402

# A service with every boundary in place. Each control below breaks one thing.
HARDENED = {
    "read_only": True,
    "security_opt": ["no-new-privileges:true"],
    "cap_drop": ["ALL"],
    "tmpfs": ["/tmp:rw,noexec,nosuid,nodev,size=64m"],
    "logging": {"driver": "json-file", "options": {"max-size": "10m", "max-file": "3"}},
    "deploy": {"resources": {"limits": {"cpus": "2.0", "memory": "1g", "pids": 512}}},
    "healthcheck": {"test": ["CMD", "true"]},
}


def check(mutate=None, name="core"):
    svc = copy.deepcopy(HARDENED)
    if mutate:
        mutate(svc)
    return mod.check_service(name, svc)


def codes(findings):
    return {f["code"] for f in findings}


def sev(findings, code):
    return next(f["severity"] for f in findings if f["code"] == code)


def _expect(findings, code, severity, label):
    assert code in codes(findings), f"{label}: expected {code}, got {sorted(codes(findings))}"
    got = sev(findings, code)
    assert got == severity, f"{label}: {code} was {got}, expected {severity}"


def a_fully_hardened_service_is_clean():
    """The baseline. Without it, every control below could be passing for the wrong reason."""
    assert check() == [], check()


def a_writable_root_blocks():
    _expect(check(lambda s: s.pop("read_only")), "compose_root_writable", "blocking", "read_only")
    _expect(check(lambda s: s.update(read_only=False)), "compose_root_writable", "blocking", "false")


def a_declared_read_only_exemption_is_info_not_blocking():
    """The difference between a decision and a regression is that somebody wrote the decision down."""
    for name in mod.READ_ONLY_EXEMPT:
        f = check(lambda s: s.pop("read_only"), name=name)
        _expect(f, "compose_read_only_exempt", "info", name)
        assert "compose_root_writable" not in codes(f), f


def missing_no_new_privileges_blocks():
    _expect(check(lambda s: s.pop("security_opt")),
            "compose_no_new_privileges_missing", "blocking", "absent")
    _expect(check(lambda s: s.update(security_opt=["seccomp=unconfined"])),
            "compose_no_new_privileges_missing", "blocking", "other opt only")


def not_dropping_all_capabilities_blocks():
    _expect(check(lambda s: s.pop("cap_drop")), "compose_caps_not_dropped", "blocking", "absent")
    _expect(check(lambda s: s.update(cap_drop=["NET_RAW"])),
            "compose_caps_not_dropped", "blocking", "partial drop is not ALL")


def an_unreviewed_capability_blocks():
    """Adding a cap back to fix a crash is the most likely way this posture erodes."""
    _expect(check(lambda s: s.update(cap_add=["SYS_ADMIN"])),
            "compose_unexpected_capability", "blocking", "core")
    # web's three reviewed caps are fine; a fourth is not.
    assert check(lambda s: s.update(cap_add=["CHOWN", "SETUID", "SETGID"]), name="web") == []
    _expect(check(lambda s: s.update(cap_add=["CHOWN", "SYS_PTRACE"]), name="web"),
            "compose_unexpected_capability", "blocking", "web extra")


def sloppy_tmpfs_flags_warn():
    _expect(check(lambda s: s.update(tmpfs=["/tmp:rw,nosuid,nodev,size=64m"])),
            "compose_tmpfs_flags_missing", "warning", "no noexec")
    _expect(check(lambda s: s.update(tmpfs=["/tmp:rw,noexec,nosuid,nodev"])),
            "compose_tmpfs_unbounded", "warning", "no size")


def unbounded_logs_warn():
    _expect(check(lambda s: s.pop("logging")), "compose_logs_unbounded", "warning", "absent")
    _expect(check(lambda s: s.update(logging={"options": {"max-size": "10m"}})),
            "compose_logs_unbounded", "warning", "max-file missing")


def each_missing_resource_limit_warns():
    for key in ("cpus", "memory", "pids"):
        f = check(lambda s, k=key: s["deploy"]["resources"]["limits"].pop(k))
        _expect(f, "compose_resource_limit_missing", "warning", key)
        assert key in next(x["detail"] for x in f if x["code"] == "compose_resource_limit_missing")


def a_missing_healthcheck_warns_unless_declared_exempt():
    _expect(check(lambda s: s.pop("healthcheck")),
            "compose_healthcheck_missing", "warning", "core")
    for name in mod.HEALTHCHECK_EXEMPT:
        f = check(lambda s: s.pop("healthcheck"), name=name)
        _expect(f, "compose_healthcheck_exempt", "info", name)
        assert "compose_healthcheck_missing" not in codes(f), f
    _expect(check(lambda s: s.update(healthcheck={"disable": True})),
            "compose_healthcheck_disabled", "warning", "disabled")


def an_exposed_mediamtx_admin_port_blocks(tmp=Path("/tmp")):
    """With host networking there is no namespace between this port and the camera VLAN."""
    import tempfile, yaml
    with tempfile.TemporaryDirectory() as d:
        p = Path(d) / "mediamtx.yml"
        p.write_text(yaml.dump({"apiAddress": "0.0.0.0:9997"}))
        _expect(mod.check_mediamtx_config(p), "mediamtx_admin_exposed", "blocking", "0.0.0.0")

        p.write_text(yaml.dump({"apiAddress": ":9997"}))
        _expect(mod.check_mediamtx_config(p), "mediamtx_admin_exposed", "blocking", "bare port")

        p.write_text(yaml.dump({"apiAddress": "127.0.0.1:9997",
                                "metricsAddress": "0.0.0.0:9998"}))
        _expect(mod.check_mediamtx_config(p), "mediamtx_admin_exposed", "blocking", "metrics")

        p.write_text(yaml.dump({"apiAddress": "127.0.0.1:9997"}))
        assert mod.check_mediamtx_config(p) == [], "loopback must be clean"

        assert "mediamtx_config_missing" in codes(mod.check_mediamtx_config(Path(d) / "nope.yml"))


def the_real_mediamtx_config_is_loopback_bound():
    """The shipped file, not a fixture."""
    f = mod.check_mediamtx_config(mod.ROOT / "deploy/mediamtx.yml")
    assert not [x for x in f if x["severity"] == "blocking"], f



def privileged_defeats_everything_and_must_block():
    """`privileged: true` restores every capability and device, making the rest of this file moot.

    An independent review built a service that was privileged, PID-host, docker-socket-mounted and
    host-root-mounted, and the checker called it clean. That is worse than no checker: it converts
    "nobody looked" into "we verified it".
    """
    f = check(lambda s: s.update(privileged=True))
    _expect(f, "compose_privileged", "blocking", "privileged")


def sharing_a_host_namespace_blocks():
    for key in ("pid", "ipc", "cgroup", "userns_mode"):
        f = check(lambda s, k=key: s.update({k: "host"}))
        _expect(f, "compose_host_namespace", "blocking", key)
    # a non-host value is fine
    assert check(lambda s: s.update(pid="container:other")) == []


def a_fatal_bind_mount_blocks():
    """The docker socket is root-equivalent; host root makes a read-only rootfs meaningless."""
    for src in ("/var/run/docker.sock", "/run/docker.sock", "/", "/etc", "/proc", "/sys", "/dev"):
        f = check(lambda s, x=src: s.update(volumes=[f"{x}:/mnt:rw"]))
        _expect(f, "compose_fatal_bind_mount", "blocking", src)
    # the long syntax too
    _expect(check(lambda s: s.update(
        volumes=[{"type": "bind", "source": "/var/run/docker.sock", "target": "/sock"}])),
        "compose_fatal_bind_mount", "blocking", "long syntax")
    # an ordinary named volume or narrow bind is not a finding
    assert check(lambda s: s.update(volumes=["heldar-data:/data"])) == []
    assert check(lambda s: s.update(volumes=["/srv/heldar/recordings:/data:rw"])) == []


def the_whole_review_escape_case_is_caught():
    """The exact service the reviewer showed passing. It must now produce blocking findings."""
    def m(s):
        s.update(privileged=True, pid="host",
                 volumes=["/var/run/docker.sock:/var/run/docker.sock", "/:/host:rw"],
                 deploy={"resources": {"limits": {"cpus": "0", "memory": "0", "pids": 0}}})
    f = check(m, name="ai")
    blocking = {x["code"] for x in f if x["severity"] == "blocking"}
    assert {"compose_privileged", "compose_host_namespace", "compose_fatal_bind_mount"} <= blocking, f


def a_zero_resource_limit_is_not_a_limit():
    """Docker reads 0 as unlimited, so presence-only checking left the fork bomb intact."""
    for key, val in (("pids", 0), ("cpus", "0"), ("memory", "0"), ("memory", "0b"), ("cpus", "0.0")):
        f = check(lambda s, k=key, v=val: s["deploy"]["resources"]["limits"].update({k: v}))
        _expect(f, "compose_resource_limit_unlimited", "warning", f"{key}={val}")
    # real ceilings stay clean
    assert check(lambda s: s["deploy"]["resources"]["limits"].update(
        {"cpus": "0.5", "memory": "512m", "pids": 128})) == []


def long_form_tmpfs_is_checked_too():
    """A tool whose premise is 'an undeclared exemption is a regression' cannot have its own."""
    def m(s):
        s.pop("tmpfs")
        s["volumes"] = [{"type": "tmpfs", "target": "/tmp", "tmpfs": {}}]
    f = check(m)
    _expect(f, "compose_tmpfs_flags_missing", "warning", "long form, no flags")
    _expect(f, "compose_tmpfs_unbounded", "warning", "long form, no size")

    def ok(s):
        s.pop("tmpfs")
        s["volumes"] = [{"type": "tmpfs", "target": "/tmp",
                         "tmpfs": {"size": "64m", "noexec": True, "nosuid": True, "nodev": True}}]
    assert check(ok) == [], check(ok)


CHECKS = [
    a_fully_hardened_service_is_clean,
    a_writable_root_blocks,
    a_declared_read_only_exemption_is_info_not_blocking,
    missing_no_new_privileges_blocks,
    not_dropping_all_capabilities_blocks,
    an_unreviewed_capability_blocks,
    sloppy_tmpfs_flags_warn,
    unbounded_logs_warn,
    each_missing_resource_limit_warns,
    a_missing_healthcheck_warns_unless_declared_exempt,
    an_exposed_mediamtx_admin_port_blocks,
    the_real_mediamtx_config_is_loopback_bound,
    privileged_defeats_everything_and_must_block,
    sharing_a_host_namespace_blocks,
    a_fatal_bind_mount_blocks,
    the_whole_review_escape_case_is_caught,
    a_zero_resource_limit_is_not_a_limit,
    long_form_tmpfs_is_checked_too,
]

if __name__ == "__main__":
    for fn in CHECKS:
        fn()
        print(f"  ok    {fn.__name__.replace('_', ' ')}")
    print(f"\nall {len(CHECKS)} hardened-profile controls passed")
