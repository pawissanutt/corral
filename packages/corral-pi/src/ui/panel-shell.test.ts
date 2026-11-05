import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stripTerminalSequences, Text, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
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

test("a non-Git launch directory reads as an explicit, readable unavailable state", async () => {
  const { native } = panels(140, 46);
  await native.loadFiles();
  expect(native.workbench.review.error).toBe("Not a Git repository");
  const changes = native.workbench.files().map(row => row.text).join("\n");
  expect(changes).toContain("Not a Git repository"); expect(changes).not.toContain("Command failed");
  expect(plain(native.renderSidebar(34, 46)).find(line => line.includes("Changes"))).toContain("!");
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
  for (const width of [34, 52, 104]) {
    const bar = stripTerminalSequences(renderLayoutFrame(root, width, 24, () => {}).lines[0] ?? "");
    expect(visibleWidth(bar)).toBeLessThanOrEqual(width);
    expect(bar).toContain("Esc");
    expect(bar).toContain("Enter");
    expect(bar).not.toContain("…");
    const click = (row: string, y: number, label: string): TuiMouseEvent => ({ type: "click", button: "left", x: row.indexOf(label) + 1, y,
      screenX: row.indexOf(label) + 1, screenY: y, width, height: 24, shift: false, alt: false, ctrl: false, clickCount: 1 });
    root.handleMouse?.(click(bar, 0, width < 64 ? " U " : " Usage "));
    expect(native.workbench.view).toBe("usage");
    root.handleMouse?.(click(bar, 0, width < 64 ? " A " : " Activity "));
    expect(native.workbench.view).toBe("activity");
    // Graph and Timeline are Activity sub-tabs on the second row.
    const sub = stripTerminalSequences(renderLayoutFrame(root, width, 24, () => {}).lines[1] ?? "");
    expect(visibleWidth(sub)).toBeLessThanOrEqual(width);
    root.handleMouse?.(click(sub, 1, " Graph "));
    expect(native.workbench.view).toBe("graph");
    root.handleMouse?.(click(sub, 1, " Timeline "));
    expect(native.workbench.view).toBe("timeline");
    native.workbench.setView("changes");
  }
  native.dispose();
});
