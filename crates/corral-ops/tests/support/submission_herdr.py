#!/usr/bin/env python3
"""Reuse operating_launch's fake, with acknowledged effect boundaries."""
import os
import runpy
import socket
import sys


def barrier(point):
    if os.environ.get("CORRAL_SUBMISSION_STOP") != point:
        return
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(30)
        stream.connect(os.environ["CORRAL_SUBMISSION_SOCKET"])
        stream.sendall((point + "\n").encode())
        assert stream.recv(1) == b"R"


args = sys.argv[1:]
if args[:1] == ["--session"]:
    args = args[2:]
effect = args[:2] in [["pane", "run"], ["agent", "prompt"]]
fixture = runpy.run_path(os.environ["CORRAL_SUBMISSION_ENGINE_FIXTURE"], run_name="fixture")
if effect:
    barrier("effect_entered")
fixture["main"]()
if effect:
    barrier("effect_returned")
