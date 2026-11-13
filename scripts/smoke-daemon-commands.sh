#!/usr/bin/env bash
# Uses only a disposable workspace and daemon; no herdr server is needed.
set -euo pipefail
exec python3 - <<'PY'
import datetime
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time


class SmokeFailure(Exception):
    pass


def require(condition, message):
    if not condition:
        raise SmokeFailure(message)


def interrupted(signum, _frame):
    raise SmokeFailure(f"interrupted or exceeded 50 s deadline (signal {signum})")


# Resolve before changing cwd or HOME, including relative CORRAL_BIN paths.
requested = os.environ.get("CORRAL_BIN", "corral")
resolved = shutil.which(requested)
if resolved is None:
    sys.exit(f"smoke: failed: corral binary not found: {requested}")
binary = str(Path(resolved).resolve())
private = tempfile.TemporaryDirectory(prefix="corral-cmd-smoke-")
base = Path(private.name)
workspace = base / "ws"
daemon = None
daemon_log = None
stage = "setup"
deadline = time.monotonic() + 50
signal.signal(signal.SIGALRM, interrupted)
signal.signal(signal.SIGTERM, interrupted)
signal.signal(signal.SIGINT, interrupted)
signal.alarm(50)

try:
    env = dict(os.environ)
    # Do not inherit agent identity, root/socket overrides, or owner's Git config.
    for key in list(env):
        if key.startswith(("CORRAL_", "HERDR_", "GIT_")) or key in (
            "AI_AGENT", "PI_CODING_AGENT",
        ):
            del env[key]
    for key, directory in {
        "HOME": "home", "XDG_RUNTIME_DIR": "run", "XDG_CONFIG_HOME": "cfg",
        "XDG_STATE_HOME": "state", "XDG_DATA_HOME": "data",
        "XDG_CACHE_HOME": "cache",
    }.items():
        path = base / directory
        path.mkdir(mode=0o700)
        env[key] = str(path)
    env.update({
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_AUTHOR_NAME": "Corral smoke", "GIT_AUTHOR_EMAIL": "smoke@example.invalid",
        "GIT_COMMITTER_NAME": "Corral smoke",
        "GIT_COMMITTER_EMAIL": "smoke@example.invalid",
        "CORRAL_DAEMON_COMMANDS": "1",
    })
    workspace.mkdir()
    os.chdir(workspace)

    def budget(maximum=8):
        remaining = deadline - time.monotonic()
        require(remaining > 0, "50 s deadline exceeded")
        return min(maximum, remaining)

    def run(argv, overrides=None, allowed=(0,), maximum=8):
        child_env = env if overrides is None else dict(env, **overrides)
        with subprocess.Popen(argv, env=child_env, stdin=subprocess.DEVNULL,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              text=True) as child:
            try:
                stdout, stderr = child.communicate(timeout=budget(maximum))
            except BaseException:
                child.kill()
                child.communicate()
                raise
            require(child.returncode in allowed,
                    f"{argv!r} exited {child.returncode}: {stderr.strip()} {stdout.strip()}")
            return stdout, child.pid, child.returncode

    def corral(*args, overrides=None, allowed=(0,), maximum=8):
        return run([binary, "--root", str(workspace), *args], overrides, allowed, maximum)

    run(["git", "init", "-q"])
    corral("init")
    task = corral("new", "smoke task")[0].strip()
    require(bool(task) and "\n" not in task, f"unexpected new-task id: {task!r}")
    socket_path = Path(corral("daemon", "socket")[0].strip())
    require(socket_path.is_relative_to(base), f"socket outside private directory: {socket_path}")
    runtime = socket_path.parent
    runtime.mkdir(parents=True, exist_ok=True)
    (runtime / "timing-on").touch()
    timing = runtime / "timing.jsonl"
    daemon_log = (base / "daemon.log").open("a+")

    def start_daemon():
        global daemon
        daemon = subprocess.Popen(
            [binary, "--root", str(workspace), "daemon", "run"], env=env,
            stdin=subprocess.DEVNULL, stdout=daemon_log, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        ready_by = min(deadline, time.monotonic() + 10)
        while time.monotonic() < ready_by:
            require(daemon.poll() is None, "private daemon exited during startup")
            _, _, code = corral("daemon", "status", allowed=(0, 1, 2),
                                maximum=max(0.01, ready_by - time.monotonic()))
            if code in (0, 1):
                return
            time.sleep(0.05)
        raise SmokeFailure("private daemon did not become ready within 10 s")

    def check_span(pid, expected):
        require(timing.exists(), "timing.jsonl was not written")
        records = [json.loads(line) for line in timing.read_text().splitlines()]
        records = [r for r in records if r["kind"] == "process" and r["verb"] == "comment"]
        require(bool(records), "missing comment process timing")
        record = records[-1]
        require(record["pid"] == pid, f"comment timing does not belong to pid {pid}")
        spans = record["spans"]
        require(spans.get(expected, {}).get("n", 0) > 0, f"missing timing span {expected}")
        if expected == "command.daemon":
            require("command.local" not in spans, "unexpected timing span command.local")

    def comments():
        return json.loads(corral("show", task, "--json")[0])["comments"]

    start_daemon()
    stage = "1 routed comment"
    _, pid, _ = corral("comment", task, "one")
    check_span(pid, "command.daemon")
    require(len(comments()) == 1, "routed comment did not leave exactly one effect")
    print("check 1: routed comment uses command.daemon", flush=True)

    # frame.rs: u32le length INCLUDES the five-byte kind/stream header.
    # JSON-RPC is CONTROL (kind 0), stream 0; outputs are DATA (kind 1).
    def receive_exact(connection, count):
        data = bytearray()
        while len(data) < count:
            connection.settimeout(budget(5))
            chunk = connection.recv(count - len(data))
            require(bool(chunk), "raw client received truncated frame")
            data.extend(chunk)
        return bytes(data)

    def receive_frame(connection):
        size = struct.unpack("<I", receive_exact(connection, 4))[0]
        require(5 <= size <= 5 + 60 * 1024, f"invalid raw frame size {size}")
        kind, stream = struct.unpack("<BI", receive_exact(connection, 5))
        require(kind in (0, 1, 2), f"invalid raw frame kind {kind}")
        payload = receive_exact(connection, size - 5)
        require(kind != 1 or len(payload) <= 32 * 1024, "oversized DATA frame")
        return kind, stream, payload

    request_id = 0

    def rpc(connection, method, params):
        global request_id
        request_id += 1
        payload = json.dumps({"jsonrpc": "2.0", "id": request_id,
                              "method": method, "params": params}).encode()
        require(len(payload) <= 60 * 1024, "raw request exceeds CONTROL limit")
        connection.settimeout(budget(5))
        connection.sendall(struct.pack("<IBI", len(payload) + 5, 0, 0) + payload)
        kind, stream, payload = receive_frame(connection)
        require((kind, stream) == (0, 0), "expected CONTROL response on stream 0")
        response = json.loads(payload)
        require(response.get("jsonrpc") == "2.0" and response.get("id") == request_id,
                f"invalid JSON-RPC response: {response!r}")
        require("error" not in response, f"{method} refused: {response.get('error')!r}")
        result = response["result"]
        output = result.get("output")
        if output is not None:
            body = bytearray()
            for _ in range(output["frames"]):
                kind, stream, payload = receive_frame(connection)
                require((kind, stream) == (1, output["stream"]), "invalid output DATA frame")
                body.extend(payload)
            require(len(body) == output["bytes"], "wrong command output length")
            decoded = json.loads(body)
            require(isinstance(decoded.get("stdout"), str)
                    and isinstance(decoded.get("stderr"), str), "invalid CmdOutput")
        return result

    build = corral("build-identity")[0].strip()
    version = run([binary, "--version"])[0].strip().split()[-1]
    now = datetime.datetime.now(datetime.timezone.utc)
    submit = {
        "cmd_id": "smoke-1",
        "caller": {
            "principal": {"kind": "unverified"}, "client": "cli", "actor": "owner",
            "via": "cli", "assistant": None, "planning_safe": False,
            "cwd": str(workspace), "env": {},
        },
        "client_build": {"exe": binary, "build": build}, "expect": None,
        "issued_at": now.isoformat(),
        "expires_at": (now + datetime.timedelta(seconds=120)).isoformat(),
        "command": {"family": "task", "args": {
            "verb": "comment", "id": task, "text": "raw-note", "author": "owner", "send": False,
        }},
    }
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(budget(5))
        connection.connect(str(socket_path))
        welcome = rpc(connection, "hello", {
            "proto": {"major": 1, "minor": 0}, "caps": ["board.v1", "read.v1", "cmd.v1"],
            "client": {"kind": "cli", "version": version},
        })
        require(welcome["proto"]["major"] == 1 and "cmd.v1" in welcome["caps"],
                "daemon did not negotiate cmd.v1")
        stage = "2 replay"
        first = rpc(connection, "cmd.submit", submit)
        require(first["state"] == "done" and first["exit"] == 0 and not first["replayed"],
                f"first submit failed: {first!r}")
        second = rpc(connection, "cmd.submit", submit)
        require(second["state"] == "done" and second["exit"] == 0 and second["replayed"] is True,
                f"second submit was not a successful replay: {second!r}")
        before = comments()
        require(len(before) == 2 and sum(c["text"] == "raw-note" for c in before) == 1,
                "replay did not leave exactly one raw-note effect")
        print("check 2: replayed:true with one effect", flush=True)

        stage = "3 stale expect"
        stale = dict(submit, cmd_id="smoke-stale", expect={
            "kind": "task_revision", "task": task, "revision": "0",
        })
        stale["command"] = {"family": "task", "args": dict(
            submit["command"]["args"], text="must-not-appear")}
        conflict = rpc(connection, "cmd.submit", stale)
        require(conflict["state"] == "conflict" and conflict["exit"] == 1,
                f"stale expect did not conflict: {conflict!r}")
        require(comments() == before, "stale expect changed comments")
        print("check 3: stale expect conflicts without effect", flush=True)

    stage = "4 PM attribution"
    _, pid, _ = corral("comment", task, "pm-note", overrides={"CORRAL_PM": "1"})
    check_span(pid, "command.daemon")
    entries = [json.loads(line) for line in corral("log", task, "--json")[0].splitlines()]
    require(bool(entries), "empty task log")
    last = entries[-1]
    require(last["kind"] == "commented" and last["summary"] == "pm-note"
            and last["actor"] == "pm" and last["via"] == "pm",
            f"PM attribution was not preserved: {last!r}")
    print("check 4: PM actor/via preserved", flush=True)

    stage = "5 stopped daemon fallback"
    corral("daemon", "stop")
    daemon.wait(timeout=budget(4))
    require(daemon.returncode == 0, f"private daemon exited {daemon.returncode}")
    daemon = None
    down_by = min(deadline, time.monotonic() + 4)
    while time.monotonic() < down_by:
        _, _, code = corral("daemon", "status", allowed=(0, 1, 2),
                            maximum=max(0.01, down_by - time.monotonic()))
        if code == 2:
            break
        time.sleep(0.05)
    else:
        raise SmokeFailure("private daemon did not report down within 4 s")
    _, pid, _ = corral("comment", task, "offline")
    check_span(pid, "command.local")
    require(not (runtime / "command-fallback.json").exists(), "absent daemon wrote fallback marker")
    require(any(c["text"] == "offline" for c in comments()), "local fallback lost its comment")
    print("check 5: absent daemon uses command.local without fallback marker", flush=True)

    stage = "6 local kill switch"
    start_daemon()
    _, pid, _ = corral("comment", task, "forced", overrides={"CORRAL_DAEMON_COMMANDS": "0"})
    check_span(pid, "command.local")
    require(any(c["text"] == "forced" for c in comments()), "forced local command lost its comment")
    print("check 6: CORRAL_DAEMON_COMMANDS=0 forces command.local", flush=True)
    print("smoke: ok", flush=True)
except Exception as error:
    print(f"smoke: failed: {stage}: {error}", file=sys.stderr)
    if daemon_log is not None:
        daemon_log.flush()
        daemon_log.seek(0)
        print("private daemon log:\n" + daemon_log.read(), file=sys.stderr)
    sys.exit(1)
finally:
    signal.alarm(0)
    # Stop only our child daemon, then kill its private process group if necessary.
    # Cleanup has its own small bound even if the main deadline expired.
    if daemon is not None and daemon.poll() is None:
        try:
            subprocess.run([binary, "--root", str(workspace), "daemon", "stop"],
                           env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, timeout=2, check=False)
            daemon.wait(timeout=1)
        except (subprocess.TimeoutExpired, OSError):
            try:
                os.killpg(daemon.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            daemon.wait(timeout=1)
    if daemon_log is not None:
        daemon_log.close()
    os.chdir("/")
    private.cleanup()
PY
