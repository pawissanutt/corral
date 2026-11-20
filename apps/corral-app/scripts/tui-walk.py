#!/usr/bin/env python3
"""Drive the app's terminal target in a pseudo-terminal and print what it shows.

Starts `nsh run --target tui --port <port>` at a given size, feeds steps, and prints the screen
as text; `attrs` steps also list the reverse-video, bold and coloured cells, so selection and
focus can be read. The emulator is `pyte`, from a scratch virtual environment under /tmp that is
created when missing; nothing else is written outside /tmp.

Steps (arguments, or lines of a --steps file; `#` starts a comment line):
  key:NAME[*N]  a key: enter tab stab esc up down left right home end pgup pgdn bs del space,
                ctrl+X, alt+X, or a single character; `*N` repeats it
  type:TEXT     typed text
  wait:TEXT     wait until TEXT is on screen (fails after --wait seconds)
  gone:TEXT     wait until TEXT is no longer on screen
  sleep:SECS    let the app run
  screen        print the screen
  rows:A[-B]    print screen rows A to B
  attrs[:all]   print styled runs: reverse, bold, underline and background (`all` adds foreground)
  diff          print the styled runs that changed since the last attrs/diff: where focus moved
  fixture       Ctrl+K, 'Data: fixture', Enter: the explicit sample preview

Example: tui-walk.py --size 140x44 fixture wait:Needs screen key:tab*3 attrs
"""

import argparse
import os
import pty
import select
import signal
import subprocess
import sys
import time

VENV = "/tmp/corral-tui-walk-venv"
APP = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def ensure_pyte():
    try:
        import pyte  # noqa: F401
        return
    except ImportError:
        pass
    python = os.path.join(VENV, "bin", "python")
    if not os.path.exists(python):
        subprocess.run([sys.executable, "-m", "venv", VENV], check=True)
        subprocess.run([python, "-m", "pip", "install", "-q", "pyte"], check=True)
    if os.path.realpath(sys.prefix) == os.path.realpath(VENV):
        sys.exit("pyte is missing from " + VENV)
    os.execv(python, [python, os.path.abspath(__file__), *sys.argv[1:]])


KEYS = {
    "enter": "\r", "tab": "\t", "stab": "\x1b[Z", "esc": "\x1b", "up": "\x1b[A",
    "down": "\x1b[B", "right": "\x1b[C", "left": "\x1b[D", "home": "\x1b[H", "end": "\x1b[F",
    "pgup": "\x1b[5~", "pgdn": "\x1b[6~", "bs": "\x7f", "del": "\x1b[3~", "space": " ",
}


def key_bytes(name):
    if name in KEYS:
        return KEYS[name]
    if name.startswith("ctrl+") and len(name) == 6:
        return chr(ord(name[5].lower()) & 0x1F)
    if name.startswith("alt+") and len(name) == 5:
        return "\x1b" + name[4]
    if len(name) == 1:
        return name
    raise SystemExit(f"unknown key: {name}")


