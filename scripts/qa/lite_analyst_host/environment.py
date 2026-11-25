"""Observe the disposable outer and worker namespaces without changing either."""
import ctypes
import errno
import os
import socket
import subprocess
import time
from pathlib import Path

from protocol import CANARY, Failure, need, wire

PACKAGE = Path("/fixture/corral")
SCRATCH = Path("/scratch")
BWRAP = "/usr/bin/bwrap"
DAEMON_ENV = {"PATH": "/usr/bin", "LC_ALL": "C", "HOME": "/scratch/outer-home",
              "XDG_CONFIG_HOME": "/scratch/outer-home/config",
              "XDG_DATA_HOME": "/scratch/outer-home/data",
              "XDG_CACHE_HOME": "/scratch/outer-home/cache", "TMPDIR": "/tmp"}


class Unsupported(Failure):
    pass


class Teardown(Failure):
    pass


def proc_status(pid):
    return dict(line.split(":", 1) for line in Path(f"/proc/{pid}/status").read_text().splitlines())


def preflight():
    def check(value, code):
        if not value:
            raise Unsupported(code)

    status = proc_status("self")
    check(os.getuid() != 0 and status["NoNewPrivs"].strip() == "1"
          and int(status["CapEff"], 16) == 0, "outer_privileges")
    check(Path.cwd() == SCRATCH and not (SCRATCH / ".git").exists() and not Path("/home").exists(),
          "non_repo_scratch")
    net = [line.split(":")[0].strip() for line in Path("/proc/net/dev").read_text().splitlines() if ":" in line]
    check(net == ["lo"], "outer_not_loopback_only")
    for family, remote in [(socket.AF_INET, ("192.0.2.1", 80)), (socket.AF_INET6, ("2001:db8::1", 80))]:
        try:
            with socket.socket(family, socket.SOCK_DGRAM) as conn:
                conn.connect(remote)  # Routing query only; no payload is sent.
        except OSError as exc:
            check(exc.errno in (errno.EAFNOSUPPORT, errno.EADDRNOTAVAIL, errno.ENETUNREACH), "outer_route_error")
        else:
            raise Unsupported("outer_external_route")
    abi = ctypes.CDLL(None, use_errno=True).syscall(444, 0, 0, 1)
    check(abi >= 4, "landlock_tcp_abi")
    # Probe nested namespaces with a trusted system executable, not a product
    # fallback. The actual C3 inner mounts and seccomp are applied by Corral.
    probe = subprocess.run([BWRAP, "--unshare-all", "--unshare-user", "--disable-userns", "--cap-drop", "ALL",
                            "--die-with-parent", "--clearenv", "--ro-bind", "/usr", "/usr",
                            "--symlink", "usr/lib", "/lib", "--symlink", "usr/lib64", "/lib64",
                            "--proc", "/proc", "--dev", "/dev", "--", "/usr/bin/true"],
                           cwd=SCRATCH, env={}, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, timeout=5, check=False)
    check(probe.returncode == 0, "inner_namespace_preflight")
    return abi


def seed_canaries():
    (SCRATCH / "AGENTS.md").write_text(CANARY + "\n")
    extension = SCRATCH / ".pi/extensions/owner.mjs"
    extension.parent.mkdir(mode=0o700, parents=True)
    extension.write_text(f'throw new Error("{CANARY}");\n')
    # Closed Pi must neither discover this MCP nor try to execute the ambient command.
    (SCRATCH / ".pi/mcp.json").write_bytes(wire({"mcpServers": {"owner_canary": {
        "command": "/usr/bin/touch", "args": ["/scratch/owner-command-ran", CANARY],
    }}}))
    for field in ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
        Path(DAEMON_ENV[field]).mkdir(mode=0o700)
    (Path(DAEMON_ENV["HOME"]) / "AGENTS.md").write_text(CANARY)
    (SCRATCH / "owner-fd").write_text(CANARY)


def other_pids():
    ours = {1, os.getpid()}
    return {int(entry.name) for entry in Path("/proc").iterdir() if entry.name.isdigit()} - ours


