#!/usr/bin/env python3
"""Local PTY regression: copy from Settings and paste into a different surface while the
external clipboard stays stale. Requires the same pyte environment as tui-walk.py.
Run with /tmp/corral-tui-walk-venv/bin/python scripts/test-tui-copy.py after a TUI walk.
"""
import importlib.util
import os
from pathlib import Path
import socket
import tempfile
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location("tui_walk", Path(__file__).with_name("tui-walk.py"))
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


def locate(walk, text):
    walk.until(text, True, 8)
    # pyte columns, not string offsets: a wide glyph may precede our ASCII target.
    for y in range(walk.rows):
        for x in range(walk.cols - len(text) + 1):
            if all(walk.screen.buffer[y][x + i].data == c for i, c in enumerate(text)):
                return x + 1, y + 1
    raise AssertionError(f"could not locate {text}")


def mouse(walk, button, x, y, release=False):
    walk.send(f"\x1b[<{button};{x};{y}{'m' if release else 'M'}")
    walk.pump()


def click(walk, text):
    x, y = locate(walk, text)
    mouse(walk, 0, x, y)
    mouse(walk, 0, x, y, True)


def check():
    with tempfile.TemporaryDirectory(prefix="corral-native-copy-") as scratch:
        root = Path(scratch)
        helper = root / "wl-paste"
        helper.write_text('#!/bin/sh\ncase "$1" in --list-types) echo text/plain;; --no-newline) printf STALE;; *) exit 1;; esac\n')
        helper.chmod(0o700)
        for key in ["SSH_TTY", "SSH_CONNECTION", "SSH_CLIENT", "DISPLAY"]:
            os.environ.pop(key, None)
        os.environ.update(PATH=scratch + os.pathsep + os.environ["PATH"], WAYLAND_DISPLAY="corral-sample",
                          CORRAL_SOCKET=str(root / "no-daemon"), XDG_DATA_HOME=str(root / "data"), XDG_STATE_HOME=str(root / "state"))
        with socket.socket() as server:
            server.bind(("127.0.0.1", 0))
            port = server.getsockname()[1]
        walk = driver.Walk(SimpleNamespace(size="180x60", wait=8, colors="truecolor", background="1e1e1e", port=port))
        try:
            walk.until("Corral", True, 240)
            walk.step("fixture")
            click(walk, "Settings")
            click(walk, "Devices")
            click(walk, "Pair a device")
            # Synthetic preview only: no real device can claim this link.
            url = "http://127.0.0.1:8080/#pair=corral-pair-preview-sample"
            x, y = locate(walk, url)
            mouse(walk, 0, x + 1, y)
            mouse(walk, 32, x + 4, y)
            mouse(walk, 0, x + 4, y, True)
            walk.step("key:ctrl+a")  # select exactly this read-only link
            walk.step("key:ctrl+c")
            click(walk, "Home")
            walk.until("Needs you", True, 8)
            walk.step("key:/")
            walk.step("key:ctrl+v")
            walk.until(url, True, 8)
            walk.step("key:ctrl+v")
            walk.until(url + "STALE", True, 8)
        except Exception:
            walk.print_screen()
            raise
        finally:
            walk.close()
    print("PASS: the native Settings selection, then the simulated desktop clipboard, pasted into the composer")


if __name__ == "__main__":
    check()
