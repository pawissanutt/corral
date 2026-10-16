"""Run sync after the creator's first publication, before any later update."""

import json
import os
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def stopped_trace(process: subprocess.Popen[bytes]) -> bytes:
    """Consume the buffered syscall signal event with a bounded deadline."""
    assert process.stderr is not None
    seen = b""
    deadline = time.monotonic() + 10
    with selectors.DefaultSelector() as ready:
        ready.register(process.stderr, selectors.EVENT_READ)
        while b"--- SIGSTOP" not in seen:
            remaining = deadline - time.monotonic()
            assert remaining > 0 and ready.select(remaining), seen.decode()
            chunk = os.read(process.stderr.fileno(), 4096)
            assert chunk, seen.decode()
            seen += chunk
    return seen


def main() -> None:
    """Interleave actual new/sync commands using an unlocked publication barrier."""
    tracer = shutil.which("strace")
    assert tracer is not None, "the Linux filing race regression requires strace"
    idea = "--idea" in sys.argv[2:]
    environment = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith(("CORRAL_", "HERDR_"))
    }
    with tempfile.TemporaryDirectory(prefix="corral-filing-race-") as directory:
        root = Path(directory)
        for name in [
            "HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
            "XDG_STATE_HOME", "XDG_CACHE_HOME",
        ]:
            environment[name] = str(root / name)
            (root / name).mkdir()
        environment.update({
            "CORRAL_ROOT": directory,
            "CORRAL_HERDR_SESSION": "corral-test",
            "HERDR_BIN_PATH": str(root / "herdr-fixture"),
            "GIT_AUTHOR_NAME": "filing fixture",
            "GIT_COMMITTER_NAME": "filing fixture",
            "GIT_AUTHOR_EMAIL": "filing@example.invalid",
            "GIT_COMMITTER_EMAIL": "filing@example.invalid",
        })
        (root / "herdr-fixture").symlink_to(Path(__file__).with_name("herdr.py"))
        command = [sys.argv[1], "--root", directory]

        def run(args: list[str]) -> subprocess.CompletedProcess[str]:
            return subprocess.run(
                args, cwd=root, env=environment, stdin=subprocess.DEVNULL,
                capture_output=True, text=True, check=True, timeout=20,
            )

        def corral(*args: str) -> str:
            return run([*command, *args]).stdout

        (root / "app").mkdir()
        run(["git", "init", "-q", "-b", "main", str(root / "app")])
        run(["git", "-C", str(root / "app"), "-c", "commit.gpgsign=false",
             "commit", "--allow-empty", "-qm", "fixture"])
        corral("init")
        for key, value in [
            ("defaults.profile", "generic"),
            ("profiles.filed.harness", "generic"),
            ("review.autopilot", "false"),
            ("dispatch.auto_backlog", "true"),
            ("dispatch.promote_ideas", "auto"),
            ("limits.running", "3"),
            ("limits.review", "2"),
            ("resources.max_load_per_core", "1000000"),
            ("resources.min_mem_available", "1B"),
            ("resources.min_disk_free", "1B"),
        ]:
            corral("config", "set", key, value)
        dependency = corral("new", "Unfinished dependency", "--idea", "-p", "app").strip()
        labels = ["owner-only", "filed"] if idea else ["filed"]
        size = "l" if idea else "s"
        new_args = [
            "new", "Concurrent filing", "-p", "app", "--depends", dependency,
            "--profile", "filed", "--priority", "p0", "--kind", "backend",
            "--size", size, "--label", ",".join(labels),
            "--body", "## Acceptance\nWait until the dependency completes.",
        ]
        if idea:
            new_args.extend(["--idea", "--design"])

        # Learn the first board-lock close, not a second update that a correct
        # implementation no longer needs. StoreLock explicitly unlocks before close.
        trace_args = [tracer, "-yy", "-e", "trace=close,flock"]
        warm = run([*trace_args, *command, *new_args])
        closes = [line for line in warm.stderr.splitlines() if line.startswith("close(")]
        ordinal = next(
            index + 1 for index, line in enumerate(closes)
            if "/lock/dispatch.lock>" in line
        )
        corral("move", warm.stdout.strip(), "dropped", "--owner-ok")

        with subprocess.Popen(
            [*trace_args, "-e", f"inject=close:signal=SIGSTOP:when={ordinal}",
             *command, *new_args],
            cwd=root, env=environment, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True,
        ) as creator:
            try:
                trace = stopped_trace(creator)
                close = [line for line in trace.splitlines() if line.startswith(b"close(")][-1]
                assert b"/lock/dispatch.lock>" in close, trace.decode()
                assert any(
                    b"/lock/dispatch.lock>, LOCK_UN)" in line and b"= 0" in line
                    for line in trace.splitlines()
                ), trace.decode()
                before = next(
                    task for task in json.loads(corral("list", "--json"))
                    if task["title"] == "Concurrent filing" and task["status"] != "dropped"
                )
                sync = corral("sync")
                during = json.loads(corral("show", before["id"], "--json"))
                os.killpg(creator.pid, signal.SIGCONT)
                stdout, stderr = creator.communicate(timeout=20)
                assert creator.returncode == 0, stderr.decode()
                assert stdout.decode().strip() == before["id"]
                after = json.loads(corral("show", before["id"], "--json"))
            finally:
                if creator.poll() is None:
                    os.killpg(creator.pid, signal.SIGKILL)
                    creator.communicate(timeout=10)

        print(json.dumps({"before": before, "sync": sync, "during": during, "after": after}))
        # Neither admission nor routing may act on the creator's intermediate defaults.
        for snapshot in [before, during, after]:
            assert snapshot["status"] == ("idea" if idea else "backlog"), snapshot
            assert not snapshot.get("runs"), snapshot
            assert not snapshot.get("plan_runs"), snapshot
            assert snapshot.get("launch") is None, snapshot
        assert json.loads((root / "protocol.json").read_text())["creates"] == 0
        expected = {
            "depends_on": [dependency], "profile": "filed", "priority": "P0",
            "kind": "backend", "size": size, "design": idea, "labels": labels,
        }
        for key, value in expected.items():
            assert before.get(key, False if key == "design" else None) == value, (
                key, before, expected
            )
        assert json.loads(corral("show", dependency, "--json"))["status"] == "idea"


if __name__ == "__main__":
    main()
