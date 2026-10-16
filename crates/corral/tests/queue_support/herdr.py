#!/usr/bin/env python3
import fcntl, json, os, pathlib, signal, socket, sys
from typing import TypedDict

class Pane(TypedDict):
    pane_id: str
    workspace_id: str
    tab_id: str
    cwd: str
    agent_status: str

class Workspace(TypedDict):
    workspace_id: str
    label: str

class State(TypedDict):
    panes: list[Pane]
    workspaces: list[Workspace]
    creates: int
    submits: int

root = pathlib.Path(__file__).parent
os.environ["CORRAL_ROOT"] = str(root)
for name in ("CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE",
             "HERDR_PANE_ID", "HERDR_WORKSPACE_ID", "HERDR_TAB_ID"):
    os.environ.pop(name, None)
args = sys.argv[1:]
if args[:1] == ["--session"]: args = args[2:]
key = " ".join(args[:2])
def barrier(point: str) -> None:
    """Pause one exact protocol boundary without holding the fixture state lock."""
    marker = root / "barrier"
    if not marker.exists() or marker.read_text() != point:
        return
    marker.unlink()
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as channel:
        channel.bind(str(root / "barrier-peer.sock"))
        channel.settimeout(10)
        _ = channel.sendto(point.encode(), str(root / "barrier-test.sock"))
        assert channel.recv(1) == b"1"

barrier("before " + key)
if key == "workspace list" and any(
    "reserved: true" in p.read_text() for p in (root / ".corral/tasks").glob("*.md")
):
    barrier("claimed workspace list")

def hook(point):
    marker = root / "fault"
    if marker.exists() and marker.read_text() == point:
        marker.unlink()
        if os.environ.get("CRASH_LAUNCH") == "1":
            os.kill(os.getppid(), signal.SIGKILL)
            sys.exit(0)
        print(json.dumps({"error":{"code":"injected","message":point}}))
        sys.exit(1)
with (root / "protocol.lock").open("a") as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    path = root / "protocol.json"
    state: State = json.loads(path.read_text()) if path.exists() else {"panes":[], "workspaces":[], "creates":0, "submits":0}
    hook("before " + key)
    result: dict[str, object] = {}
    if key == "workspace list":
        if any("reserved: true" in p.read_text() for p in (root / ".corral/tasks").glob("*.md")):
            hook("claimed workspace list")
        result = {"workspaces":state["workspaces"]}
    elif key in ["workspace create", "tab create"]:
        state["creates"] += 1
        n = str(state["creates"])
        wid = args[args.index("--workspace")+1] if key == "tab create" else "w" + n
        created_pane: Pane = {"pane_id":wid+":p"+n, "workspace_id":wid, "tab_id":wid+":t"+n,
                "cwd":args[args.index("--cwd")+1], "agent_status":"unknown"}
        ws: Workspace = {"workspace_id":wid, "label":args[args.index("--label")+1]}
        if key == "workspace create": state["workspaces"].append(ws)
        state["panes"].append(created_pane)
        result = {"workspace":ws, "root_pane":created_pane}
    elif key == "pane list":
        # A non-corral observer makes this a usable inventory even before admission.
        observer: Pane = {"pane_id":"w0:p1", "workspace_id":"w0", "tab_id":"w0:t1",
                          "cwd":str(root), "agent_status":"unknown"}
        result = {"panes":[*state["panes"], observer]}
    elif key == "pane get":
        pane = next((p for p in state["panes"] if p["pane_id"] == args[2]), None)
        if pane is None:
            print('{"error":{"code":"pane_not_found","message":"gone"}}'); sys.exit(1)
        result = {"pane":pane}
    elif key == "pane process-info":
        # pane run models a live harness; planner smoke checks now inspect it.
        result = {"process_info":{"shell_pid":100,"foreground_processes":[
            {"pid":101,"argv":["fixture-agent"]}
        ]}}
    elif key == "pane close":
        state["panes"] = [p for p in state["panes"] if p["pane_id"] != args[2]]
        state["workspaces"] = [w for w in state["workspaces"] if any(p["workspace_id"] == w["workspace_id"] for p in state["panes"])]
    elif key in ["pane run", "agent prompt"]:
        state["submits"] += 1
    elif key in ["pane report-metadata", "notification show"]: pass
    else:
        print(json.dumps({"error":{"code":"unexpected","message":key}})); sys.exit(1)
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(state)); temp.replace(path)
    hook("after " + key)
    print(json.dumps({"result":result}))
