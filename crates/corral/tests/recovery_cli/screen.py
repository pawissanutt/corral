"""Small VT cell grid for actual PTY captures; SVG colors reuse the repository renderer."""
import codecs
from pathlib import Path
import re
import sys
import unicodedata

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "scripts"))
import ansi2svg


class Screen:
    def __init__(self, cols, rows):
        self.cols, self.rows = cols, rows
        self.cells = [[" "] * cols for _ in range(rows)]
        self.style = (None, None, False, False, False)
        self.styles = [[self.style] * cols for _ in range(rows)]
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
                    self.styles.pop(0)
                    self.styles.append([self.style] * self.cols)
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
                self.styles[self.y][self.x] = self.style
                width = 2 if unicodedata.east_asian_width(c) in "WF" else 1
                if width == 2 and self.x + 1 < self.cols:
                    self.cells[self.y][self.x + 1] = ""
                    self.styles[self.y][self.x + 1] = self.style
                self.x += width

    def csi(self, args, op):
        private = args.startswith("?")
        values = [int(x or 0) for x in args.lstrip("?><=").split(";")]
        n = values[0] or 1
        if op == "m":
            self.style = ansi2svg.apply_sgr(self.style, args)
        elif op in "Hf":
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
                self.styles = [[self.style] * self.cols for _ in range(self.rows)]
            elif values[0] == 0:
                self.cells[self.y][self.x:] = [" "] * (self.cols - self.x)
                self.styles[self.y][self.x:] = [self.style] * (self.cols - self.x)
                for y in range(self.y + 1, self.rows):
                    self.cells[y] = [" "] * self.cols
                    self.styles[y] = [self.style] * self.cols
        elif op == "K":
            a, b = (0, self.cols) if values[0] == 2 else (0, self.x + 1) if values[0] == 1 else (self.x, self.cols)
            self.cells[self.y][a:b] = [" "] * (b - a)
            self.styles[self.y][a:b] = [self.style] * (b - a)
        elif private and op == "h" and 1049 in values:
            self.cells = [[" "] * self.cols for _ in range(self.rows)]
            self.styles = [[self.style] * self.cols for _ in range(self.rows)]
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
