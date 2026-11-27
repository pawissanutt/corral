#!/usr/bin/env python3
"""Opt-in offline QA using real Corral component binaries and a fake Lite peer."""
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
from contextlib import ExitStack
from pathlib import Path

from protocol import FAULT_SCENARIOS, OPT_IN, SCENARIOS, Failure, load, need, wire
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
    need(type(info) is dict, "outer_bwrap_info_shape")
    pid = info.get("child-pid")
    need(type(pid) is int and pid > 1 and pid != process.pid and process.poll() is None,
         "outer_child_identity")
    pidfd = os.pidfd_open(pid)
    try:
        proc = Path(f"/proc/{pid}")
        status = dict(line.split(":", 1) for line in (proc / "status").read_text().splitlines())
        need(status.get("PPid", "").split() == [str(process.pid)] and
             status.get("NSpid", "").split() == [str(pid), "1"], "outer_child_not_namespace_init")
        for ns in ("pid", "mnt", "net"):
            reported = info.get(f"{ns}-namespace")
            need(type(reported) is int and reported == (proc / "ns" / ns).stat().st_ino and
                 reported != Path(f"/proc/self/ns/{ns}").stat().st_ino, "outer_namespace_not_verified")
        # If the PID were replaced while reading procfs, the pinned task would
        # already be exited. The waitable wrapper was not reaped during verification.
        need(not select.select([pidfd], [], [], 0)[0], "outer_child_exited_during_verification")
        return pidfd
    except BaseException:
        os.close(pidfd)
        raise


def stop_outer(process, pidfd):
    until = time.monotonic() + 5
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=max(0, until - time.monotonic()))
        except subprocess.TimeoutExpired:
            pass
    if (process.poll() is not None and pidfd is not None and
            select.select([pidfd], [], [], max(0, until - time.monotonic()))[0]):
        return
    if pidfd is not None:
        try:
            signal.pidfd_send_signal(pidfd, signal.SIGKILL)
        except OSError:
            pass  # A subsequent exit observation, not the signal result, is the proof.
    if process.poll() is None:
        process.kill()
    until = time.monotonic() + 3
    try:
        process.wait(timeout=max(0, until - time.monotonic()))
    except subprocess.TimeoutExpired:
        raise TeardownUnconfirmed("outer_wrapper_still_alive") from None
    if pidfd is None:
        raise TeardownUnconfirmed("outer_child_identity_unconfirmed")
    if not select.select([pidfd], [], [], max(0, until - time.monotonic()))[0]:
        raise TeardownUnconfirmed("outer_namespace_still_alive")


def run_sandbox(root, lanes, receipt):
    with ExitStack() as host, ExitStack() as inherited:
        fd, writer = os.pipe()
        host.callback(os.close, fd)
        inherited.callback(os.close, writer)
        gate, release = os.pipe()
        inherited.callback(os.close, gate)
        host.callback(os.close, release)
        # Set the fail-closed default before Popen, including interruptions
        # during spawn or cleanup. Only both retained exit proofs can clear it.
        receipt["outer_teardown"] = "unconfirmed"
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
                                        "--info-fd", str(writer), "--block-fd", str(gate), "--sync-fd", str(release),
                                        "--chdir", "/scratch", "--", "/usr/bin/python3", "-EsB", "/qa/inside.py", "--inside"],
                                       env=CLEAN_ENV, cwd=root, pass_fds=(writer, gate, release), stdin=subprocess.DEVNULL,
                                       stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        except OSError:
            # Popen itself reported failure before wrapper exec.
            del receipt["outer_teardown"]
            raise
        pidfd = None
        try:
            inherited.close()
            pidfd = bwrap_info(fd, process)
            need(process.poll() is None, "outer_wrapper_exited_before_release")
            # bwrap treats block-fd EOF as release. The sync-fd copy kept by
            # namespace init prevents EOF even if this QA runner disappears;
            # bwrap closes that copy in the actual controller before exec.
            need(os.write(release, b"x") == 1, "outer_gate_release")
            raw, _ = process.communicate(timeout=35 + 50 * lanes)
            need(0 < len(raw) <= 16384, "outer_controller_receipt")
            result = load(raw)
            need(type(result) is dict and result.get("status") in ("pass", "fail") and
                 (result["status"] == "pass") == (process.returncode == 0), "outer_controller_status")
            return process.returncode if process.returncode in (0, 69, 70, 74) else 70, result
        finally:
            try:
                stop_outer(process, pidfd)
                receipt["outer_teardown"] = "confirmed"
            finally:
                if pidfd is not None:
                    os.close(pidfd)
                if process.stdout is not None:
                    process.stdout.close()


