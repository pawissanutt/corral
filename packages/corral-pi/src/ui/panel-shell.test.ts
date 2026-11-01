import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stripTerminalSequences, Text, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { NativePanels } from "./panels.ts";
import { anchoredRow } from "./reading-anchor.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";

const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 3 };
const snapshot: PanelSnapshot = {
  bound: true, model: "openai-codex/example", effort: "high", streaming: false,
  context: { tokens: 1234, contextWindow: 10000, percent: 12.34 }, tokens: 5678,
  tools: ["read", "bash"], contextFiles: ["/repo/AGENTS.md", "/repo/界面/AGENTS.md", "/repo/👩‍💻/notes.md"], skills: ["corral"],
  ledger: null, ledgerStale: false, jobs: [{ job: "job-1", state: "running" }, { job: "job-2", state: "exited 0" }], auxiliary: null, steering: 0, followUps: 0,
};
const work = mkdtempSync(join(tmpdir(), "corral-panel-shell-unit-"));
afterAll(() => rmSync(work, { recursive: true, force: true }));
const KEY = { f6: "\x1b[17~", esc: "\x1b", down: "\x1b[B", pageUp: "\x1b[5~", pageDown: "\x1b[6~", bottom: "\x1b[1;5F", top: "\x1b[1;5H", ctrlB: "\x02" };

function panels(columns: number, rows: number) {
  const native = new NativePanels({ identity, cwd: work, snapshot: () => snapshot });
  const physical = new RecordingTerminal(); physical.resize(columns, rows);
  native.wrap(physical).start(() => {}, () => {});
  return { native, physical };
}
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
function pagerOf(line: string): { first: number; total: number; from: number; to: number } | undefined {
  const text = stripTerminalSequences(line), match = /‹ (\d+)(?:–\d+)?\/(\d+) ›/.exec(text);
  if (!match) return undefined;
  const from = visibleWidth(text.slice(0, match.index));
  return { first: Number(match[1]), total: Number(match[2]), from, to: from + visibleWidth(match[0]) };
}
const click = (x: number, y: number): TuiMouseEvent => ({ type: "click", button: "left", x, y, screenX: x, screenY: y, width: 80, height: 1, shift: false, alt: false, ctrl: false });

test("viewport scrolling and jump-to-live keys pass through without taking focus from the inspector", () => {
  for (const [columns, rows] of [[52, 24], [80, 34], [140, 46]] as const) {
    const { native } = panels(columns, rows);
    native.handleInput(KEY.f6);
    expect(native.focused).toBeTrue();
    for (const key of [KEY.pageUp, KEY.pageDown, KEY.bottom, KEY.top]) {
      expect(native.handleInput(key)).toBeFalse();
      expect(native.focused).toBeTrue();
    }
    expect(native.handleInput("x")).toBeFalse();
    expect(native.focused).toBeFalse(); // Typing still returns to the real editor.
    native.dispose();
  }
});

test("short narrow inspector pages every item; keyboard and pointer move it identically", () => {
  for (const [columns, rows] of [[52, 14], [80, 14]] as const) {
    const { native } = panels(columns, rows);
    native.handleInput(KEY.f6);
    expect(plain(native.renderInline(columns, rows))).toHaveLength(2);
    const start = pagerOf(native.renderInline(columns, rows)[0] ?? "");
    if (!start) throw new Error("pager missing at " + columns);
    expect(start.first).toBe(1);
    const seen = new Set<string>();
    for (let item = 1; item <= start.total; item++) {
      const lines = native.renderInline(columns, rows);
      for (const line of lines) expect(visibleWidth(line)).toBe(columns);
      expect(pagerOf(lines[0] ?? "")?.first).toBe(item);
      seen.add(plain(lines)[1]?.trim() ?? "");
      if (item % 2) native.handleInput(KEY.down);
      else expect(native.inlineMouse(click(start.to - 1, 0))).toEqual({ handled: true });
    }
    expect(seen.size).toBe(start.total);
    expect([...seen].join("\n")).toContain("界面");
    expect(native.inlineMouse(click(start.from, 0))).toEqual({ handled: true });
    expect(pagerOf(native.renderInline(columns, rows)[0] ?? "")?.first).toBe(start.total - 1);
    native.dispose();
  }
});

