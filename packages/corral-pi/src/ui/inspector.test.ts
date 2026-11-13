import { expect, test } from "bun:test";
import type { AgentToolResult, ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../bridge/events.ts";
import { InspectorActivity, inspectorActivity, inspectorContext, type InspectorSnapshot } from "./inspector.ts";
import { TodoLedger } from "./todo.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import { NativePanels } from "./panels.ts";
import { stripTerminalSequences, Text } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { runChild, childKey } from "../extensions/codemode/child.ts";

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
  // Unauthenticated nested calls and non-Corral tools cannot inject receipt outcomes.
  send({ state: "committed" }, { parentToolCallId: "nested" });
  send({ state: "committed" }, { toolName: "read" });
  expect(activity.snapshot().receipts).toHaveLength(6);
  const lines = inspectorActivity(base(activity)).join("\n");
  for (const text of ["rejected", "uncertain", "queued", "replayed", "transport unavailable", "Event gap: 1 missing", "not gate PASS"]) expect(lines).toContain(text);
  for (let n = 0; n < 30; n++) send({ state: "committed", result: { revision: n } });
  expect(activity.snapshot().receipts).toHaveLength(24);
  expect(activity.snapshot().receiptsOmitted).toBe(12);
});

test("only authenticated, non-denied Code Mode children can feed receipt and ledger adapters", async () => {
  type Result = { toolName: string; toolCallId: string; parentToolCallId?: string; details: unknown; content: []; isError: boolean };
  const listeners: ((event: Result) => unknown)[] = [];
  const pi = { on: (_name: string, fn: (event: Result) => unknown) => { listeners.push(fn); } } as unknown as ExtensionAPI;
  const activity = new InspectorActivity(new EventLog(3), () => {});
  const ledger = new TodoLedger(identity, async () => ({ runs: [{ id: "R1", progress: progress(1) }] }));
  await ledger.restore();
  activity.extend(pi, new Set(["corral_todo"])); ledger.extend(pi);
  const parent = { tool_call_id: "code", cell: "cell-1", call: "cell-1.1" };
  const child = { key: childKey(parent), parent, tool: "corral_todo" };
  const event: Result = { toolName: "corral_todo", toolCallId: "code/1", parentToolCallId: "code", content: [], isError: false,
    details: { state: "committed", result: { progress: progress(2) } } };
  const send = async (result = event) => { await Promise.all(listeners.map(fn => fn(result))); };
  await send(); // A receipt-shaped object and parent ID are insufficient.
  await runChild({ ...child, denied: "not allowed" }, () => send());
  await runChild(child, () => send({ ...event, parentToolCallId: "foreign" }));
  await runChild({ ...child, tool: "read" }, () => send());
  expect(ledger.current?.revision).toBe(1); expect(activity.snapshot().receipts).toHaveLength(0);
  await runChild(child, () => send());
  expect(ledger.current?.revision).toBe(2); expect(activity.snapshot().receipts).toHaveLength(1);
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
  expect(inspectorActivity(info).join("\n")).toContain("No tool receipt observed");
  const panels = new NativePanels({ identity, cwd: ".", snapshot: () => snapshot });
  const root = panels.workbench.mount(new Text("", 0, 0));
  panels.workbench.openAgents();
  const agents = stripTerminalSequences(renderLayoutFrame(root, 80, 30, () => {}).lines.join("\n"));
  expect(agents).toContain("Host unbound"); expect(agents).toContain("unknown, not none");
  expect(agents).not.toMatch(/\bPASS\b|accepted/);
  panels.sidebar.tab = "plan";
  const rendered = stripTerminalSequences(panels.renderSidebar(80, 20).join("\n"));
  expect(rendered).toContain("Plan"); expect(rendered).toContain("stale");
  const evidence = resolve(import.meta.dirname, "../../../../.evidence");
  mkdirSync(evidence, { recursive: true });
  writeFileSync(resolve(evidence, "T-689-adapter-plan-render.txt"), rendered);
  panels.dispose();
});
