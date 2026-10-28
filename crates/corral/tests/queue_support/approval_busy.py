"""Trace only disposable sync processes; preserve both failed and successful approvals."""

import fcntl
import json
import os
import shutil
import signal
import subprocess
import sys
from pathlib import Path


def lock_attempt(process: subprocess.Popen[bytes], marker: bytes) -> bytes:
    """Subscribe to the exact blocking syscall, not a timing-dependent delay."""
    assert process.stderr is not None
    seen = b""
    # A blocking pipe read waits for the syscall (or EOF), regardless of host load.
    # A per-phase deadline would reject correctly progressing slow debug builds.
    while marker not in seen:
        chunk = os.read(process.stderr.fileno(), 4096)
        assert chunk, seen.decode()
        seen += chunk
    return seen


def main() -> None:
    """Separate actual guard ownership from owner-ready sync's rebuildable work."""
    tracer = shutil.which("strace")
    assert tracer is not None, "Linux approval regression requires strace"
    binary, directory, task, peer = sys.argv[1:]
    root = Path(directory)
    environment = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CORRAL_", "HERDR_"))
    }
    environment["CORRAL_HERDR_SESSION"] = "corral-test"
    command = [binary, "--root", directory]

    def cli(*args: str) -> subprocess.CompletedProcess[bytes]:
        result = subprocess.run(
            [*command, *args],
            env=environment,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            check=False,
        )
        print(json.dumps({
            "argv": args, "exit": result.returncode,
            "stdout": result.stdout.decode(), "stderr": result.stderr.decode(),
        }), flush=True)
        return result

    approve = ["approve", task, "--merge", "rebase", "--keep-worktree", "--keep-branch"]
    landing = root / "app" / ".git" / "corral-landing.lock"
    with landing.open("a") as held:
        fcntl.flock(held, fcntl.LOCK_EX)
        approval = subprocess.Popen(
            [tracer, "-yy", "-e", "trace=flock,fcntl", *command, *approve],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        try:
            trace = lock_attempt(approval, b"/corral-landing.lock>, LOCK_EX")
            print("ACTIVE_APPROVAL_AT_LANDING_LOCK\n" + trace.decode(), flush=True)
            concurrent = cli(*approve)
            assert concurrent.returncode == 1
            assert b"approval already in progress" in concurrent.stderr
            # Fail on a real landing precondition, not an unrelated peer's review.
            # Change only the disposable main checkout, leaving candidate receipts fresh.
            tracked = root / "app" / "initial.txt"
            clean = tracked.read_bytes()
            tracked.write_bytes(clean + b"approval fixture dirty checkout\n")
            fcntl.flock(held, fcntl.LOCK_UN)
            stdout, stderr = approval.communicate()
            print("DIRTY_CHECKOUT_REFUSAL\n" + stdout.decode() + stderr.decode(), flush=True)
            assert approval.returncode == 1
            assert b"has uncommitted changes" in stderr
            # A completed refusal releases the guard; the real dirty checkout still blocks landing.
            pending = cli(*approve)
            assert pending.returncode == 1
            assert b"has uncommitted changes" in pending.stderr
            tracked.write_bytes(clean)
        finally:
            fcntl.flock(held, fcntl.LOCK_UN)
            if approval.poll() is None:
                os.killpg(approval.pid, signal.SIGKILL)
                approval.wait()
    assert cli("verdict", peer, "approve", "--notes", "Peer fixture reviewed.").returncode == 0
    assert cli("sync").returncode == 0

    search_lock = root / ".corral" / "state" / "content-search.lock"
    with search_lock.open("a") as held:
        fcntl.flock(held, fcntl.LOCK_EX)
        sync = subprocess.Popen(
            [tracer, "-yy", "-e", "trace=flock,fcntl", *command, "sync"],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        try:
            trace = lock_attempt(sync, b"/content-search.lock>, LOCK_EX")
            print("SYNC_AT_INDEX_LOCK\n" + trace.decode(), flush=True)
            # This is deliberately a real CLI attempt while the traced sync remains blocked.
            first = cli(*approve)
            if first.returncode != 0:
                second = cli(*approve)
                assert second.returncode == 1
                assert b"approval already in progress" in second.stderr
            fcntl.flock(held, fcntl.LOCK_UN)
            stdout, stderr = sync.communicate()
            print("SYNC_FINISHED\n" + stdout.decode() + stderr.decode(), flush=True)
            assert sync.returncode == 0
            if first.returncode != 0:
                assert cli(*approve).returncode == 0
            # Completed approval cannot authorize another merge or acceptance.
            assert cli(*approve).returncode == 1
            shown = json.loads(cli("show", task, "--json").stdout)
            events = [
                json.loads(line)
                for line in (root / ".corral" / "events.jsonl").read_text().splitlines()
            ]
            approvals = [event for event in events if event["kind"] == "approved"]
            assert shown["status"] == "done"
            assert len(approvals) == 1, approvals
            assert approvals[0]["detail"]["acceptance_mode"] == "owner_override"
            peer_state = json.loads(cli("show", peer, "--json").stdout)
            assert peer_state["status"] == "review", "owner acceptance must still wait"
            assert not any(
                artifact["title"] == "Acceptance record"
                for artifact in shown["artifacts"]
            ), "manual approval must not invent autonomous acceptance evidence"
            print("APPROVAL_RECEIPTS " + json.dumps({
                "run": shown["runs"][-1], "approvals": approvals,
                "first_approval_exit": first.returncode,
            }), flush=True)
            assert first.returncode == 0, (
                "owner-ready sync held approval guard during rebuildable indexing; "
                "repeated native busy refusals reproduced"
            )
        finally:
            fcntl.flock(held, fcntl.LOCK_UN)
            if sync.poll() is None:
                os.killpg(sync.pid, signal.SIGKILL)
                sync.wait()
            print("TRACE_CLEANUP: disposable trace process reaped; fixture locks released", flush=True)


if __name__ == "__main__":
    main()