def sigterm(_sig, _frame):
    raise KeyboardInterrupt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(OPT_IN, action="store_true")
    for name in ENTRYPOINTS:
        parser.add_argument("--" + name, type=Path, help="path to an already built trusted local " + name + " executable")
    parser.add_argument("--bundle", type=Path, help="C4 fixture archive instead of three trusted local executables")
    parser.add_argument("--archive-sha256", help="independent C4 archive pin")
    parser.add_argument("--manifest-sha256", help="independent C4 raw manifest pin")
    parser.add_argument("--revision", help="40 lowercase hex digits naming the Corral build revision")
    parser.add_argument("--packet", type=Path, default=REPO / ".evidence/lite-fixtures/docs/protocol/analyst-february-close-v1.json",
                        help="raw pinned T-2073:3 packet")
    parser.add_argument("--with-faults", action="store_true", help="run only the separately pinned Corral fault variant")
    parser.add_argument("--scenario", action="append", choices=(*SCENARIOS, *FAULT_SCENARIOS),
                        help="select lanes within the chosen variant; default: all in that variant")
    opts = parser.parse_args()
    if not opts.enable_public_fixture_e2e:
        print(wire({"status": "disabled", "scope": "corral_component_only", "joined_e1": "not_run"}).decode())
        return 64
    receipt = {"scope": "corral_component_only", "lite": "synthetic_contract_peer", "joined_e1": "not_run",
               "report_import": "not_run", "fresh_compute": "not_run", "status": "fail"}
    if opts.with_faults:
        receipt["joined_fault_matrix"] = "not_run"
    root = None
    exit_code = 64
    try:
        local = [getattr(opts, ep) for ep in ENTRYPOINTS]
        need(opts.revision is not None and re.fullmatch(r"[a-f0-9]{40}", opts.revision) and int(opts.revision, 16) != 0,
             "exact_corral_revision_required")
        need((opts.bundle is not None and not any(local) and opts.archive_sha256 and opts.manifest_sha256)
             or (opts.bundle is None and all(local) and not opts.archive_sha256 and not opts.manifest_sha256),
             "select_pinned_archive_or_three_trusted_local_binaries")
        need(not opts.with_faults or opts.bundle is not None, "fault_bundle_required")
        allowed = FAULT_SCENARIOS if opts.with_faults else SCENARIOS
        selected = opts.scenario or allowed
        need(set(selected) <= set(allowed), "scenario_variant_mismatch")
        need(len(set(selected)) == len(selected), "duplicate_scenario")
        opts.scenario = selected
        if platform.system() != "Linux" or platform.machine() != "x86_64" or os.getuid() == 0 or os.geteuid() == 0:
            raise IsolationUnavailable("rootless_linux_x86_64_required")
        if (BWRAP.is_symlink() or not BWRAP.is_file() or not Path(LOADER).is_file() or
                not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal")):
            raise IsolationUnavailable("missing_pinned_runtime")
        checked_program(BWRAP)
        if ctypes.CDLL(None, use_errno=True).syscall(38, 1, 0, 0, 0) != 0:  # PR_SET_NO_NEW_PRIVS
            raise IsolationUnavailable("no_new_privs_unavailable")
        root = Path(tempfile.mkdtemp(prefix="corral-c3-qa-", dir="/tmp"))
        signal.signal(signal.SIGTERM, sigterm)
        receipt["pins"] = stage(root, opts)
        exit_code = 70
        exit_code, observed = run_sandbox(root, len(selected), receipt)
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
        if receipt.get("outer_teardown") == "unconfirmed":
            exit_code = 74
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
