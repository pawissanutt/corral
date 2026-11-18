import { expect, test } from "bun:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, Text } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { runChild, childKey } from "../extensions/codemode/child.ts";
import { TodoLedger } from "./todo.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import { NativePanels } from "./panels.ts";

const identity = { store: "st-test", project: "demo", role: "worker" as const, task: "T-001", run: "R1" };
const progress = (revision: number) => ({ revision, spec_revision: "spec", phases: [{ id: "p", label: "Implement", steps: [
  { id: "s", label: "Work", status: { state: "in_progress" as const }, criteria: ["C1"] },
] }] });
/** A corral_todo success as the host-local ledger returns it. */
const todoResult = (revision: number) => ({ revision, progress: progress(revision), plan: "P1", disposition: { state: "actionable", next: "s" }, closed_plans: 0 });

test("only authenticated, non-denied Code Mode children can feed the ledger", async () => {
  type Result = { toolName: string; toolCallId: string; parentToolCallId?: string; input: Record<string, unknown>; details: unknown; content: []; isError: boolean };
  const listeners: ((event: Result) => unknown)[] = [];
  const pi = { on: (_name: string, fn: (event: Result) => unknown) => { listeners.push(fn); } } as unknown as ExtensionAPI;
  const ledger = new TodoLedger(identity, async () => ({ runs: [{ id: "R1", progress: progress(1) }] }));
  await ledger.restore();
  ledger.extend(pi);
  const parent = { tool_call_id: "code", cell: "cell-1", call: "cell-1.1" };
  const child = { key: childKey(parent), parent, tool: "corral_todo" };
  const event: Result = { toolName: "corral_todo", toolCallId: "code/1", parentToolCallId: "code", input: { op: "start", step: "s" },
    content: [], isError: false, details: todoResult(2) };
  const send = async (result = event) => { await Promise.all(listeners.map(fn => fn(result))); };
  await send(); // A result-shaped object and parent ID are insufficient.
  await runChild({ ...child, denied: "not allowed" }, () => send());
  await runChild(child, () => send({ ...event, parentToolCallId: "foreign" }));
  await runChild({ ...child, tool: "read" }, () => send());
  expect(ledger.current?.revision).toBe(1);
  await runChild(child, () => send());
  expect(ledger.current?.revision).toBe(2);
});

test("the ledger applies returned progress, ignores failures and older revisions, and rereads when a result has none", async () => {
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

test("a stale plan and an empty host never read as success", async () => {
  const ledger = new TodoLedger(identity, async () => ({ runs: [{ id: "R1", progress: progress(1) }] }));
  await ledger.restore();
  const snapshot: PanelSnapshot = { bound: false, model: null, effort: "off", streaming: false, context: undefined,
    tokens: 0, tools: [], contextFiles: [], skills: [], ledger: ledger.current, ledgerStale: true, jobs: null,
    auxiliary: null, steering: 0, followUps: 0 };
  const panels = new NativePanels({ identity, cwd: ".", snapshot: () => snapshot });
  const root = panels.workbench.mount(new Text("", 0, 0));
  panels.workbench.openAgents();
  const agents = stripTerminalSequences(renderLayoutFrame(root, 80, 30, () => {}).lines.join("\n"));
  expect(agents).toContain("No agents yet.");
  expect(agents).not.toMatch(/\bPASS\b|accepted/);
  panels.sidebar.tab = "plan";
  const rendered = stripTerminalSequences(panels.renderSidebar(80, 20).join("\n"));
  expect(rendered).toContain("Plan"); expect(rendered).toContain("stale");
  panels.dispose();
});
