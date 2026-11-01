import { expect, test } from "bun:test";
import type { AgentToolResult, ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../bridge/events.ts";
import { InspectorActivity, inspectorActivity, inspectorContext, type InspectorSnapshot } from "./inspector.ts";
import { TodoLedger } from "./todo.ts";
import { panelLines, type PanelSnapshot } from "./panels-view.ts";
import { activityLines } from "./activity-view.ts";
import { NativePanels } from "./panels.ts";
import { stripTerminalSequences } from "@earendil-works/pi-tui";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const identity = { store: "st-test", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 3 };
function adapter() {
  let listener: (event: { toolName: string; details: unknown; content: AgentToolResult<unknown>["content"]; isError: boolean; parentToolCallId?: string }) => void = () => {};
  const pi = { on: (_name: string, fn: typeof listener) => { listener = fn; } } as unknown as ExtensionAPI;
  const events = new EventLog(3, 2);
  const activity = new InspectorActivity(events, () => {});
  activity.extend(pi, new Set(["corral_todo", "corral_gate"]));
  return { events, activity, send: (details: unknown, extra = {}) => listener({
    toolName: "corral_gate", details, content: [], isError: false, ...extra,
  }) };
}
const base = (activity: InspectorActivity): InspectorSnapshot => ({
  session: "session-real-adapter", boundSession: null, turn: 2, turnOpen: true,
  capabilities: null, guidance: { projectText: null, skillPaths: [], promptPaths: [], prompts: [], skills: [] },
  settings: [], budget: ["Budget unavailable · legacy generation"], ...activity.snapshot(),
});
const progress = (revision: number) => ({ revision, spec_revision: "spec", phases: [{ id: "p", label: "Implement", steps: [
  { id: "s", label: "Work", status: { state: "in_progress" as const }, criteria: ["C1"] },
] }] });

test("event and receipt adapter retains actual outcomes, replay, gaps and bounded omission", () => {
  const { events, activity, send } = adapter();
  events.emit("bound"); events.emit("turn_started", { turn: 2 }); events.emit("tool_denied");
  for (const state of ["committed", "rejected", "uncertain", "queued"]) send({ state, result: { code: "example" } });
  send({ state: "committed", replayed: true, result: { status: "started" } });
  send(undefined, { isError: true, content: [{ type: "text", text: "transport unavailable" }] });
  // Neither nested calls nor non-Corral tools can inject receipt outcomes.
  send({ state: "committed" }, { parentToolCallId: "nested" });
  send({ state: "committed" }, { toolName: "read" });
  expect(activity.snapshot().receipts).toHaveLength(6);
  const lines = inspectorActivity(base(activity)).join("\n");
  for (const text of ["rejected", "uncertain", "queued", "replayed", "transport unavailable", "Event gap: 1 missing", "not gate PASS"]) expect(lines).toContain(text);
  for (let n = 0; n < 30; n++) send({ state: "committed", result: { revision: n } });
  expect(activity.snapshot().receipts).toHaveLength(24);
  expect(activity.snapshot().receiptsOmitted).toBe(12);
});

test("canonical ledger adapters reject local assertions, rejected outcomes, late replay and stale reads", async () => {
  let read: unknown = { runs: [{ id: "R1", progress: progress(1) }] };
  const ledger = new TodoLedger(identity, async () => read);
  expect(ledger.unavailable).toBeTrue();
  await ledger.restore();
  await ledger.receipt({ state: "committed", result: { progress: progress(3) } });
  await ledger.receipt({ state: "committed", replayed: true, result: { progress: progress(2) } });
  for (const state of ["rejected", "uncertain", "queued", "agent says done"]) await ledger.receipt({ state, result: { progress: progress(9) } });
  expect(ledger.current?.revision).toBe(3);
  await expect(ledger.restore()).rejects.toThrow("older ledger");
  expect(ledger.unavailable).toBeTrue();
  expect(ledger.current?.revision).toBe(3);
  read = { runs: [{ id: "R1", progress: progress(4) }] };
  await ledger.restore();
  expect(ledger.unavailable).toBeFalse();
  for (const role of ["pm", "assistant"] as const) {
    const data = { todo: progress(1), todo_requests: null };
    const own = new TodoLedger({ ...identity, role }, async () => ({ container: role === "pm" ? data : { state: data } }));
    await own.restore(); expect(own.current?.phases[0]?.steps[0]?.criteria).toEqual(["C1"]);
  }
});

test("disconnected/legacy inspectors and activity never present unknown as successful acceptance", async () => {
  const { activity } = adapter();
  const info = base(activity);
  const ledger = new TodoLedger(identity, async () => ({ runs: [{ id: "R1", progress: progress(1) }] }));
  await ledger.restore();
  const snapshot: PanelSnapshot = { bound: false, model: null, effort: "off", streaming: false, context: undefined,
    tokens: 0, tools: [], contextFiles: [], skills: [], ledger: ledger.current, ledgerStale: true, jobs: null,
    auxiliary: null, steering: 0, followUps: 0, inspector: info };
  expect(inspectorContext(info).join("\n")).toContain("legacy launch");
  expect(panelLines("Agents", identity, snapshot, { kind: "unread" }).map(row => row.text).join("\n")).toContain("Disconnected/unbound");
  const plan = panelLines("Plan", identity, snapshot, { kind: "unread" }).map(row => row.text).join("\n");
  expect(plan).toContain("stale"); expect(plan).toContain("Criterion: C1"); expect(plan).toContain("not gate evidence");
  expect(activityLines(snapshot, 0).join("\n")).toContain("No tool receipt observed");
  expect(panelLines("Context", identity, null, { kind: "unread" })[0]?.text).toContain("unavailable");
  const panels = new NativePanels({ identity, cwd: ".", snapshot: () => snapshot });
  panels.focused = true; panels.tab = "Plan";
  const rendered = stripTerminalSequences([...panels.renderSidebar(80, 20), ...panels.renderInline(80, 34)].join("\n"));
  expect(rendered).toContain("Criterion: C1");
  const evidence = resolve(import.meta.dirname, "../../../../.evidence");
  mkdirSync(evidence, { recursive: true });
  writeFileSync(resolve(evidence, "T-689-adapter-plan-render.txt"), rendered);
  panels.dispose();
});
