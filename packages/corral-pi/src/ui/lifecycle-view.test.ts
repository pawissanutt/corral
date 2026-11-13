import { test, expect } from "bun:test";
import { GRAPH_MIN, LifecycleView, type LifecycleSnapshot } from "./lifecycle-view.ts";
import { VIEW_INSET } from "./frame.ts";
import type { InspectorSnapshot } from "./inspector.ts";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
export function sample(): LifecycleSnapshot {
  return { version: 1, task: "T-001", run: "R1", graph_gap: true, event_gap: true, end: 500000,
    nodes: [{ id: "T-001", title: "Blocked root", status: "running", depends_on: ["T-002", "T-003"], parent: "T-002", missing: false },
      { id: "T-002", title: "Prerequisite", status: "backlog", depends_on: [], parent: null, missing: false },
      { id: "T-003", title: "Missing", status: null, depends_on: [], parent: null, missing: true }],
    events: Array.from({ length: 80 }, (_, index) => ({ source: "store:fixture", sequence: 100 + index, event: { ts: "2026-01-01T00:00:00Z", task: "T-001", kind: "blocked", detail: { run: "R1", index, gate: { exit: 1 } } } })),
  };
}
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
/** The list row carrying the selection bar. */
const selectedRow = (lines: readonly string[]) => plain(lines).find(line => line.includes("▌")) ?? "";

