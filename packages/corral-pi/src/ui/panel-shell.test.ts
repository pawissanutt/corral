import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stripTerminalSequences, Text, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { getLayoutBoxesAt, renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
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

function panels(columns: number, rows: number) {
  const native = new NativePanels({ identity, cwd: work, snapshot: () => snapshot });
  const physical = new RecordingTerminal(); physical.resize(columns, rows);
  native.wrap(physical).start(() => {}, () => {});
  return { native, physical };
}
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
/** A plain left click at frame cell (x, y), delivered the way the layout routes it. */
function press(root: Component, width: number, height: number, x: number, y: number): void {
  const frame = renderLayoutFrame(root, width, height, () => {});
  const event: TuiMouseEvent = { type: "click", button: "left", x, y, screenX: x, screenY: y, width, height, shift: false, alt: false, ctrl: false, clickCount: 1 };
  for (const box of getLayoutBoxesAt(frame, x, y)) {
    if (box.component.handleMouse?.({ ...event, x: x - box.rect.x, y: y - box.rect.y, width: box.rect.width, height: box.rect.height })) return;
  }
}

test("a non-Git launch directory reads as an explicit, readable unavailable state", async () => {
  const { native } = panels(140, 46);
  await native.loadFiles();
  expect(native.workbench.review.error).toBe("Not a Git repository");
  native.sidebar.tab = "files";
  const files = plain(native.renderSidebar(34, 46)).join("\n");
  expect(files).toContain("Git unavailable"); expect(files).toContain("Not a Git repository"); expect(files).not.toContain("Command failed");
  const root = native.workbench.mount(new Text("transcript", 0, 0));
  const tabs = plain(renderLayoutFrame(root, 105, 46, () => {}).lines).find(line => line.includes("F7"));
  expect(tabs).toMatch(/Changes\s+!/);
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

test("the tab row opens Agents and Changes by click at the minimum width, with no sub-row and no hint text", () => {
  const { native } = panels(52, 24);
  const root = native.workbench.mount(new Text("transcript", 0, 0));
  native.workbench.setView("changes");
  for (const width of [34, 52, 104]) {
    const lines = plain(renderLayoutFrame(root, width, 24, () => {}).lines);
    const y = lines.findIndex(line => line.includes("F9"));
    const row = lines[y] ?? "";
    expect(y).toBeGreaterThanOrEqual(0);
    expect(visibleWidth(row)).toBeLessThanOrEqual(width);
    expect(row).not.toMatch(/\bEsc\b|\bEnter\b|esc chat/);
    expect(lines.join("\n")).not.toMatch(/\bGraph\b|\bTimeline\b|\bActivity\b|\bUsage\b/);
    // Only the open tab keeps its label at the narrowest widths, so positions are read after each click.
    const at = (key: string) => {
      const current = plain(renderLayoutFrame(root, width, 24, () => {}).lines)[y] ?? "";
      return visibleWidth(current.slice(0, current.indexOf(key))) + 1;
    };
    press(root, width, 24, at("F8"), y);
    expect(native.workbench.view).toBe("agents");
    press(root, width, 24, at("F7"), y);
    expect(native.workbench.view).toBe("changes");
  }
  native.dispose();
});