class Walk:
    def __init__(self, args):
        import pyte

        self.cols, self.rows = (int(n) for n in args.size.split("x"))
        self.wait = args.wait
        walk = self

        class Screen(pyte.Screen):
            def write_process_input(self, data):
                walk.send(data)

        self.screen = Screen(self.cols, self.rows)
        self.stream = pyte.ByteStream(self.screen)
        pid, fd = pty.fork()
        if pid == 0:
            import fcntl
            import struct
            import termios

            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", self.rows, self.cols, 0, 0))
            os.chdir(APP)
            # The app is drawn as on the owner's terminal: an inherited NO_COLOR (agent shells set
            # it) would make NSH draw everything in reverse video and bold alone.
            os.environ.pop("NO_COLOR", None)
            os.environ.update(
                TERM="xterm-256color", COLUMNS=str(self.cols), LINES=str(self.rows),
                COLORTERM="truecolor", NSH_TUI_COLORS=args.colors,
            )
            # A walk must never touch the owner's saved preferences, enrolment or drafts: unless
            # the caller sets these itself, the app's data and state live under /tmp.
            for name in ("XDG_DATA_HOME", "XDG_STATE_HOME"):
                if not os.environ.get(name):
                    scratch = os.path.join("/tmp/corral-tui-walk-home", name.lower())
                    os.makedirs(scratch, exist_ok=True)
                    os.environ[name] = scratch
            os.execvp("nsh", ["nsh", "run", "--target", "tui", "--port", str(args.port)])
        self.pid, self.fd = pid, fd
        self.background = args.background

    def answer_colors(self, data):
        """Answers NSH's startup colour queries (OSC 10 and 11), which pyte ignores, as a
        terminal with the given background does; without the answer NSH draws no surfaces."""
        if b"\x1b]11;?" not in data or self.background == "none":
            return
        back = self.background.lstrip("#")
        fore = "d4d4d4" if int(back[:2], 16) < 0x80 else "1e1e1e"
        rgb = lambda hex6: "/".join(hex6[i : i + 2] * 2 for i in (0, 2, 4))
        self.send(f"\x1b]10;rgb:{rgb(fore)}\x1b\\\x1b]11;rgb:{rgb(back)}\x1b\\")

    def send(self, data):
        os.write(self.fd, data.encode() if isinstance(data, str) else data)

    def pump(self, quiet=0.3, limit=5.0):
        """Reads output until the app has been quiet for `quiet` seconds."""
        end = time.time() + limit
        last = time.time()
        while time.time() < end and time.time() - last < quiet:
            ready, _, _ = select.select([self.fd], [], [], 0.05)
            if ready:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    raise SystemExit("the app exited:\n" + self.text())
                self.answer_colors(data)
                self.stream.feed(data)
                last = time.time()

    def text(self):
        return "\n".join(line.rstrip() for line in self.screen.display)

    def until(self, needle, present, limit):
        end = time.time() + limit
        while time.time() < end:
            self.pump(quiet=0.1, limit=0.5)
            if (needle in self.text()) == present:
                return
        word = "appear" if present else "go"
        print(self.text())
        raise SystemExit(f"timed out waiting for {needle!r} to {word}")

    def print_screen(self):
        x, y = self.screen.cursor.x, self.screen.cursor.y
        print(f"--- screen {self.cols}x{self.rows} · cursor {y},{x}")
        print(self.text())
        print("---")

    def runs(self, every):
        found = []
        for y in range(self.rows):
            line = self.screen.buffer[y]
            run, start, words = None, 0, ""
            for x in range(self.cols + 1):
                style = None
                if x < self.cols:
                    cell = line[x]
                    marks = []
                    if cell.reverse:
                        marks.append("rev")
                    if cell.bold:
                        marks.append("bold")
                    if cell.underscore:
                        marks.append("under")
                    if cell.bg != "default":
                        marks.append("bg=" + cell.bg)
                    if every and cell.fg != "default":
                        marks.append("fg=" + cell.fg)
                    style = ",".join(marks) or None
                if style != run:
                    if run and words.strip():
                        found.append(f"{y}:{start}-{x - 1} {run} {words.strip()!r}")
                    run, start, words = style, x, ""
                if x < self.cols:
                    words += line[x].data
        return found

    def print_attrs(self, every):
        print("--- styled runs (row:col-col style 'text')")
        self.seen = self.runs(every)
        print("\n".join(self.seen))
        print("---")

    def print_diff(self):
        """The styled runs that changed since the last attrs or diff step: where focus went."""
        now = self.runs(False)
        before = getattr(self, "seen", [])
        print(f"--- styled runs changed · cursor {self.screen.cursor.y},{self.screen.cursor.x}")
        print("\n".join([f"- {r}" for r in before if r not in now] + [f"+ {r}" for r in now if r not in before]))
        print("---")
        self.seen = now

    def step(self, step):
        name, _, value = step.partition(":")
        if name == "key":
            key, _, times = value.partition("*")
            for _ in range(int(times or 1)):
                self.send(key_bytes(key))
                self.pump()
        elif name == "type":
            self.send(value)
            self.pump()
        elif name == "wait":
            self.until(value, True, self.wait)
        elif name == "gone":
            self.until(value, False, self.wait)
        elif name == "sleep":
            end = time.time() + float(value)
            while time.time() < end:
                self.pump(quiet=end - time.time(), limit=end - time.time())
        elif name == "screen":
            self.print_screen()
        elif name == "rows":
            first, _, last = value.partition("-")
            lines = self.text().split("\n")
            print("\n".join(lines[int(first) : int(last or first) + 1]))
        elif name == "attrs":
            self.print_attrs(value == "all")
        elif name == "diff":
            self.print_diff()
        elif name == "fixture":
            for sub in ["key:ctrl+k", "type:Data: fixture", "sleep:0.5", "key:enter", "sleep:1"]:
                self.step(sub)
        else:
            raise SystemExit(f"unknown step: {step}")

    def close(self):
        try:
            os.killpg(self.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
        for _ in range(20):
            if os.waitpid(self.pid, os.WNOHANG) != (0, 0):
                return
            time.sleep(0.1)
        os.killpg(self.pid, signal.SIGKILL)


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--size", default="140x44", help="COLSxROWS")
    parser.add_argument("--port", type=int, default=8797)
    parser.add_argument("--steps", help="a file of steps, one per line, run before the arguments")
    parser.add_argument("--start", type=float, default=900, help="seconds allowed for build and start")
    parser.add_argument("--wait", type=float, default=20, help="seconds a wait/gone step allows")
    parser.add_argument(
        "--colors", default="truecolor", choices=["truecolor", "256", "16", "mono"],
        help="colour depth NSH draws at (NSH_TUI_COLORS)",
    )
    parser.add_argument(
        "--background", default="1e1e1e",
        help="terminal background RRGGBB the colour queries are answered with, or none",
    )
    parser.add_argument("steps_args", nargs="*", metavar="STEP")
    args = parser.parse_args()
    steps = []
    if args.steps:
        with open(args.steps) as file:
            steps = [l.strip() for l in file if l.strip() and not l.lstrip().startswith("#")]
    steps += args.steps_args
    ensure_pyte()
    walk = Walk(args)
    try:
        # Started: the top bar names the app. The sidebar's "Home" is not a sign at every size:
        # a narrow grid folds the sidebar to its keys.
        walk.until("Corral", True, args.start)
        walk.pump(quiet=1.0, limit=10)
        for step in steps:
            print(f"> {step}")
            walk.step(step)
        if not steps or steps[-1] not in ("screen", "attrs", "attrs:all"):
            walk.print_screen()
    finally:
        walk.close()


if __name__ == "__main__":
    main()
