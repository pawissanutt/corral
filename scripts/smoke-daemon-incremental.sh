#!/usr/bin/env bash
# Run inside: (source scripts/test-herdr.sh; corral_test_herdr_start; this-script)
# Never starts/stops herdr. Only its own workspace and daemon are removed.
set -euo pipefail
exec python3 - <<'PY'
import datetime
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import sqlite3
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
    raise SmokeFailure(f"interrupted or exceeded 82 s deadline (signal {signum})")


requested = os.environ.get("CORRAL_BIN", "corral")
resolved = shutil.which(requested)
if resolved is None:
    sys.exit(f"smoke: failed: binary not found: {requested}")
binary = str(Path(resolved).resolve())
private = tempfile.TemporaryDirectory(prefix="corral-inc-smoke-")
base = Path(private.name)
workspace = base / "fixture"
daemon = None
log = None
connection = None
stage = "setup"
deadline = time.monotonic() + 82
for sig in (signal.SIGALRM, signal.SIGINT, signal.SIGTERM):
    signal.signal(sig, interrupted)
signal.alarm(82)

try:
    # Reuse ONLY the helper's private HOME/config/data paths, so clients reach its
    # plugin-free server. A new HOME would silently select a different server.
    home = Path(os.environ.get("HOME", "")).resolve()
    helper = home.parent
    require(home.name == "home" and helper.parent == Path("/tmp")
            and helper.name.startswith("corral-herdr-") and helper.is_dir()
            and os.environ.get("CORRAL_HERDR_SESSION") == "corral-test",
            "run inside the scripts/test-herdr.sh private herdr subshell")
    # The helper exports these paths/session, but its root and PID are normally
    # shell-local. An explicitly exported PID is an additional liveness check.
    helper_pid = os.environ.get("CORRAL_TEST_HERDR_PID")
    if helper_pid:
        os.kill(int(helper_pid), 0)
    for key, directory in {
        "XDG_CONFIG_HOME": "config", "XDG_DATA_HOME": "data",
        "XDG_STATE_HOME": "state", "XDG_CACHE_HOME": "cache",
    }.items():
        value = os.environ.get(key, "")
        require(bool(value) and Path(value).resolve() == helper / directory,
                f"{key} does not match the private herdr root")
    env = dict(os.environ)
    for key in list(env):
        if ((key.startswith("CORRAL_") and not key.startswith("CORRAL_TEST_"))
                or key.startswith(("HERDR_", "GIT_"))
                or key in ("AI_AGENT", "PI_CODING_AGENT")):
            del env[key]
    (base / "run").mkdir(mode=0o700)
    env.update({
        "XDG_RUNTIME_DIR": str(base / "run"), "CORRAL_HERDR_SESSION": "corral-test",
        "CORRAL_DAEMON_COMMANDS": "1", "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_AUTHOR_NAME": "Corral smoke", "GIT_AUTHOR_EMAIL": "smoke@example.invalid",
        "GIT_COMMITTER_NAME": "Corral smoke", "GIT_COMMITTER_EMAIL": "smoke@example.invalid",
    })
    workspace.mkdir()
    os.chdir(workspace)

    def budget(maximum=6):
        remaining = deadline - time.monotonic()
        require(remaining > 0, "82 s deadline exceeded")
        return min(maximum, remaining)

    def run(argv, maximum=6, allowed=(0,)):
        with subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as child:
            try:
                stdout, stderr = child.communicate(timeout=budget(maximum))
            except BaseException:
                child.kill()
                child.communicate()
                raise
            require(child.returncode in allowed,
                    f"{argv!r} exited {child.returncode}: {stderr.strip()} {stdout.strip()}")
            return stdout

    def corral(*args):
        return run([binary, "--root", str(workspace), *args])

    def git(*args):
        return run(["git", *args])

    git("init", "-q", "--initial-branch=main")
    (workspace / "fixture.txt").write_text("initial\n")
    git("add", "fixture.txt")
    git("commit", "-q", "-m", "initial fixture")
    corral("init")
    config = workspace / ".corral/corral.toml"
    database = workspace / ".corral/db/corral.db"
    socket_path = Path(corral("daemon", "socket").strip())
    require(socket_path.is_relative_to(base), f"nonprivate daemon socket: {socket_path}")
    runtime = socket_path.parent
    runtime.mkdir(parents=True, exist_ok=True)
    (runtime / "timing-on").touch()
    timing = runtime / "timing.jsonl"
    log = (base / "daemon.log").open("a+")
    daemon = subprocess.Popen([binary, "--root", str(workspace), "daemon", "run"],
                              env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                              start_new_session=True)

    def records():
        if not timing.exists():
            return []
        # A concurrent append may have an incomplete last line; use complete lines only.
        data = timing.read_bytes()
        lines = data.split(b"\n")[:-1]
        return [json.loads(line) for line in lines if line]

    def passes():
        return [r for r in records() if r["kind"] == "pass" and r["pid"] == daemon.pid]

    def n(record, span):
        return record["spans"].get(span, {}).get("n", 0)

    def wait_for(predicate, seconds, description):
        until = min(deadline, time.monotonic() + seconds)
        while time.monotonic() < until:
            require(daemon.poll() is None, f"daemon exited {daemon.returncode}")
            value = predicate()
            if value:
                return value
            time.sleep(0.05)
        raise SmokeFailure(f"timed out: {description}")

    wait_for(lambda: socket_path.exists() and passes(), 10, "initial daemon pass")
    require(n(passes()[0], "sync.full.start") == 1, "first pass was not full/start")

    # Read-only SQLite snapshots: raw sequence is meta.change_seq; the application
    # sequence is max seq having any scope OTHER THAN Record{area: leases or
    # app-commands}. This is exactly FeedDelta/reader::unchanged's exclusion, not
    # a byte/mtime comparison. No database writes are issued by this client.
    def feed():
        with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=1) as db:
            db.execute("PRAGMA query_only=ON")
            db.execute("BEGIN")
            raw = int(db.execute("SELECT value FROM meta WHERE key='change_seq'").fetchone()[0])
            scopes = db.execute("SELECT seq, key FROM change_scopes ORDER BY seq").fetchall()
            application = 0
            for seq, encoded in scopes:
                scope = json.loads(encoded)
                record = scope.get("Record")
                if record is None or record["area"] not in ("leases", "app-commands"):
                    application = max(application, seq)
            return raw, application

    def exact(connection, length):
        result = bytearray()
        while len(result) < length:
            connection.settimeout(budget())
            chunk = connection.recv(length - len(result))
            require(bool(chunk), "truncated daemon frame")
            result.extend(chunk)
        return bytes(result)

    def frame():
        length = struct.unpack("<I", exact(connection, 4))[0]
        require(5 <= length <= 5 + 60 * 1024, f"invalid frame length {length}")
        kind, stream = struct.unpack("<BI", exact(connection, 5))
        require(kind in (0, 1, 2), f"invalid frame kind {kind}")
        payload = exact(connection, length - 5)
        require(kind != 1 or len(payload) <= 32 * 1024, "oversized DATA frame")
        return kind, stream, payload

    serial = 0

    def rpc(method, params):
        global serial
        serial += 1
        payload = json.dumps({"jsonrpc": "2.0", "id": serial,
                              "method": method, "params": params}).encode()
        connection.settimeout(budget())
        connection.sendall(struct.pack("<IBI", len(payload) + 5, 0, 0) + payload)
        kind, stream, payload = frame()
        require((kind, stream) == (0, 0), "expected CONTROL reply on stream 0")
        response = json.loads(payload)
        require(response.get("id") == serial and response.get("jsonrpc") == "2.0",
                f"invalid RPC response: {response!r}")
        require("error" not in response, f"{method}: {response.get('error')!r}")
        return response["result"]

    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    connection.settimeout(budget())
    connection.connect(str(socket_path))
    version = run([binary, "--version"]).strip().split()[-1]
    welcome = rpc("hello", {"proto": {"major": 1, "minor": 0},
                           "caps": ["read.v1", "classic.v1"],
                           "client": {"kind": "cli", "version": version}})
    require("read.v1" in welcome["caps"] and "classic.v1" in welcome["caps"],
            "daemon lacks classic/read capabilities")
    build = corral("build-identity").strip()

    def reconnect():
        global connection
        connection.close()
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connection.settimeout(budget())
        connection.connect(str(socket_path))
        rpc("hello", {"proto": {"major": 1, "minor": 0},
                      "caps": ["read.v1", "classic.v1"],
                      "client": {"kind": "cli", "version": version}})

    def classic():
        reconnect()
        reply = rpc("read", {"kind": "classic_frame", "exe": binary, "build": build})
        body = bytearray()
        for _ in range(reply["frames"]):
            kind, stream, payload = frame()
            require((kind, stream) == (1, reply["stream"]), "unexpected classic DATA frame")
            body.extend(payload)
        require(len(body) == reply["bytes"], "classic frame byte count mismatch")
        return json.loads(body)

    def next_pass():
        # Do not retain an idle connection across CLI bursts: server idle I/O
        # deadlines are intentionally shorter than the smoke's overall budget.
        reconnect()
        # A non-owner raw nudge only wakes the loop; unlike source cli/board, it
        # does NOT request Full(Owner), mutate the store, or change the planner rules.
        before = len(passes())
        ack = rpc("nudge", {"source": "smoke-incremental", "wait_ms": 5000})
        require(ack["completed"], "incremental nudge did not complete within 5 s")
        return wait_for(lambda: passes()[before:], 2, "completed pass timing")

    def quiet(record):
        return not n(record, "sync.full") and n(record, "sync.quiet") == 1

    def settle():
        for _ in range(8):
            found = next_pass()
            if quiet(found[-1]):
                return
        raise SmokeFailure("store did not settle to an incremental quiet pass")

    stage = "1 quiet passes"
    settle()
    # Warm the daemon's reader before measuring, and drain its first full frame's
    # spans into a pass record. Classic/reader has no separate timing writer:
    # timing::pass snapshots process-wide counters, INCLUDING the reader thread
    # (timing.rs snapshot/record). With no command/PM clients in this window,
    # classic.* identifies frame work without pretending to have per-read records.
    classic()
    settle()
    quiet_start = len(passes())
    quiet_started = time.monotonic()
    before = feed()
    sampled = []
    for _ in range(3):
        sampled.extend(next_pass())
    after = feed()
    require(len(sampled) >= 3 and all(quiet(r) for r in sampled),
            f"expected consecutive quiet incremental passes: {sampled!r}")
    require(all(r["git_calls"] == 0 for r in sampled), f"quiet pass ran git: {sampled!r}")
    require(before[1] == after[1], f"quiet passes changed application sequence: {before} -> {after}")
    print(f"check 1: {len(sampled)} quiet incremental passes, git=0, application seq={before[1]}"
          f" unchanged (raw {before[0]}->{after[0]}; leases/app-commands excluded)", flush=True)

    # Keep the frame warm with no writes across its 15 s gauges and 30 s attention
    # periods. Natural passes may run clock-driven steps, so require three quiet
    # passes, not that every pass in this longer window is sync.quiet.
    stage = "1 quiet classic frame reuse"
    while time.monotonic() - quiet_started < 31.5:
        require(daemon.poll() is None, f"daemon exited {daemon.returncode}")
        time.sleep(0.1)
    # Flush all reader-thread spans since the previous pass, including work that
    # finished between passes. No daemon command records overlap this window.
    next_pass()
    window = passes()[quiet_start:]
    quiet_records = [r for r in window if quiet(r)]
    require(len(quiet_records) >= 3, f"fewer than three quiet frame-window passes: {window!r}")
    require(all(r["git_calls"] == 0 for r in quiet_records),
            f"quiet frame-window pass ran git: {quiet_records!r}")
    require(feed()[1] == before[1], f"quiet classic window changed application sequence: {before} -> {feed()}")
    for forbidden in ("pm.workset", "board.workset"):
        require(not any(n(r, forbidden) for r in window),
                f"quiet classic window recomputed {forbidden}: {window!r}")
    refreshes = sum(n(r, "classic.tick") for r in window)
    parts = {}
    for record in window:
        for name, value in record["spans"].items():
            if (name.startswith(("classic.", "board.frame.", "reader."))
                    or name == "sync.attention_phase_reconcile") and name not in (
                "classic.tick", "classic.encode",
            ):
                parts[name] = parts.get(name, 0) + value["n"]
    require(refreshes > 0 and parts.get("classic.resources", 0) > 0,
            f"no positive gauges refresh observation: ticks={refreshes}, parts={parts!r}")
    attention_refreshes = (parts.get("classic.attention", 0)
                           + parts.get("sync.attention_phase_reconcile", 0))
    require(attention_refreshes > 0,
            f"no positive 30s attention refresh observation: {parts!r}")
    print(f"check 1 frame reuse: {len(quiet_records)} quiet passes over"
          f" {time.monotonic() - quiet_started:.2f}s, classic refresh attempts={refreshes},"
          f" recomputed parts={json.dumps(parts, sort_keys=True)};"
          " pm.workset=0 board.workset=0; 15s gauges and 30s attention covered",
          flush=True)

    stage = "2 audit mixed burst"
    audit = runtime / "sync-audit-on"
    audit.touch()
    audit_start = len(passes())
    # Routed command workers wake the projector, not the sync loop. Only explicit
    # sync/daemon-nudge CLI paths send source=cli. This burst is consumed under audit.
    # Non-owner filing origin permits the real Idea -> Blocked -> Idea column
    # round trip without manufacturing owner consent (origin.rs protection).
    # --force permits the non-manual Blocked edge; neither move queues a launch.
    a = corral("new", "audit one", "--idea", "--origin", "agent",
               "--label", "owner-only").strip()
    b = corral("new", "audit two", "--idea", "--label", "owner-only").strip()
    corral("pin", a)
    corral("unpin", a)
    corral("comment", b, "audit-note")
    corral("move", a, "blocked", "--force", "--reason", "smoke audit")
    corral("move", a, "idea", "--force")
    git("commit", "-q", "--allow-empty", "-m", "audit main movement")
    config.touch()
    after_burst = len(passes())
    while len(passes()) < after_burst + 3:
        next_pass()
    audited = passes()[audit_start:]
    require(sum(n(r, "sync.audit.pass") for r in audited) > 0,
            f"no positively recorded audit pass: {audited!r}")
    require(sum(n(r, "sync.audit.wrong_skip") for r in audited) == 0,
            f"audit found wrong skips: {audited!r}")
    steps = sum(n(r, "sync.audit.step") for r in audited)
    require(steps > 0, "audit recorded no step decisions")
    # A config reload can take precedence over audit. A separate git movement after
    # the reload proves a git-triggered pass is actually audited, not only full/config.
    git("commit", "-q", "--allow-empty", "-m", "audited git movement")
    git_audit = next_pass()[-1]
    require(n(git_audit, "sync.audit.pass") == 1 and n(git_audit, "sync.start_queued") > 0,
            f"git movement was not audited: {git_audit!r}")
    audited = passes()[audit_start:]
    require(not any(n(r, "sync.audit.wrong_skip") for r in audited), "git audit found wrong skips")
    steps = sum(n(r, "sync.audit.step") for r in audited)
    print(f"check 2: zero sync.audit.wrong_skip; audited steps={steps}"
          " (routed metadata/new burst, config settling, and post-config git movement)", flush=True)

    stage = "3 incremental classic pickup"
    audit.unlink()
    settle()
    started = time.monotonic()
    pickup = corral("new", "pickup task", "--idea", "--label", "owner-only").strip()
    corral("move", pickup, "blocked", "--force", "--reason", "pickup smoke")
    picked = next_pass()[-1]
    require(not n(picked, "sync.full") and not n(picked, "sync.quiet"),
            f"pickup pass was not active incremental: {picked!r}")

    def reflected():
        document = classic()
        return document if any(h["task"]["id"] == pickup and h["task"]["status"] == "needs_me"
                               for h in document["heads"]) else None

    document = wait_for(reflected, max(0.01, 10 - (time.monotonic() - started)),
                        "classic frame pickup within 10 s")
    elapsed = time.monotonic() - started
    require(elapsed < 10, f"classic pickup took {elapsed:.3f} s")
    print(f"check 3: classic_frame read reflects blocked task in {elapsed:.3f}s"
          f" after incremental pass (frame rev={document['rev']}, seq={document['seq']})", flush=True)

    stage = "4 git movement"
    settle()
    git("commit", "-q", "--allow-empty", "-m", "incremental main movement")
    moved = next_pass()[-1]
    # Head-phase work is PER_TASK and has no timing span of its own. Main movement
    # fires StartQueued and the Autopilot git_repos rule, not HeadLoop. This fixture
    # has no Review runs: the autopilot wrapper is observable, but no landing is run.
    require(not n(moved, "sync.full") and not n(moved, "sync.quiet"),
            f"git movement did not produce active incremental pass: {moved!r}")
    require(n(moved, "sync.start_queued") > 0 and n(moved, "sync.autopilot_advance_all") > 0,
            f"missing git-triggered phase spans: {moved!r}")
    following = next_pass()[-1]
    require(quiet(following) and following["git_calls"] == 0,
            f"pass after git movement was not quiet: {following!r}")
    print("check 4: main movement runs sync.start_queued/autopilot; next pass quiet", flush=True)

    stage = "5 full switch"
    full = runtime / "sync-full-on"
    full.touch()
    forced = []
    for _ in range(3):
        forced.extend(next_pass())
    require(len(forced) >= 3 and all(n(r, "sync.full") == 1
            and n(r, "sync.full.switch") == 1 for r in forced),
            f"full switch did not force three passes: {forced!r}")
    full.unlink()
    returned = next_pass()[-1]
    require(not n(returned, "sync.full"), f"switch removal did not restore incremental: {returned!r}")
    settle()
    print("check 5: three full/switch passes; removal restores incremental", flush=True)

    stage = "6 clock-driven supervision"
    # Shortest unconditional periodic step is Supervise, 15 s (incremental_rules).
    # Other generic external-input steps are 20 s; no live task/deadline is needed.
    baseline = feed()
    prior = [r for r in passes() if n(r, "daemon.supervise")]
    require(bool(prior), "no prior daemon.supervise timestamp")
    last = prior[-1]
    position = len(passes())
    due_record = wait_for(
        lambda: next((r for r in passes()[position:] if n(r, "daemon.supervise")), None),
        18, "15 s clock-driven daemon.supervise on quiet store")
    parse = lambda r: datetime.datetime.fromisoformat(r["ts"].replace("Z", "+00:00"))
    interval = (parse(due_record) - parse(last)).total_seconds()
    require(interval >= 14, f"supervision ran before its 15 s due interval: {interval:.3f}s")
    require(not n(due_record, "sync.full"), f"clock step required a full pass: {due_record!r}")
    require(feed()[1] == baseline[1], f"clock step store was not quiet: {baseline} -> {feed()}")
    print(f"check 6: daemon.supervise runs clock-driven after {interval:.3f}s"
          " (shortest unconditional interval 15s; no application writes)", flush=True)
    print("smoke: ok", flush=True)
except Exception as error:
    print(f"smoke: failed: {stage}: {error}", file=sys.stderr)
    if daemon is not None:
        try:
            print("recent pass evidence: " + json.dumps(passes()[-5:]), file=sys.stderr)
        except Exception:
            pass
    if log is not None:
        log.flush()
        log.seek(0)
        print("private daemon log:\n" + log.read(), file=sys.stderr)
    sys.exit(1)
finally:
    signal.alarm(0)
    if connection is not None:
        connection.close()
    if daemon is not None and daemon.poll() is None:
        try:
            subprocess.run([binary, "--root", str(workspace), "daemon", "stop"], env=env,
                           stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, timeout=2, check=False)
            daemon.wait(timeout=1)
        except (subprocess.TimeoutExpired, OSError):
            try:
                os.killpg(daemon.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            daemon.wait(timeout=1)
    if log is not None:
        log.close()
    os.chdir("/")
    private.cleanup()
PY
