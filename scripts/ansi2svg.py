#!/usr/bin/env python3
"""Render ANSI terminal text (stdin) as a standalone SVG (stdout).

Usage: ansi2svg.py [--title TEXT] < capture.ansi > capture.svg
"""
import re
import sys
import unicodedata
from xml.sax.saxutils import escape

CELL_W, CELL_H, PAD = 8.4, 18, 14
FG, BG = "#d4d4d4", "#1b1d23"
BASE16 = [
    "#2e3436", "#cc0000", "#4e9a06", "#c4a000", "#3465a4", "#75507b", "#06989a", "#d3d7cf",
    "#555753", "#ef2929", "#8ae234", "#fce94f", "#729fcf", "#ad7fa8", "#34e2e2", "#eeeeec",
]
SGR = re.compile(r"\x1b\[([0-9;:]*)m")
OTHER_ESC = re.compile(r"\x1b(\[[0-9;:?]*[A-Za-z]|\][^\x07\x1b]*(\x07|\x1b\\)|[()][A-Za-z0-9])")


def xterm256(n):
    if n < 16:
        return BASE16[n]
    if n >= 232:
        v = 8 + (n - 232) * 10
        return f"#{v:02x}{v:02x}{v:02x}"
    n -= 16
    steps = [0, 95, 135, 175, 215, 255]
    return "#%02x%02x%02x" % (steps[n // 36], steps[(n // 6) % 6], steps[n % 6])


def width(ch):
    if unicodedata.combining(ch):
        return 0
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1


def apply_sgr(state, params):
    fg, bg, bold, dim, rev = state
    p = [int(x) if x else 0 for x in re.split(r"[;:]", params)] if params else [0]
    i = 0
    while i < len(p):
        c = p[i]
        if c == 0:
            fg, bg, bold, dim, rev = None, None, False, False, False
        elif c == 1:
            bold = True
        elif c == 2:
            dim = True
        elif c == 22:
            bold = dim = False
        elif c == 7:
            rev = True
        elif c == 27:
            rev = False
        elif 30 <= c <= 37:
            fg = BASE16[c - 30]
        elif 90 <= c <= 97:
            fg = BASE16[c - 90 + 8]
        elif 40 <= c <= 47:
            bg = BASE16[c - 40]
        elif 100 <= c <= 107:
            bg = BASE16[c - 100 + 8]
        elif c == 39:
            fg = None
        elif c == 49:
            bg = None
        elif c in (38, 48) and i + 1 < len(p):
            if p[i + 1] == 5 and i + 2 < len(p):
                col, i = xterm256(p[i + 2]), i + 2
            elif p[i + 1] == 2 and i + 4 < len(p):
                col, i = "#%02x%02x%02x" % tuple(p[i + 2:i + 5]), i + 4
            else:
                col = None
            if col:
                if c == 38:
                    fg = col
                else:
                    bg = col
        i += 1
    return (fg, bg, bold, dim, rev)


def parse(text):
    rows = []
    state = (None, None, False, False, False)
    for line in text.replace("\r", "").split("\n"):
        cells, pos = [], 0
        while pos < len(line):
            m = SGR.match(line, pos)
            if m:
                state = apply_sgr(state, m.group(1))
                pos = m.end()
                continue
            m = OTHER_ESC.match(line, pos)
            if m:
                pos = m.end()
                continue
            ch = line[pos]
            pos += 1
            w = width(ch)
            if w:
                cells.append((ch, w, state))
        rows.append(cells)
    while rows and not rows[-1]:
        rows.pop()
    return rows


def resolve(state):
    fg, bg, bold, dim, rev = state
    f, b = fg or FG, bg
    if rev:
        f, b = (b or BG), (fg or FG)
    return f, b, bold, dim


def render(rows, title):
    cols = max((sum(w for _, w, _ in r) for r in rows), default=1)
    width_px = cols * CELL_W + PAD * 2
    height_px = len(rows) * CELL_H + PAD * 2
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width_px:.0f}" height="{height_px:.0f}" '
        f'viewBox="0 0 {width_px:.0f} {height_px:.0f}" role="img" aria-label="{escape(title, {chr(34): "&quot;"})}">',
        f'<title>{escape(title)}</title>',
        f'<rect width="100%" height="100%" rx="8" fill="{BG}"/>',
        '<g font-family="DejaVu Sans Mono, Menlo, Consolas, monospace" font-size="14" xml:space="preserve">',
    ]
    for y, row in enumerate(rows):
        runs, x = [], 0
        for ch, w, st in row:
            key = resolve(st)
            if runs and runs[-1][0] == key and runs[-1][1] + runs[-1][3] == x:
                runs[-1][2].append(ch)
                runs[-1][3] += w
            else:
                runs.append([key, x, [ch], w])
            x += w
        for key, start, chars, span in runs:
            f, b, bold, dim = key
            px = PAD + start * CELL_W
            py = PAD + y * CELL_H
            if b:
                out.append(f'<rect x="{px:.1f}" y="{py:.1f}" width="{span * CELL_W:.1f}" height="{CELL_H}" fill="{b}"/>')
            off = 0
            for ch in chars:
                if ch != " ":
                    attrs = f' fill="{f}"' + (' font-weight="bold"' if bold else "") + (' opacity="0.6"' if dim else "")
                    out.append(f'<text x="{px + off * CELL_W:.1f}" y="{py + CELL_H - 5:.1f}"{attrs}>{escape(ch)}</text>')
                off += width(ch)
    out.append("</g></svg>")
    return "\n".join(out) + "\n"


def main():
    title = "corral terminal screenshot"
    if len(sys.argv) > 2 and sys.argv[1] == "--title":
        title = sys.argv[2]
    sys.stdout.write(render(parse(sys.stdin.read()), title))


if __name__ == "__main__":
    main()
