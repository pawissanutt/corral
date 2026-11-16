"""Real controlling PTYs and a small VT screen reader (never ratatui TestBackend)."""
import atexit
import codecs
import errno
import fcntl
import json
import os
import pty
import re
import select
import signal
import struct
import termios
import time
import unicodedata


class Screen:
    def __init__(self, cols, rows):
        self.cols, self.rows = cols, rows
        self.cells = [[" "] * cols for _ in range(rows)]
        self.x = self.y = 0
        self.saved = (0, 0)
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        while self.pending:
            if self.pending.startswith("\x1b["):
                match = re.match(r"\x1b\[([0-?]*)([ -/]*)([@-~])", self.pending)
                if not match:
                    return
                self.pending = self.pending[match.end():]
                self.csi(match[1], match[3])
                continue
            if self.pending.startswith(("\x1b]", "\x1b_", "\x1bP")):
                match = re.search(r"\x07|\x1b\\", self.pending)
                if not match:
                    return
                self.pending = self.pending[match.end():]
                continue
            if self.pending[0] == "\x1b":
                if len(self.pending) < 2:
                    return
                op = self.pending[1]
                if op == "7":
                    self.saved = (self.x, self.y)
                elif op == "8":
                    self.x, self.y = self.saved
                # Charset designations consume a third byte.
                n = 3 if op in "()" else 2
                if len(self.pending) < n:
                    return
                self.pending = self.pending[n:]
                continue
            c, self.pending = self.pending[0], self.pending[1:]
            if c == "\r":
                self.x = 0
            elif c in "\n\v\f":
                self.y += 1
                if self.y == self.rows:
                    self.cells.pop(0)
                    self.cells.append([" "] * self.cols)
                    self.y -= 1
            elif c == "\b":
                self.x = max(0, self.x - 1)
            elif c == "\t":
                self.x = min(self.cols - 1, (self.x // 8 + 1) * 8)
            elif ord(c) >= 32 and ord(c) != 127:
                if unicodedata.combining(c):
                    if self.x:
                        self.cells[self.y][self.x - 1] += c
                    continue
                if self.x >= self.cols:
                    self.x = 0
                    self.y = min(self.rows - 1, self.y + 1)
                self.cells[self.y][self.x] = c
                width = 2 if unicodedata.east_asian_width(c) in "WF" else 1
                if width == 2 and self.x + 1 < self.cols:
                    self.cells[self.y][self.x + 1] = ""
                self.x += width

    def csi(self, args, op):
        private = args.startswith("?")
        values = [int(x or 0) for x in args.lstrip("?><=").split(";")]
        n = values[0] or 1
        if op in "Hf":
            self.y = min(self.rows - 1, n - 1)
            self.x = min(self.cols - 1, (values[1] or 1) - 1) if len(values) > 1 else 0
        elif op == "A":
            self.y = max(0, self.y - n)
        elif op == "B":
            self.y = min(self.rows - 1, self.y + n)
        elif op == "C":
            self.x = min(self.cols - 1, self.x + n)
        elif op == "D":
            self.x = max(0, self.x - n)
        elif op == "G":
            self.x = min(self.cols - 1, n - 1)
        elif op == "d":
            self.y = min(self.rows - 1, n - 1)
        elif op == "J":
            if values[0] in (2, 3):
                self.cells = [[" "] * self.cols for _ in range(self.rows)]
            elif values[0] == 0:
                self.cells[self.y][self.x:] = [" "] * (self.cols - self.x)
                for y in range(self.y + 1, self.rows):
                    self.cells[y] = [" "] * self.cols
        elif op == "K":
            a, b = (0, self.cols) if values[0] == 2 else (0, self.x + 1) if values[0] == 1 else (self.x, self.cols)
            self.cells[self.y][a:b] = [" "] * (b - a)
        elif private and op == "h" and 1049 in values:
            self.cells = [[" "] * self.cols for _ in range(self.rows)]
            self.x = self.y = 0

    def text(self):
        return "\n".join("".join(row).rstrip() for row in self.cells) + "\n"

    def locate(self, needle):
        for y, row in enumerate(self.cells):
            line = "".join(row)
            if needle in line:
                # Find a terminal cell, not a UTF-8 byte or Python string offset.
                offset = line.index(needle)
                x = 0
                for cell in row:
                    if offset <= 0:
                        return x, y
                    offset -= len(cell)
                    x += 1
        return None


ACTIVE = []
atexit.register(lambda: [terminal.close() for terminal in ACTIVE[:]])


class Terminal:
    def __init__(self, argv, cwd, evidence, cols=80, rows=24):
        self.evidence = evidence
        evidence.mkdir(parents=True, exist_ok=True)
        self.cols, self.rows = cols, rows
        self.screen = Screen(cols, rows)
        self.raw = bytearray()
        self.queries = {}
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
