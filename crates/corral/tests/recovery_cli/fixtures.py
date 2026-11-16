"""Disposable CLI fixtures and event-driven barriers for qa-recovery.sh (Linux)."""
import ctypes
import hashlib
import json
import os
from pathlib import Path
import select
import shlex
import subprocess
import time

from terminal import Terminal

BIN = os.environ["CORRAL"]
HERE = Path(__file__).resolve().parent
PLAN = """---
goal: isolated recovery evidence
non_goals: []
decisions: []
affected_paths: [a.rs]
contracts: []
dependencies: []
acceptance: [{scenario: fixture, expected: exact callback}]
verification: [inert fixture only]
owner_decisions: []
---
No model or provider is called.
"""


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def digest(data):
    return hashlib.sha256(data).hexdigest()


class Watch:
    """Subscribe before triggering; filesystem notifications are only wake hints."""
    def __init__(self, directories):
        libc = ctypes.CDLL(None, use_errno=True)
        self.fd = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
        if self.fd < 0:
            raise OSError(ctypes.get_errno(), "inotify_init1")
        for directory in directories:
            if libc.inotify_add_watch(self.fd, os.fsencode(directory), 0x100 | 0x2 | 0x8 | 0x80) < 0:
                self.close()
                raise OSError(ctypes.get_errno(), f"watch {directory}")

    def until(self, predicate, description, timeout=30):
        deadline = time.monotonic() + timeout
        while not predicate():
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.fd], [], [], remaining)[0]:
                raise AssertionError(f"timeout waiting for {description}")
            try:
                os.read(self.fd, 65536)
            except BlockingIOError:
                pass

    def close(self):
        os.close(self.fd)


