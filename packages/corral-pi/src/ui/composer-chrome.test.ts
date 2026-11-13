import { expect, test } from "bun:test";
import { sliceByColumn, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { COMPOSER_INSET, composerBar, composerBottom, composerRow, composerTitle, composerTop } from "./composer-chrome.ts";
import { PANEL_BLUE, PANEL_LINE, type PanelSnapshot } from "./panels-view.ts";

const snapshot: PanelSnapshot = {
  bound: true, model: "anthropic/claude-sonnet-4-5", effort: "high", effortAvailable: true, streaming: true,
  context: { tokens: 1234, contextWindow: 10000, percent: 12.34 }, tokens: 5678,
  tools: ["read"], contextFiles: [], skills: [], ledger: null, ledgerStale: false,
  jobs: null, auxiliary: null, steering: 1, followUps: 2,
};
const KEY_HINT = /enter|⏎|\bF\d+\b|alt\+|ctrl\+|shift\+|\besc\b|follow-up|\bsend\b|\bsteer\b/i;
const plain = (line: string) => stripTerminalSequences(line);
const WIDTHS = [30, 46, 52, 61, 80, 92, 105, 140, 185];

test("the composer title and bar carry no key hints at any width, streaming or idle", () => {
  const inputs = [undefined, { key: "terminal:1", disposition: "queued" as const, policy: "followUp" },
    { key: "terminal:2", disposition: "pending" as const, reason: "preflight_pending" }];
  for (const width of [30, 80, 140]) {
    for (const streaming of [true, false]) {
      for (const input of inputs) {
        const state = { ...snapshot, streaming, input };
        expect(plain(composerTitle(state))).not.toMatch(KEY_HINT);
        for (const hidden of [0, 4]) {
          const bar = composerBar(state, width, hidden, false).line;
          expect(plain(bar)).not.toMatch(KEY_HINT);
          expect(visibleWidth(bar)).toBe(width);
        }
      }
    }
  }
  expect(plain(composerBar(snapshot, 80, 4, false).line)).toContain("↓ 4 more");
});

test("the box: title set into a rounded top border, sides on every row, bar inside, one cell in from each edge", () => {
  for (const width of WIDTHS) {
    const top = composerTop(composerTitle(snapshot, { label: "parent", color: PANEL_BLUE }), width, false);
    const bar = composerBar(snapshot, width, 0, false).line, bottom = composerBottom(width, false);
    const row = composerRow(" ".repeat(5) + "draft" + " ".repeat(width - 10), 5, width, false, true);
    for (const line of [top, bar, bottom, row]) expect(visibleWidth(line)).toBe(width);
    expect(plain(top).startsWith(" ╭─ Message → parent ─")).toBeTrue();
    expect(plain(top).endsWith("╮ ")).toBeTrue();
    expect(plain(bottom)).toMatch(/^ ╰─+╯ $/);
    expect(plain(bar).startsWith(" │ claude-sonnet")).toBeTrue();
    expect(plain(bar).endsWith("│ ")).toBeTrue();
    expect(plain(row)).toMatch(/^ │ › draft +│ $/);
  }
  // The draft's own continuation rows carry no prompt, and the prompt cell keeps the editor's columns.
  expect(plain(composerRow(" ".repeat(5) + "more" + " ".repeat(71), 5, 80, false, false))).toMatch(/^ │   more +│ $/);
  expect(plain(composerTop("Message", 80, false, "↑ 3 more"))).toContain("↑ 3 more");
  expect(plain(composerBottom(80, false, "↓ 2 more"))).toContain("↓ 2 more");
});

test("the border is accent while the composer has the focus, the line colour otherwise", () => {
  expect(composerTop("Message", 80, true)).toContain(PANEL_BLUE + "╭");
  expect(composerTop("Message", 80, false)).toContain(PANEL_LINE + "╭");
  expect(composerBottom(80, true)).toContain(PANEL_BLUE + "╰");
  expect(composerBar(snapshot, 80, 0, true).line).toContain(PANEL_BLUE + "│");
  expect(composerRow(" ".repeat(5) + "x" + " ".repeat(74), 5, 80, true, true)).toContain(PANEL_BLUE + "│");
});

test("model and effort hit ranges cover exactly the cells that show them", () => {
  for (const width of [30, 52, 80, 140]) {
    const bar = composerBar(snapshot, width, 0, false);
    const cells = (range: { from: number; to: number }) => plain(sliceByColumn(bar.line, range.from, range.to - range.from));
    expect(bar.model).toBeDefined();
    expect(bar.model!.from).toBe(COMPOSER_INSET);
    expect(cells(bar.model!)).toMatch(/^claude-sonnet/);
    // The model range ends where the effort text begins, so a click never lands on the wrong control.
    expect(plain(sliceByColumn(bar.line, bar.model!.to, 3))).toBe(" · ");
    if (width >= 52) {
      expect(cells(bar.model!)).toBe("claude-sonnet-4-5 ▾");
      expect(cells(bar.effort!)).toBe(" · high ↻");
    }
    if (bar.effort) expect(bar.effort.to).toBeLessThanOrEqual(width - COMPOSER_INSET);
  }
  const fixed = composerBar({ ...snapshot, effortAvailable: false }, 80, 0, false);
  expect(fixed.effort).toBeUndefined();
  expect(composerBar({ ...snapshot, model: null }, 80, 0, false).model).toBeUndefined();
});

test("a refused input shows its sanitized reason; started and handled input add nothing", () => {
  const refused = composerTitle({ ...snapshot, input: { key: "terminal:3", disposition: "rejected", reason: "queue full\x1b[2J" } });
  expect(refused).toContain("queue full");
  expect(refused).not.toContain("\x1b[2J");
  const base = composerTitle(null);
  expect(composerTitle({ ...snapshot, input: { key: "terminal:4", disposition: "started" } })).toBe(base);
  expect(composerTitle({ ...snapshot, input: { key: "terminal:5", disposition: "handled" } })).toBe(base);
  expect(composerTitle({ ...snapshot, input: { key: "terminal:6", disposition: "queued" } })).not.toBe(base);
  expect(composerTitle({ ...snapshot, input: { key: "terminal:7", disposition: "pending", reason: "preflight_pending" } })).not.toContain("preflight");
  expect(plain(composerTitle(snapshot, { label: "host-slots\x07", color: "\x1b[35m" }))).toContain("host-slots");
});
