import { beforeAll, expect, test } from "bun:test";
import { ToolExecutionComponent, TranscriptContainer, initTheme } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, type TUI, type TuiMouseEvent } from "@earendil-works/pi-tui";

import { groupingFor } from "./tool-group.ts";
import { presenterFor, type ToolRenderDeps } from "./tool-rendering.ts";
import { rowRenderers } from "./tool-row.ts";

beforeAll(() => initTheme("dark", false));

const deps: ToolRenderDeps = { cwd: "/work", ticker: () => undefined, job: () => undefined, code: () => undefined, ledger: () => undefined, corralSpecs: [] };
const tui = { requestRender() {} } as unknown as TUI;

/** A `read` row of `path`: settled ok, failed, or still running. */
function read(path: string, state: "ok" | "failed" | "running", grouped = true): ToolExecutionComponent {
  const renderers = rowRenderers(presenterFor("read", deps), deps, "read", grouped ? groupingFor("read") : undefined);
  const row = new ToolExecutionComponent("read", `call-${path}`, { path }, { outputPad: 1 }, renderers, tui, "/work");
  row.setArgsComplete();
  row.markExecutionStarted();
  if (state === "ok") row.updateResult({ content: [{ type: "text", text: "one\ntwo" }], isError: false, durationMs: 400 });
  if (state === "failed") row.updateResult({ content: [{ type: "text", text: "ENOENT: no such file or directory" }], isError: true });
  return row;
}
function transcript(...rows: ToolExecutionComponent[]): TranscriptContainer {
  const container = new TranscriptContainer();
  for (const row of rows) container.addChild(row);
  return container;
}
function plain(container: TranscriptContainer, width = 100): string[] {
  return container.render(width).map(line => stripTerminalSequences(line).trimEnd()).filter(line => line !== "");
}
function click(y: number, height: number): TuiMouseEvent {
  return { type: "click", button: "left", x: 6, y, screenX: 6, screenY: y, width: 100, height, shift: false, alt: false, ctrl: false, clickCount: 1 };
}

test("three successful reads fold to one row naming the count", () => {
  const rows = plain(transcript(read("a.ts", "ok"), read("b.ts", "ok"), read("c.ts", "ok")));
  expect(rows).toHaveLength(1);
  expect(rows[0]).toContain("✓ read");
  expect(rows[0]).toContain("×3");
  expect(rows[0]).toContain("a.ts");
});

test("a failure in the middle splits the run and stays visible", () => {
  const rows = plain(transcript(read("a.ts", "ok"), read("b.ts", "ok"), read("c.ts", "failed"), read("d.ts", "ok"), read("e.ts", "ok")));
  const failed = rows.findIndex(line => line.includes("✗"));
  expect(failed).toBeGreaterThan(0);
  expect(rows[failed]).toContain("c.ts");
  expect(rows[failed]).toContain("not found");
  expect(rows.filter(line => line.includes("×2"))).toHaveLength(2);
  expect(rows.findIndex(line => line.includes("×2"))).toBe(failed - 1);
});

test("a click on the group row shows its rows; collapsing all folds them again", () => {
  const container = transcript(read("a.ts", "ok"), read("b.ts", "ok"), read("c.ts", "ok"));
  const lines = container.render(100);
  const at = lines.findIndex(line => stripTerminalSequences(line).includes("×3"));
  expect(container.handleMouse(click(at, lines.length))?.handled).toBeTrue();
  const open = plain(container);
  expect(open).toHaveLength(3);
  expect(open.some(line => line.includes("×"))).toBeFalse();
  expect(open[1]).toContain("b.ts");
  // A member row then behaves as before: a click expands it alone.
  const shown = container.render(100);
  const second = shown.findIndex(line => stripTerminalSequences(line).includes("b.ts"));
  container.handleMouse(click(second, shown.length));
  expect(plain(container).length).toBeGreaterThan(3);
  for (const row of container.children) if (row instanceof ToolExecutionComponent) row.setExpanded(false);
  expect(plain(container)).toHaveLength(1);
});

test("expanding every row shows every member", () => {
  const container = transcript(read("a.ts", "ok"), read("b.ts", "ok"), read("c.ts", "ok"));
  for (const row of container.children) if (row instanceof ToolExecutionComponent) row.setExpanded(true);
  const rows = plain(container);
  expect(rows.some(line => line.includes("×"))).toBeFalse();
  expect(rows.filter(line => line.includes("✓ read"))).toHaveLength(3);
});

test("a running row is never inside a group: it draws under the group of the settled rows above it", () => {
  const running = read("c.ts", "running");
  const container = transcript(read("a.ts", "ok"), read("b.ts", "ok"), running);
  const rows = plain(container);
  expect(rows).toHaveLength(2);
  expect(rows[0]).toContain("×2");
  expect(rows[0]).not.toContain("c.ts");
  expect(rows[1]).toContain("c.ts");
  expect(rows[1]).not.toContain("×");
  // Settled, it joins the group above.
  running.updateResult({ content: [{ type: "text", text: "one" }], isError: false });
  expect(plain(container)).toHaveLength(1);
  expect(plain(container)[0]).toContain("×3");
});

test("rows without a grouping never fold", () => {
  const rows = plain(transcript(read("a.ts", "ok", false), read("b.ts", "ok", false), read("c.ts", "ok", false)));
  expect(rows).toHaveLength(3);
  expect(rows.some(line => line.includes("×"))).toBeFalse();
});
