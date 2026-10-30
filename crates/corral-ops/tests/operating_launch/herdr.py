#!/usr/bin/env python3
"""Disposable terminal engine with an acknowledged, pre-submission IPC barrier."""
import json
import os
import selectors
import socket
import subprocess
import sys
from pathlib import Path


# One hang guard for the whole controller operation (store locks, git and sync),
# not an IPC latency expectation. Keep in step with Barrier::arrival in Rust.
BARRIER_BUDGET = 300


def wait_for_release(channel: socket.socket) -> None:
    """Wait for the controller's release event; EOF is a failed barrier, not release."""
    with selectors.DefaultSelector() as events:
        events.register(channel, selectors.EVENT_READ)
        if not events.select(BARRIER_BUDGET):
            raise TimeoutError("controller did not release the prepared barrier")
        if channel.recv(1) != b"R":
            raise AssertionError("controller disconnected without releasing the barrier")


def publish(path: Path, value: object) -> None:
    """Publish complete JSON before readers can observe the new state."""
    temporary = path.with_name(f"{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(value) + "\n")
    temporary.replace(path)


def main() -> None:
    directory = Path(sys.argv[0]).parent
    args = sys.argv[1:]
    if args[:1] == ["--session"]:
        args = args[2:]
    pair = tuple(args[:2])
    state_path = directory / "panes.json"
    state = json.loads(state_path.read_text()) if state_path.exists() else []
    result = {}
    match pair:
        case ("workspace", "list"):
            result = {"workspaces": [
                {"workspace_id": pane["workspace_id"], "label": pane["label"]}
                for pane in state
            ]}
        case ("workspace", "create") | ("tab", "create"):
            number = len(state) + 1
            pane = {
                "pane_id": f"w{number}:p1",
                "tab_id": f"w{number}:t1",
                "workspace_id": f"w{number}",
                "label": args[args.index("--label") + 1] if "--label" in args else "fixture",
                "agent_status": "idle",
            }
            state.append(pane)
            publish(state_path, state)
            publish(directory / f"{pane['pane_id']}.json", pane)
            result = {"workspace": pane, "root_pane": pane}
        case ("pane", "list"):
            result = {"panes": [
                {"pane_id": "baseline:p1", "workspace_id": "baseline",
                 "tab_id": "baseline:t1", "agent_status": "idle"},
                *state,
            ]}
        case ("pane", "get"):
            result = {"pane": next(pane for pane in state if pane["pane_id"] == args[2])}
        case ("pane", "process-info"):
            result = {"process_info": {
                "shell_pid": 10,
                "foreground_processes": [{"pid": 11, "argv": ["fixture-agent"]}],
            }}
        case ("pane", "report-metadata"):
            barrier = directory / "barrier.sock"
            if barrier.exists():
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.connect(str(barrier))
                    channel.sendall(json.dumps(args).encode() + b"\n")
                    wait_for_release(channel)
        case ("pane", "run"):
            with (directory / "submissions.jsonl").open("a") as log:
                log.write(json.dumps(args) + "\n")
            if "/state/gates/" in args[3] and not (directory / "hold-gates").exists():
                environment = os.environ.copy()
                environment["CORRAL_ROOT"] = str(directory.parent)
                subprocess.run(args[3], shell=True, check=False, stdin=subprocess.DEVNULL,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                               env=environment)
        case ("pane", "close"):
            with (directory / "closed.jsonl").open("a") as log:
                log.write(json.dumps(args) + "\n")
        case ("agent", "prompt"):
            with (directory / "deliveries.jsonl").open("a") as log:
                log.write(json.dumps(args) + "\n")
        case _:
            result = {}
    print(json.dumps({"result": result}))


if __name__ == "__main__":
    main()
