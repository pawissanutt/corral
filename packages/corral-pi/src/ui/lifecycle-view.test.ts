import { test, expect } from "bun:test";
import { LifecycleView, type LifecycleSnapshot } from "./lifecycle-view.ts";
import type { InspectorSnapshot } from "./inspector.ts";
import { visibleWidth } from "@earendil-works/pi-tui";
const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
export function sample(): LifecycleSnapshot {
  return { version: 1, task: "T-001", run: "R1", graph_gap: true, event_gap: true, end: 500000,
    nodes: [{ id: "T-001", title: "Blocked root", status: "running", depends_on: ["T-002", "T-003"], parent: "T-002", missing: false },
      { id: "T-002", title: "Prerequisite", status: "backlog", depends_on: [], parent: null, missing: false },
      { id: "T-003", title: "Missing", status: null, depends_on: [], parent: null, missing: true }],
    events: Array.from({ length: 80 }, (_, index) => ({ source: "store:fixture", sequence: 100 + index, event: { ts: "2026-01-01T00:00:00Z", task: "T-001", kind: "blocked", detail: { run: "R1", index } } })),
  };
}
test("bounded cached projection, source order, graph/list input and canonical details", async () => {
  let reads = 0;
  const view = new LifecycleView(identity, async () => { reads++; return sample(); }, () => [{ id: "child", parent: "host", session: "session-child", kind: "auxiliary", state: "done", model: null, transcript: false }], () => true, () => {}, () => true);
  await view.refresh();
  for (let i = 0; i < 100; i++) { view.render(140); view.render(52); }
  expect(reads).toBe(1);
  expect(view.render(140).join("\n")).toContain("─depends_on→");
  expect(view.render(140).join("\n")).toContain("unmet prerequisite");
  expect(view.render(52).join("\n")).toContain("Dependency list");
  expect(view.render(140).join("\n")).toContain("agent-parent→ host");
  view.handleInput("j"); expect(view.render(140).join("\n")).toContain("Inspect canonical task T-002");
  view.handleInput("j"); expect(view.render(140).join("\n")).toContain("Inspect canonical task T-003 · unavailable");
  view.mode = "timeline";
  for (let i = 0; i < 77; i++) view.handleInput("j");
  const rows = view.render(52);
  expect(rows.join("\n")).toContain("store:fixture@179");
  expect(rows.join("\n")).toContain('"index":79');
  expect(rows.join("\n")).toContain("GAP:");
  expect(rows.every(row => visibleWidth(row) <= 52)).toBe(true);
  expect(reads).toBe(1); view.dispose();
});
test("read failures retain historical rows, foreign identities rejected and actions gated", async () => {
  let fail = false;
  const view = new LifecycleView(identity, async () => { if (fail) throw new Error("disconnected"); return sample(); }, () => undefined, () => true, () => {}, () => true);
  await view.refresh(); fail = true; await view.refresh();
  expect(view.render(140).join("\n")).toContain("STALE / disconnected");
  expect(view.render(140).join("\n")).toContain("Blocked root");
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
  expect(view.render(140).join("\n")).toContain("session-one / g1 / #521");
  view.handleInput("j");
  const text = view.render(52).join("\n");
  expect(text).toContain("GAP: 88 host events missing");
  expect(text).toContain('"seq":522');
  expect(text).toContain("#521"); expect(text).toContain("#522");
  expect(text.indexOf("#521") < text.lastIndexOf("#522")).toBe(true);
  expect(text.replace(/\s+/g, " ")).toContain("NOT canonical task settlement");
});
