#!/usr/bin/env python3
"""File-backed fake herdr; records launches but never executes agent commands."""

import json
import sys
from pathlib import Path


def pane(pane_id: str) -> dict[str, str]:
    """Return the inventory entry for a fixture pane."""
    return {
        "pane_id": pane_id,
        "workspace_id": "w2",
        "tab_id": pane_id.replace(":p", ":t"),
        "agent_status": "idle",
    }


def main() -> None:
    """Handle one synchronous IPC and persist its observable lifecycle effects."""
    root = Path(sys.argv[0]).parent
    state_path = root / "engine.json"
    state = json.loads(state_path.read_text())
    args = sys.argv[1:]
    if args[:1] == ["--session"]:
        args = args[2:]
    result = {}
    match args[:2]:
        case ["workspace", "list"]:
            result = {
                "workspaces": [{"workspace_id": "w2"}] if state["workspace"] else []
            }
        case ["workspace", "create"] | ["tab", "create"]:
            state["workspace"] = True
            state["next"] += 1
            created = pane(f'w2:p{state["next"]}')
            state["panes"].append(created["pane_id"])
            state["created"].append(created["pane_id"])
            # The ResourceProbe sees pressure only after pane preparation completes.
            (root / "replacement-prepared").write_text(created["pane_id"])
            result = {
                "workspace": {"workspace_id": "w2"},
                "root_pane": created,
            }
        case ["pane", "get"]:
            if args[2] in state["panes"]:
                result = {"pane": pane(args[2])}
        case ["pane", "list"] | ["agent", "list"]:
            key = "panes" if args[0] == "pane" else "agents"
            result = {key: [pane(value) for value in state["panes"]]}
        case ["pane", "run"]:
            assert args[2] in state["panes"], args
            state["launched"].append(args[2])
            state["commands"].append(args[3])
        case ["pane", "close"]:
            state["closed"].append(args[2])
            if args[2] in state["panes"]:
                state["panes"].remove(args[2])
        case ["pane", "read"]:
            print("> ")
            return
        case ["agent", "prompt"]:
            assert args[2] in state["panes"], args
        case _:
            raise AssertionError(f"unexpected fake herdr call: {args}")
    state_path.write_text(json.dumps(state))
    print(json.dumps({"id": "fake", "result": result}))


if __name__ == "__main__":
    main()