test("relocation keeps the first visible inspector item, whether headings are shown or not", () => {
  const { native, physical } = panels(80, 9);
  native.handleInput(KEY.f6); native.handleInput("\x1b[C"); native.handleInput("\x1b[C");
  expect(native.tab).toBe("Agents");
  native.handleInput(KEY.down);
  const item = plain(native.renderInline(80, 9))[1]?.trim() ?? "";
  expect(item).toContain("g3");
  physical.resize(140, 9);
  const rail = plain(native.renderSidebar(34, 9)).map(line => line.replace(/^│\s?|\s?│$/g, "").trim());
  expect(rail[3]).toBe(item);
  physical.resize(80, 9);
  expect(plain(native.renderInline(80, 9))[1]?.trim()).toBe(item);
  native.dispose();
});

test("a tall rail pages overflowing metadata through its footer pager", () => {
  const { native, physical } = panels(140, 9);
  native.handleInput(KEY.f6);
  const lines = native.renderSidebar(34, 9);
  for (const line of lines) expect(visibleWidth(line)).toBe(34);
  const footer = lines[7] ?? "", pager = pagerOf(footer);
  if (!pager) throw new Error("rail pager missing:\n" + plain(lines).join("\n"));
  const before = plain(lines)[3];
  physical.send(`\x1b[<0;${106 + pager.to};8M`); physical.send(`\x1b[<0;${106 + pager.to};8m`);
  expect(pagerOf(native.renderSidebar(34, 9)[7] ?? "")?.first).toBe(pager.first + 1);
  expect(plain(native.renderSidebar(34, 9))[3]).not.toBe(before);
  native.dispose();
});

test("a non-Git launch directory reads as an explicit, readable unavailable state", async () => {
  const { native } = panels(140, 46);
  await native.loadFiles();
  const rail = plain(native.renderSidebar(34, 46)).join("\n");
  expect(rail).toContain("Not a Git repository");
  expect(rail).not.toContain("Command failed");
  native.dispose();
});

test("re-wrapped transcripts keep the reader on the same block and depth, including wide graphemes", () => {
  const paragraphs = Array.from({ length: 12 }, (_, i) => `B${i} ` + "界面 👩‍💻 word ".repeat(12 + i));
  const wrap = (width: number) => paragraphs.flatMap(text => [...new Text(text, 0, 0).render(width), ""]);
  const wide = wrap(105), narrow = wrap(52), minimum = wrap(34);
  for (const top of [0, 3, wide.findIndex(line => line.includes("B7")) + 1, wide.length - 2]) {
    const block = (lines: readonly string[], row: number) => lines.slice(0, row + 1).filter(line => /B\d+/.test(line)).length;
    for (const target of [narrow, minimum]) {
      const mapped = anchoredRow(wide, top, target);
      expect(block(target, mapped)).toBe(block(wide, top));
      expect(anchoredRow(target, mapped, wide)).toBeLessThanOrEqual(top);
    }
  }
  expect(anchoredRow(["a", "", "b"], 2, ["a", "b"])).toBe(1);
});

test("the main view bar keeps its focused hints and click targets at the minimum width", () => {
  const { native } = panels(52, 24);
  const root = native.workbench.mount(new Text("transcript", 0, 0));
  native.workbench.setView("changes");
  const bar = stripTerminalSequences(root.render(52)[0] ?? "");
  expect(visibleWidth(bar)).toBeLessThanOrEqual(52);
  expect(bar).toContain("Esc");
  expect(bar).toContain("Enter");
  expect(bar).not.toContain("…");
  native.dispose();
});
