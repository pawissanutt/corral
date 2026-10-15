#!/usr/bin/env python3
"""Protocol-only wrapper around queue_support/herdr.py; never runs real herdr."""

import json
import os
import pathlib
import socket
import sys


def main() -> None:
    root = pathlib.Path(__file__).parent
    args = sys.argv[1:]
    assert args[:1] == ["--session"], args
    session = args[1]
    assert session in ("owner", "foreign"), session
    key = " ".join(args[2:4])
    with (root / f"calls-{session}").open("a") as calls:
        calls.write(key + "\n")

    # Each CLI has its own barrier path. Claim it once so subsequent engine
    # invocations in that CLI cannot accidentally stop at the same point.
    barrier = os.environ.get("RACE_BARRIER")
    if barrier and key == os.environ["RACE_POINT"]:
        marker = pathlib.Path(barrier + ".claimed")
        try:
            with marker.open("x"):
                pass
        except FileExistsError:
            pass
        else:
            address = str(root / f"reply-{os.getpid()}.sock")
            try:
                with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as event:
                    event.settimeout(30)
                    event.bind(address)
                    event.sendto(f"{session}:{key}".encode(), barrier)
                    assert event.recv(32) == b"release"
            finally:
                pathlib.Path(address).unlink(missing_ok=True)

    if key == "pane list" and (
        session == "foreign" or os.environ.get("RACE_FOREIGN_SOURCE") == "1"
    ):
        # Server-scoped IDs may collide. This must never block the owner's run.
        print(
            json.dumps(
                {
                    "result": {
                        "panes": [
                            {
                                "pane_id": "w1:p1",
                                "workspace_id": "w1",
                                "tab_id": "w1:t1",
                                "cwd": str(root),
                                "agent_status": "blocked",
                            }
                        ]
                    }
                }
            )
        )
        return

    # The existing queue fixture implements durable workspace creation, pane
    # metadata and pane run, without executing the submitted agent command.
    os.execv(
        sys.executable,
        [sys.executable, str(root / "queue-engine"), *sys.argv[1:]],
    )


if __name__ == "__main__":
    main()