class Fixture:
    def __init__(self, base, evidence, name, family=False):
        self.family = family
        self.codes = (["reviewer-family-policy"] if family else []) + ["configured-reviewer-binding"]
        self.root = base / name
        self.root.mkdir()
        self.ev = evidence / name
        self.ev.mkdir()
        self.commands = (self.ev / "commands.jsonl").open("w")
        self.c("init")
        app = self.root / "app"
        app.mkdir()
        subprocess.run(["git", "init", "-q", "-b", "main", str(app)], check=True)
        (app / "a.rs").write_text("// inert fixture\n")
        subprocess.run(["git", "-C", str(app), "add", "a.rs"], check=True)
        subprocess.run(["git", "-C", str(app), "commit", "-qm", "fixture"], check=True)
        command = f"python3 {shlex.quote(str(HERE / 'fixture_agent.py'))} {shlex.quote(str(self.root))} {{prompt_file}} {{model_args}}"
        config = f'''[defaults]
profile = "gpt-sol"
[projects.app]
path = "app"
[projects.app.review]
reviewer = "gpt-sol"
plan_accept = "owner"
'''
        if family:
            config += 'require_cross_family = true\n'
        config += f'''[pm]
agent = "gpt-sol"
[resources]
max_load_per_core = 1000000.0
min_mem_available = "1B"
min_disk_free = "1B"
[harnesses.fixture]
command = {json.dumps(command)}
model_flag = "--model"
[profiles.gpt-sol]
harness = "fixture"
model = "chatgpt-subscription/gpt-6.1-sol"
vendor = "openai"
[profiles.gpt-astra]
harness = "fixture"
model = "chatgpt-subscription/gpt-6-astra"
vendor = "openai"
'''
        self.config = self.root / ".corral/corral.toml"
        self.config.write_text(config)
        self.id = self.c("new", f"Recovery {name}", "-p", "app", "--ready").strip()
        self.c("dispatch", self.id, "--plan", "--profile", "gpt-sol")
        (self.root / "plan.md").write_text(PLAN)
        self.c("plan", "submit", self.id, str(self.root / "plan.md"))
        self.other = self.c("new", "Unrelated fixture", "--body", "Preserve me").strip()
        (self.root / "notes.md").write_text("Accept this exact disposable plan only.\n")
        self.control = self.root / ".corral/qa-control"
        self.control.mkdir(exist_ok=True)
        # Establish initial Assist admission/attention before measuring a cancelled UI action.
        # Otherwise the board's first independent sync legitimately adds authority_blocked.
        self.c("sync")
        self.watch = Watch([self.control, self.root / ".corral/db"])
        self.before = self.invariants()
        save(self.ev / "before.json", self.before)
        save(self.ev / "route.json", self.j("route", self.id, "--json"))

    def c(self, *args, ok=True):
        result = subprocess.run([BIN, "--root", str(self.root), *args], cwd=self.root,
                                stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=40)
        self.commands.write(json.dumps({"argv": list(args), "exit": result.returncode,
                                       "stdout": result.stdout, "stderr": result.stderr}) + "\n")
        self.commands.flush()
        if ok:
            assert result.returncode == 0, f"{args}: {result.stdout}\n{result.stderr}"
        else:
            assert result.returncode != 0, f"unexpected success: {args}"
        return result.stdout if ok else result.stderr

    def j(self, *args):
        return json.loads(self.c(*args))

    def task(self):
        return self.j("show", self.id, "--json")

    def history(self):
        return self.j("recover", self.id, "--history", "--json")

    def invariants(self):
        task = self.task()
        plan = task["plans"][-1]
        sha = plan["artifact"]["sha256"]
        reference = f"{self.id}:{plan['artifact']['run']}:{Path(plan['artifact']['path']).name}"
        snapshot = self.c("artifact", "show", reference, "--raw").encode()
        assert digest(snapshot) == sha
        return {"stored_snapshot_sha256": digest(snapshot), "config_sha256": digest(self.config.read_bytes()),
                "source_sha256": digest((self.root / "plan.md").read_bytes()),
                "artifact": plan["artifact"], "document": plan["document"],
                "other": self.j("show", self.other, "--json")}

    def callback(self, attempt, verdict="approve"):
        ready = self.control / f"{self.id}-{attempt}.ready.json"
        self.watch.until(ready.exists, "separate reviewer ready")
        identity = json.loads(ready.read_text())
        assert identity["environment"]["HERDR_PANE_ID"]
        save(self.ev / f"{attempt}-callback.json", identity)
        fd = os.open(self.control / f"{self.id}-{attempt}.fifo", os.O_WRONLY | os.O_NONBLOCK)
        try:
            os.write(fd, b"approve\n")
        finally:
            os.close(fd)
        def completed():
            review = next(r for r in self.task()["plans"][-1]["reviews"] if r["id"] == attempt)
            return review.get("verdict") == verdict
        self.watch.until(completed, f"canonical {attempt} {verdict}")
        save(self.ev / f"{attempt}-after-callback.json", self.task())

    def reviewer_args(self):
        codes = ["configured-reviewer-binding"]
        if "require_cross_family = true" in self.config.read_text():
            codes.insert(0, "reviewer-family-policy")
        return ["recover", self.id, "--action", "use-reviewer", "--stage", "plan-review",
                "--plan", "P1", "--reviewer", "gpt-astra", "--waive", ",".join(codes)]

    def accept_args(self):
        codes = ["configured-reviewer-binding"]
        if "require_cross_family = true" in self.config.read_text():
            codes.insert(0, "reviewer-family-policy")
        return ["recover", self.id, "--action", "accept-plan", "--plan", "P1",
                "--notes-file", str(self.root / "notes.md"), "--waive", ",".join(codes)]

    def apply_cli(self, args, label):
        preview = self.j(*args, "--json")
        save(self.ev / f"{label}-preview.json", preview)
        assert not preview["remaining_blockers"], preview
        with_terminal = Terminal([BIN, "--root", str(self.root), *args, "--apply", preview["digest"],
                                  "--reason", "One exact disposable fixture exception", "--json"],
                                 self.root, self.ev / label)
        try:
            with_terminal.wait(lambda: b"anything else cancels" in with_terminal.raw, "exact owner challenge")
            with_terminal.capture("challenge")
            with_terminal.text(f"confirm {self.id} {preview['digest']}\n")
            with_terminal.wait(lambda: with_terminal.status is not None, "CLI apply exit")
            assert with_terminal.status == 0, with_terminal.raw.decode(errors="replace")
        finally:
            with_terminal.close()

    def finish(self, recovered):
        assert self.invariants() == self.before, "config, original plan or unrelated task changed"
        task, history = self.task(), self.history()
        assert task["plans"][-1].get("approval"), task
        assert bool(history) == recovered
        assert bool(task.get("dispatch_hold", False)) == recovered
        assert not task.get("queued", False)
        if recovered:
            assert all(record["preview"]["request"]["exceptions"] == self.codes for record in history)
            assert task.get("reviewer") is None, "one-attempt recovery changed persistent selection"
            accepted = [link["acceptance"] for record in history for link in record["links"] if link.get("acceptance")]
            assert accepted and all(a["acceptance_mode"] == "owner_recovery" and not a["policy_satisfied"] for a in accepted)
        save(self.ev / "final.json", task)
        save(self.ev / "history.json", history)
        (self.ev / "events.jsonl").write_text(self.c("log", self.id, "--all", "--json"))
        self.watch.close()
        self.commands.close()
