#!/usr/bin/env python3
"""Opt-in offline C3 QA using real Corral component binaries and a fake Lite peer."""
import argparse
import ctypes
import os
import platform
import re
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from protocol import OPT_IN, SCENARIOS, Failure, load, need, wire
from staging import BWRAP, CLEAN_ENV, ENTRYPOINTS, LOADER, REPO, checked_program, stage


class IsolationUnavailable(Failure):
    pass


class TeardownUnconfirmed(Failure):
    pass


def bwrap_info(fd, process):
    until = time.monotonic() + 6
    raw = bytearray()
    while True:
        if not select.select([fd], [], [], max(0, until - time.monotonic()))[0]:
            raise IsolationUnavailable("outer_bwrap_info_timeout")
        chunk = os.read(fd, 4097 - len(raw))
        if not chunk:
            break
        raw.extend(chunk)
        need(len(raw) <= 4096, "outer_bwrap_info_size")
    if not raw:
        raise IsolationUnavailable("outer_bwrap_namespace_unavailable")
    info = load(bytes(raw))
    for ns in ("pid", "mnt", "net"):
        if info.get(f"{ns}-namespace") in (None, os.stat(f"/proc/self/ns/{ns}").st_ino):
            raise IsolationUnavailable("outer_bwrap_namespace_not_private")
    pid = info.get("child-pid")
    need(type(pid) is int and pid > 1 and pid != process.pid, "outer_child_identity")
    try:
        return os.pidfd_open(pid)
    except ProcessLookupError:
        # Fast startup failure can destroy the entire private PID namespace
        # before the reviewer acquires the pidfd; no work can survive its init.
        if process.poll() is None:
            raise TeardownUnconfirmed("outer_child_identity_lost") from None
        return None


def stop_outer(process, pidfd):
    try:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
        if pidfd is not None and not select.select([pidfd], [], [], 3)[0]:
            raise TeardownUnconfirmed("outer_namespace_still_alive")
    except subprocess.TimeoutExpired:
        raise TeardownUnconfirmed("outer_wrapper_still_alive") from None
    finally:
        if pidfd is not None:
            os.close(pidfd)


def run_sandbox(root, lanes):
    fd, writer = os.pipe()
    try:
        process = subprocess.Popen([str(root / "bwrap"), "--unshare-all", "--new-session", "--die-with-parent",
                                    "--clearenv", "--cap-drop", "ALL", "--proc", "/proc", "--dev", "/dev",
                                    "--tmpfs", "/tmp", "--ro-bind", "/usr", "/usr", "--symlink", "usr/lib", "/lib",
                                    "--symlink", "usr/lib64", "/lib64",
                                    "--ro-bind", str(root / "bwrap"), "/usr/bin/bwrap",
                                    "--ro-bind", str(root / "qa"), "/qa",
                                    "--ro-bind", str(root / "corral"), "/fixture/corral",
                                    "--ro-bind", str(root / "meta"), "/fixture/meta",
                                    "--bind", str(root / "state"), "/state",
                                    "--bind", str(root / "scratch"), "/scratch",
                                    "--info-fd", str(writer), "--chdir", "/scratch", "--", "/usr/bin/python3",
                                    "-EsB", "/qa/inside.py", "--inside"],
                                   env=CLEAN_ENV, cwd=root, pass_fds=(writer,), stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    except BaseException:
        os.close(fd)
        raise
    finally:
        os.close(writer)
    pidfd = None
    try:
        pidfd = bwrap_info(fd, process)
        raw, _ = process.communicate(timeout=35 + 50 * lanes)
        need(0 < len(raw) <= 16384, "outer_controller_receipt")
        result = load(raw)
        need(type(result) is dict and result.get("status") in ("pass", "fail") and
             (result["status"] == "pass") == (process.returncode == 0), "outer_controller_status")
        return process.returncode if process.returncode in (0, 69, 70, 74) else 70, result
    finally:
        os.close(fd)
        stop_outer(process, pidfd)
        if process.stdout is not None:
            process.stdout.close()


def sigterm(_sig, _frame):
    raise KeyboardInterrupt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(OPT_IN, action="store_true")
    for name in ENTRYPOINTS:
        parser.add_argument("--" + name, type=Path, help="path to an already built trusted local " + name + " executable")
    parser.add_argument("--revision", help="40 lowercase hex digits naming the local build's Corral baseline")
    parser.add_argument("--packet", type=Path, default=REPO / ".evidence/lite-fixtures/docs/protocol/analyst-february-close-v1.json",
                        help="raw pinned T-2073:3 packet")
    parser.add_argument("--scenario", action="append", choices=SCENARIOS, help="select lanes; default: all")
    opts = parser.parse_args()
    if not opts.enable_public_fixture_e2e:
        print(wire({"status": "disabled", "scope": "corral_component_only", "joined_e1": "not_run"}).decode())
        return 64
    receipt = {"scope": "corral_component_only", "lite": "synthetic_contract_peer", "joined_e1": "not_run",
               "report_import": "not_run", "fresh_compute": "not_run", "status": "fail"}
    root = None
    exit_code = 64
    try:
        need(all(getattr(opts, ep) is not None for ep in ENTRYPOINTS) and opts.revision is not None
             and re.fullmatch(r"[a-f0-9]{40}", opts.revision) and int(opts.revision, 16) != 0,
             "trusted_binary_inputs_and_revision_required")
        selected = opts.scenario or SCENARIOS
        need(len(set(selected)) == len(selected), "duplicate_scenario")
        if platform.system() != "Linux" or platform.machine() != "x86_64" or os.getuid() == 0 or os.geteuid() == 0:
            raise IsolationUnavailable("rootless_linux_x86_64_required")
        if BWRAP.is_symlink() or not BWRAP.is_file() or not Path(LOADER).is_file() or not hasattr(os, "pidfd_open"):
            raise IsolationUnavailable("missing_pinned_runtime")
        checked_program(BWRAP)
        if ctypes.CDLL(None, use_errno=True).syscall(38, 1, 0, 0, 0) != 0:  # PR_SET_NO_NEW_PRIVS
            raise IsolationUnavailable("no_new_privs_unavailable")
        root = Path(tempfile.mkdtemp(prefix="corral-c3-qa-", dir="/tmp"))
        signal.signal(signal.SIGTERM, sigterm)
        receipt["pins"] = stage(root, opts)
        exit_code = 70
        exit_code, observed = run_sandbox(root, len(selected))
        receipt.update(observed)
        receipt["outer_teardown"] = "confirmed"
    except IsolationUnavailable as exc:
        exit_code = 69
        receipt["error"] = str(exc)
    except TeardownUnconfirmed as exc:
        exit_code = 74
        receipt["error"] = str(exc)
    except Failure as exc:
        receipt["error"] = str(exc)
    except (Exception, KeyboardInterrupt) as exc:
        receipt["error"] = type(exc).__name__
    finally:
        if root is not None:
            if exit_code == 74:
                receipt["scratch_quarantined"] = True
            else:
                shutil.rmtree(root)
        receipt["status"] = "pass" if exit_code == 0 else "fail"
        print(wire(receipt).decode())
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