test("bounded cached projection, source order, graph/list input and flattened details", async () => {
  let reads = 0;
  const view = new LifecycleView(identity, async () => { reads++; return sample(); }, () => [{ id: "child", parent: "host", session: "session-child", kind: "auxiliary", state: "done", model: null, transcript: false }], () => true, () => {}, () => true);
  await view.refresh();
  for (let i = 0; i < 100; i++) { view.render(140); view.render(52); }
  expect(reads).toBe(1);
  const wide = plain(view.render(140)).join("\n");
  expect(wide).toContain("─depends_on→");
  expect(wide).toContain("unmet prerequisite");
  expect(wide).toContain("agent-parent→ host");
  expect(plain(view.render(140))[0]?.startsWith("‹ Task")).toBe(true);
  expect(view.back.to).toBe(visibleWidth("‹ Task"));
  view.handleInput("j"); expect(selectedRow(view.render(140))).toContain("T-002");
  view.handleInput("j"); expect(selectedRow(view.render(140))).toContain("T-003");
  expect(selectedRow(view.render(140))).toContain("missing");
  view.mode = "timeline";
  for (let i = 0; i < 77; i++) view.handleInput("j");
  const rows = view.render(52), text = plain(rows).join("\n");
  expect(selectedRow(rows)).toContain("179");
  expect(text).not.toContain('{"');
  expect(text).toContain("gate.exit");
  expect(text).toContain("gap");
  expect(rows.every(row => visibleWidth(row) <= 52)).toBe(true);
  expect(reads).toBe(1); view.dispose();
});
test("the arrow form of the graph starts at GRAPH_MIN main-view columns, measured before the workbench's inset", async () => {
  const view = new LifecycleView(identity, async () => sample(), () => undefined, () => true, () => {}, () => true);
  await view.refresh();
  // The workbench renders the view VIEW_INSET cells narrower than the main view.
  expect(plain(view.render(GRAPH_MIN - VIEW_INSET)).join("\n")).toContain("─depends_on→");
  expect(plain(view.render(GRAPH_MIN - VIEW_INSET - 1)).join("\n")).not.toContain("─depends_on→");
  view.dispose();
});
test("a click selects only inside the list's columns, never through the detail pane beside it", async () => {
  const view = new LifecycleView(identity, async () => sample(), () => undefined, () => true, () => {}, () => true);
  await view.refresh();
  const lines = plain(view.render(140)), y = lines.findIndex(line => line.startsWith("  T-002"));
  const click = { type: "click" as const, button: "left" as const, x: 100, y, screenX: 100, screenY: y, width: 140, height: 40, shift: false, alt: false, ctrl: false };
  expect(y).toBeGreaterThan(0);
  expect(view.handleMouse(click)).toBeUndefined();
  expect(selectedRow(view.render(140))).toContain("T-001");
  expect(view.handleMouse({ ...click, x: 4, screenX: 4 })?.handled).toBe(true);
  expect(selectedRow(view.render(140))).toContain("T-002");
  view.dispose();
});
test("the list fills the rows it is given and keeps the selection in view", async () => {
  const view = new LifecycleView(identity, async () => sample(), () => undefined, () => true, () => {}, () => true, () => undefined, { rows: () => 14 });
  await view.refresh();
  view.mode = "timeline";
  for (const width of [138, 98]) {
    for (let i = 0; i < 79; i++) view.handleInput("j");
    const lines = view.render(width);
    expect(selectedRow(lines)).toContain("179");
    expect(plain(lines).filter(line => /^\S?\s+\d{3}\s/.test(line)).length).toBeLessThanOrEqual(13);
    for (let i = 0; i < 79; i++) view.handleInput("k");
    expect(selectedRow(view.render(width))).toContain("100");
  }
  view.dispose();
});
test("every caveat stays reachable behind the legend", async () => {
  let legend = false;
  const view = new LifecycleView(identity, async () => sample(), () => undefined, () => true, () => {}, () => true, () => undefined, { legend: () => legend });
  await view.refresh();
  view.mode = "timeline";
  expect(plain(view.render(140)).join(" ")).not.toContain("No queue, move or dispatch controls");
  legend = true;
  const shown = plain(view.render(140)).join(" ");
  expect(shown).toContain("No queue, move or dispatch controls");
  expect(shown).toContain("never merged or timestamp-sorted");
  expect(shown).toContain("GAP: bounded page");
  view.mode = "graph";
  const graph = plain(view.render(140)).join(" ");
  expect(graph).toContain("GAP: graph capped at 24");
  expect(graph).toContain("No agent parent is inferred from conversation order");
  view.dispose();
});
test("read failures retain historical rows, foreign identities rejected and actions gated", async () => {
  let fail = false;
  const view = new LifecycleView(identity, async () => { if (fail) throw new Error("disconnected"); return sample(); }, () => undefined, () => true, () => {}, () => true);
  await view.refresh(); fail = true; await view.refresh();
  const text = plain(view.render(140)).join("\n");
  expect(text).toContain("STALE");
  expect(text).toContain("disconnected");
  expect(text).toContain("Blocked root");
  expect(selectedRow(view.render(140))).toContain("T-001");
  const foreign = new LifecycleView(identity, async () => ({ ...sample(), task: "T-999" }), () => undefined, () => true, () => {}, () => false);
  await foreign.refresh(); expect(foreign.current).toBeUndefined(); expect(foreign.handleInput("r")).toBe(false);
});
test("session timeline retains source generation/sequence, gaps and matching selected details", () => {
  const host: InspectorSnapshot = { session: "session-one", boundSession: "session-one", turn: 0, turnOpen: false,
    capabilities: null, guidance: { projectText: null, skillPaths: [], promptPaths: [], prompts: [], skills: [] },
    settings: [], budget: [], receipts: [], receiptsOmitted: 0,
    events: { generation: 1, last: 600, next: 600, more: false, gap: { after: 0, resumed: 89, missing: 88, reconcile: true },
      events: Array.from({ length: 512 }, (_, i) => ({ seq: i + 89, generation: 1, at: String(1000 - i), kind: "bound" })) } };
  const view = new LifecycleView(identity, undefined, () => undefined, () => true, () => {}, () => true, () => host);
  view.mode = "timeline"; view.handleInput("h");
  expect(plain(view.render(140)).join("\n")).toContain("session-one / g1 / #521");
  view.handleInput("j");
  const rows = view.render(52), text = plain(rows).join("\n");
  expect(selectedRow(rows)).toContain("522");
  expect(text).toContain("88 missing");
  expect(text).toContain("#522");
  expect(text.indexOf("521") < text.indexOf("522")).toBe(true);
  expect(plain(view.render(140)).join(" ")).toContain("NOT canonical task settlement");
});