def observe_payloads():
    """Snapshot the actual Pi/shim while the first provider POST is blocked here."""
    names = {"corral-pi-lite-analyst", "corral-lite-mcp-shim"}
    seen = set()
    namespaces = {name: {os.readlink(f"/proc/self/ns/{name}")} for name in ("pid", "mnt", "net")}
    baseline_filters = int(proc_status("self")["Seccomp_filters"].strip())
    pi_environment = {"LC_ALL=C.UTF-8", "PATH=/nonexistent", "HOME=/scratch/home", "TMPDIR=/scratch/tmp",
                      "XDG_CONFIG_HOME=/scratch/config", "XDG_DATA_HOME=/scratch/data", "XDG_CACHE_HOME=/scratch/cache",
                      "XDG_STATE_HOME=/scratch/state", "XDG_RUNTIME_DIR=/scratch/run"}
    for pid in other_pids():
        proc = Path(f"/proc/{pid}")
        cmd = (proc / "cmdline").read_bytes().split(b"\0")
        entry = cmd[0].decode("utf-8")
        component = Path(entry).name
        if component not in names:
            continue
        need(component not in seen and cmd == [f"{PACKAGE}/bin/{component}".encode(), b""], "payload_argv")
        for name, identities in namespaces.items():
            ns = (proc / "ns" / name).readlink()
            need(str(ns) not in identities, "payload_shared_namespace")
            identities.add(str(ns))
        status = proc_status(pid)
        need(int(status["CapEff"], 16) == 0 and status["NoNewPrivs"].strip() == "1"
             and status["Seccomp"].strip() == "2" and int(status["Seccomp_filters"].strip()) > baseline_filters,
             "payload_privileges_or_seccomp")
        need(str((proc / "cwd").readlink()) == "/scratch", "payload_cwd")
        interfaces = [line.split(":")[0].strip() for line in (proc / "net/dev").read_text().splitlines() if ":" in line]
        ipv4_routes = (proc / "net/route").read_text().splitlines()[1:]
        need(interfaces == ["lo"] and all(row.split()[0] == "lo" and row.split()[1] != "00000000"
                                          for row in ipv4_routes), "payload_route")
        mountinfo = [line.split() for line in (proc / "mountinfo").read_text().splitlines()]
        mounts = {fields[4]: fields[5].split(",") for fields in mountinfo}
        need({"/", "/scratch", "/proc", "/dev", "/lib", "/lib64", entry} <= mounts.keys()
             and all(path.startswith("/dev/") or path in {"/", "/scratch", "/proc", "/dev", "/lib", "/lib64", entry}
                     for path in mounts)
             and all("ro" in mounts[path] for path in ("/", "/lib", "/lib64", entry)), "payload_mount_map")
        root = proc / "root"
        for unwanted in ("state", "vault", "home", "usr", "qa", "fixture/meta", "scratch/AGENTS.md",
                         "scratch/.pi", "scratch/outer-home", "scratch/owner-fd"):
            need(not (root / unwanted).exists(), "payload_ambient_mount")
        env = set((proc / "environ").read_bytes().decode().split("\0")) - {"", "PWD=/scratch"}
        is_pi = component == "corral-pi-lite-analyst"
        need(env == (pi_environment if is_pi else set()), "payload_environment")
        links = {f.name: str(f.readlink()) for f in (proc / "fd").iterdir()}
        need(links.get("0", "").startswith("socket:[") and links.get("1") == links["0"]
             and links.get("2") == "/dev/null", "payload_stdio_inventory")
        allowed = {"0", "1", "2"}
        if is_pi:
            need(all(fd in allowed or value.startswith(("anon_inode:", "pipe:[", "/dev/", "/proc/"))
                     or value in (links["0"], entry) for fd, value in links.items()), "pi_ambient_descriptor")
        else:
            extra = [link for fd, link in links.items() if fd not in allowed]
            need(len(extra) == 2 and len(set(extra + [links["0"]])) == 3
                 and all(link.startswith("socket:[") for link in extra), "shim_descriptor_roles")
        seen.add(component)
    need(seen == names, "actual_payloads_not_observed")


def wait_namespace_empty():
    until = time.monotonic() + 3
    while True:
        if not other_pids():
            return
        if time.monotonic() >= until:
            raise Teardown("daemon_namespace_not_empty")
        time.sleep(0.02)


def assert_no_workers(daemon_pid):
    until = time.monotonic() + 1
    while True:
        if other_pids() == {daemon_pid}:
            return
        if time.monotonic() >= until:
            raise Teardown("worker_processes_after_terminal")
        time.sleep(0.02)
