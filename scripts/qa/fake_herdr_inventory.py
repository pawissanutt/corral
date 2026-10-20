"""Synthesize a steady herdr inventory for a copied store: every open run, review, plan run and
running gate pane is listed with its last recorded agent state (working when unknown).

Usage: fake_herdr_inventory.py TASKS_DIR OUT_DIR
"""

import json
import pathlib
import sys

import yaml

STATES = {"working", "idle", "blocked", "done"}


def frontmatter(path):
    text = path.read_text()
    if not text.startswith("---\n"):
        return None
    end = text.find("\n---\n", 4)
    return yaml.safe_load(text[4:end]) if end > 0 else None


def runs(task):
    stack = list(task.get("runs") or []) + list(task.get("plan_runs") or [])
    for plan in task.get("plans") or []:
        stack.extend(plan.get("reviews") or [])
    while stack:
        run = stack.pop()
        stack.extend(run.get("review_runs") or [])
        yield run


def main(tasks_dir, out):
    panes = {}
    for path in sorted(pathlib.Path(tasks_dir).glob("*.md")):
        task = frontmatter(path)
        if not isinstance(task, dict):
            continue
        task = task.get("task", task)
        for run in runs(task):
            herdr = run.get("herdr")
            if herdr and not run.get("ended") and not run.get("outcome"):
                state = run.get("agent_state")
                panes[herdr["pane_id"]] = (herdr.get("workspace_id", "w0"),
                                           state if state in STATES else "working")
            gate = run.get("gate") or {}
            if gate.get("pane") and gate.get("result") == "running":
                panes[gate["pane"]] = (gate["pane"].split(":")[0], "working")
    if not panes:
        panes["w0:p0"] = ("w0", "idle")
    out = pathlib.Path(out)
    (out / "get").mkdir(parents=True, exist_ok=True)
    listed = []
    for pane_id, (workspace, status) in sorted(panes.items()):
        info = {"workspace_id": workspace, "tab_id": f"{workspace}:t1", "pane_id": pane_id,
                "agent_status": status}
        listed.append(info)
        (out / "get" / f"{pane_id}.json").write_text(json.dumps({"pane": info}))
    workspaces = sorted({p["workspace_id"] for p in listed})
    (out / "panes.json").write_text(json.dumps({"panes": listed}))
    (out / "workspaces.json").write_text(json.dumps(
        {"workspaces": [{"workspace_id": w, "label": w} for w in workspaces]}))
    print(f"fake inventory: {len(listed)} panes in {len(workspaces)} workspaces")


if __name__ == "__main__":
    main(*sys.argv[1:3])
