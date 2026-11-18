import { expect, test } from "bun:test";
import type { AgentToolResult, ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../bridge/events.ts";
import { InspectorActivity, inspectorContext, receiptLines, type InspectorSnapshot } from "./inspector.ts";
import { TodoLedger } from "./todo.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import { NativePanels } from "./panels.ts";
import { statusBar } from "./panels-chrome.ts";
import { stripTerminalSequences, Text } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { runChild, childKey } from "../extensions/codemode/child.ts";

const identity = { store: "st-test", project: "demo", role: "worker" as const, task: "T-001", run: "R1" };
type Observed = { toolName: string; input: Record<string, unknown>; details: unknown; content: AgentToolResult<unknown>["content"]; isError: boolean; parentToolCallId?: string };
function adapter() {
  let listener: (event: Observed) => void = () => {};
  const pi = { on: (_name: string, fn: typeof listener) => { listener = fn; } } as unknown as ExtensionAPI;
  const events = new EventLog(3);
  const activity = new InspectorActivity(events, () => {});
  activity.extend(pi, new Set(["corral_todo", "corral_done"]));
  return { events, activity, send: (details: unknown, extra: Partial<Observed> = {}) => listener({
    toolName: "corral_done", input: { summary: "Implemented." }, details, content: [{ type: "text", text: JSON.stringify(details) }],
    isError: false, ...extra,
  }) };
}
const base = (activity: InspectorActivity): InspectorSnapshot => ({
  session: "session-real-adapter", boundSession: null, turn: 2, turnOpen: true,
  capabilities: null, guidance: { projectText: null, skillPaths: [], promptPaths: [], prompts: [], skills: [] },
  settings: [], budget: ["Budget unavailable · legacy launch"], ...activity.snapshot(),
});
const progress = (revision: number) => ({ revision, spec_revision: "spec", phases: [{ id: "p", label: "Implement", steps: [
  { id: "s", label: "Work", status: { state: "in_progress" as const }, criteria: ["C1"] },
] }] });
/** A corral_todo success as the host-local ledger returns it. */
const todoResult = (revision: number) => ({ revision, progress: progress(revision), plan: "P1", disposition: { state: "actionable", next: "s" }, closed_plans: 0 });

test("observed operations keep ok or failed and nesting, bounded; gaps and omissions in both modes", () => {
  const { events, activity, send } = adapter();
  events.emit("bound"); events.emit("turn_started", { turn: 2 }); events.emit("tool_denied"); events.emit("turn_ended");
  send({ id: "T-001", status: "review" });
  send({ reason: "T-001 is not running", next: ["corral start T-001"] }, { isError: true });
  // Unauthenticated nested calls and non-Corral tools cannot inject outcomes.
  send({ id: "T-001" }, { parentToolCallId: "nested" });
  send({ id: "T-001" }, { toolName: "read" });
  const rows = activity.snapshot().receipts;
  expect(rows.map(row => [row.tool, row.state, row.nested])).toEqual([["corral_done", "ok", false], ["corral_done", "failed", false]]);
  const now = Date.now(), text = (set: readonly { text: string }[]) => set.map(line => line.text).join("\n");
  const first = receiptLines(base(activity), now);
  const operations = first.lines.slice(first.lines.findIndex((line, index) => index > 0 && line.heading) + 1).slice(0, rows.length);
  expect(operations.map(line => line.text.split("  ")[1])).toEqual([
    "✓ Finished the run · sent to review · done · ok", "✗ Finish · refused: T-001 is not running · done · failed",
  ]);
  // The exact records keep the observed summary and every retained event's time and kind; the readable lines read local time.
  expect(text(first.raw)).toContain("✗ Finish · refused: T-001 is not running");
  for (const event of base(activity).events.events) {
    expect(text(first.raw)).toContain(event.at); expect(text(first.raw)).toContain(event.kind);
    expect(text(first.lines)).not.toContain(event.at);
  }
  for (let n = 0; n < 30; n++) send({ id: "T-001", status: "review" });
  expect(activity.snapshot().receipts).toHaveLength(24);
  expect(activity.snapshot().receiptsOmitted).toBe(8);
  const info = base(activity), shown = receiptLines(info, now);
  expect(info.events.gap?.missing).toBe(1);
  const page = info.events, gapless = { events: page.events, last: page.last, next: page.next, more: page.more };
  for (const variant of [{ ...info, events: gapless }, { ...info, receiptsOmitted: 0 }]) {
    const other = receiptLines(variant, now);
    expect(text(other.lines)).not.toBe(text(shown.lines)); expect(text(other.raw)).not.toBe(text(shown.raw));
  }
});

test("only authenticated, non-denied Code Mode children can feed the operation and ledger adapters", async () => {
  type Result = { toolName: string; toolCallId: string; parentToolCallId?: string; input: Record<string, unknown>; details: unknown; content: []; isError: boolean };
  const listeners: ((event: Result) => unknown)[] = [];
  const pi = { on: (_name: string, fn: (event: Result) => unknown) => { listeners.push(fn); } } as unknown as ExtensionAPI;
  const activity = new InspectorActivity(new EventLog(3), () => {});
  const ledger = new TodoLedger(identity, async () => ({ runs: [{ id: "R1", progress: progress(1) }] }));
  await ledger.restore();
  activity.extend(pi, new Set(["corral_todo"])); ledger.extend(pi);
  const parent = { tool_call_id: "code", cell: "cell-1", call: "cell-1.1" };
  const child = { key: childKey(parent), parent, tool: "corral_todo" };
  const event: Result = { toolName: "corral_todo", toolCallId: "code/1", parentToolCallId: "code", input: { op: "start", step: "s" },
    content: [], isError: false, details: todoResult(2) };
  const send = async (result = event) => { await Promise.all(listeners.map(fn => fn(result))); };
  await send(); // A result-shaped object and parent ID are insufficient.
  await runChild({ ...child, denied: "not allowed" }, () => send());
  await runChild(child, () => send({ ...event, parentToolCallId: "foreign" }));
  await runChild({ ...child, tool: "read" }, () => send());
  expect(ledger.current?.revision).toBe(1); expect(activity.snapshot().receipts).toHaveLength(0);
  await runChild(child, () => send());
  expect(ledger.current?.revision).toBe(2); expect(activity.snapshot().receipts.map(row => row.nested)).toEqual([true]);
});

test("the ledger adapter applies returned progress, ignores failures and older revisions, and rereads when a result has none", async () => {
  let read: unknown = { runs: [{ id: "R1", progress: progress(1) }] };
  const ledger = new TodoLedger(identity, async () => read);
  expect(ledger.unavailable).toBeTrue();
  await ledger.restore();
  await ledger.result(todoResult(3), false);
  await ledger.result(todoResult(2), false);
  await ledger.result(todoResult(9), true);
  expect(ledger.current?.revision).toBe(3);
  await expect(ledger.restore()).rejects.toThrow("older ledger");
  expect(ledger.unavailable).toBeTrue();
  expect(ledger.current?.revision).toBe(3);
  read = { runs: [{ id: "R1", progress: progress(4) }] };
  await ledger.result({ note: "no progress" }, false);
  expect(ledger.unavailable).toBeFalse();
  expect(ledger.current?.revision).toBe(4);
  const data = { todo: progress(1), todo_requests: null };
  const own = new TodoLedger({ ...identity, role: "assistant" }, async () => ({ container: { state: data } }));
  await own.restore(); expect(own.current?.phases[0]?.steps[0]?.criteria).toEqual(["C1"]);
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
  const panels = new NativePanels({ identity, cwd: ".", snapshot: () => snapshot });
  const root = panels.workbench.mount(new Text("", 0, 0));
  panels.workbench.openAgents();
  const agents = stripTerminalSequences(renderLayoutFrame(root, 80, 30, () => {}).lines.join("\n"));
  // Nothing observed reads as nothing yet, and the status bar says the host is unbound: unknown is never shown as success.
  expect(agents).toContain("No agents yet.");
  const status = stripTerminalSequences(statusBar(identity, snapshot, new Map(), 105).join("\n"));
  expect(status).toContain("○"); expect(status).toContain("unbound"); expect(status).not.toContain("●");
  expect(agents).not.toMatch(/\bPASS\b|accepted/);
  panels.sidebar.tab = "plan";
  const rendered = stripTerminalSequences(panels.renderSidebar(80, 20).join("\n"));
  expect(rendered).toContain("Plan"); expect(rendered).toContain("stale");
  const evidence = resolve(import.meta.dirname, "../../../../.evidence");
  mkdirSync(evidence, { recursive: true });
  writeFileSync(resolve(evidence, "T-689-adapter-plan-render.txt"), rendered);
  panels.dispose();
});
