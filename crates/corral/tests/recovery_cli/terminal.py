"""Real controlling PTYs, frame barriers and bounded owned-process cleanup."""
import atexit
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import termios
import time

from screen import Screen, ansi2svg


ACTIVE = []
atexit.register(lambda: [terminal.close() for terminal in ACTIVE[:]])


class Terminal:
    def __init__(self, argv, cwd, evidence, cols=80, rows=24, framed=False):
        self.evidence = evidence
        evidence.mkdir(parents=True, exist_ok=True)
        self.cols, self.rows = cols, rows
        self.screen = Screen(cols, rows)
        self.raw = bytearray()
        self.queries = {}
        self.framed = framed
        self.frame = bytearray()
        self.status = None
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(cwd)
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
            os.environ["TERM"] = "xterm-256color"
            os.execv(argv[0], argv)
        self.closed = False
        ACTIVE.append(self)
        self.actions = (evidence / "inputs.jsonl").open("a")
        self.log({"start": argv, "pid": self.pid, "cols": cols, "rows": rows})

    def log(self, item):
        self.actions.write(json.dumps({"monotonic": time.monotonic(), **item}) + "\n")
        self.actions.flush()

    def pump(self, timeout=0.1):
        if self.status is not None:
            return False
        ready, _, _ = select.select([self.fd], [], [], timeout)
        if ready:
            try:
                data = os.read(self.fd, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                data = b""
            if data:
                self.raw.extend(data)
                if self.framed:
                    self.frame.extend(data)
                    # A PTY read can split one paint across many chunks. Publish cells only at
                    # ratatui's final cursor visibility command, never click a partial repaint.
                    end = max(self.frame.rfind(b"\x1b[?25l"), self.frame.rfind(b"\x1b[?25h"))
                    if end >= 0:
                        self.screen.feed(bytes(self.frame[:end + 6]))
                        del self.frame[:end + 6]
                else:
                    self.screen.feed(data)
                # Answer ordinary terminal capability probes, just as herdr's emulator does.
                # Without the final DSR the image picker can consume user keys as query input.
                for query, reply in [(b"\x1b_Gi=31", b"\x1b_Gi=31;ENOTSUP\x1b\\"),
                                     (b"\x1b[c", b"\x1b[?62;4;6;22c"),
                                     (b"\x1b[16t", b"\x1b[6;16;8t"),
                                     (b"\x1b[5n", b"\x1b[0n"),
                                     (b"\x1b[6n", b"\x1b[1;1R")]:
                    count = self.raw.count(query)
                    for _ in range(count - self.queries.get(query, 0)):
                        os.write(self.fd, reply)
                    self.queries[query] = count
                return True
            _, status = os.waitpid(self.pid, 0)
            self.status = os.waitstatus_to_exitcode(status)
        return False

    def wait(self, predicate, description, timeout=20):
        deadline = time.monotonic() + timeout
        while not predicate():
            if self.status is not None or time.monotonic() >= deadline:
                self.capture("FAILED")
                from pathlib import Path
                waits = {}
                for task in Path(f"/proc/{self.pid}/task").glob("*"):
                    try:
                        waits[task.name] = (task / "wchan").read_text()
                    except FileNotFoundError:
                        pass
                (self.evidence / "FAILED-waits.json").write_text(json.dumps(waits))
                raise AssertionError(f"real terminal failed waiting for {description}; exit={self.status}\n{self.screen.text()}")
            self.pump(min(0.2, max(0, deadline - time.monotonic())))

    def contains(self, text):
        self.wait(lambda: text in self.screen.text(), repr(text))

    def send(self, data, label=None):
        self.log({"input": label or data.decode("utf-8", "backslashreplace")})
        os.write(self.fd, data)
        self.pump(0.1)

    def key(self, key):
        keys = {"tab": b"\t", "backtab": b"\x1b[Z", "enter": b"\r", "esc": b"\x1b", "right": b"\x1b[C", "left": b"\x1b[D", "up": b"\x1b[A", "down": b"\x1b[B", "pageup": b"\x1b[5~", "pagedown": b"\x1b[6~", "space": b" ", "ctrl-u": b"\x15"}
        self.send(keys.get(key, key.encode()), key)

    def text(self, text):
        self.send(text.encode(), "text:" + text)

    def click(self, needle):
        self.contains(needle)
        x, y = self.screen.locate(needle)
        self.send(f"\x1b[<0;{x+1};{y+1}M\x1b[<0;{x+1};{y+1}m".encode(), f"click {needle!r} cell {x},{y}")

    def wheel(self, down):
        button = 65 if down else 64
        self.send(f"\x1b[<{button};{self.cols//2};{self.rows//2}M".encode(), "wheel down" if down else "wheel up")

    def resize(self, cols, rows):
        self.cols, self.rows = cols, rows
        self.screen = Screen(cols, rows)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        os.kill(self.pid, signal.SIGWINCH)
        self.log({"resize": [cols, rows]})

    def capture(self, name):
        (self.evidence / f"{name}.txt").write_text(self.screen.text())
        rows = [[(cell, ansi2svg.width(cell[0]), self.screen.styles[y][x])
                 for x, cell in enumerate(row) if cell] for y, row in enumerate(self.screen.cells)]
        (self.evidence / f"{name}.svg").write_text(ansi2svg.render(rows, f"Actual PTY viewport: {name}"))
        (self.evidence / f"{name}.ansi").write_bytes(self.raw)
        (self.evidence / f"{name}.terminal.json").write_text(json.dumps({"cols": self.cols, "rows": self.rows, "real_pty": True, "pid": self.pid}))

    def close(self):
        if self.closed:
            return
        self.closed = True
        ACTIVE.remove(self)
        try:
            if self.status is None:
                try:
                    os.killpg(self.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                deadline = time.monotonic() + 2
                while self.status is None and time.monotonic() < deadline:
                    self.pump(0.1)
                if self.status is None:
                    try:
                        os.killpg(self.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    _, status = os.waitpid(self.pid, 0)
                    self.status = os.waitstatus_to_exitcode(status)
            self.capture("final")
            self.log({"exit": self.status})
        finally:
            os.close(self.fd)
            self.actions.close()
