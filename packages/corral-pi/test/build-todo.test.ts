import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { createTodoLedger } from "../src/build/todo-ledger.ts";
import { RequestContext } from "../src/host/request.ts";
import { sessionTodoTool } from "../src/roles/common/todo.ts";
import type { HostIdentity } from "../src/roles/types.ts";
import { ProgressSchema } from "../src/ui/todo-widget.ts";
import { TodoLedger, snapshotLedger } from "../src/ui/todo.ts";

const identity: HostIdentity = { store: "", project: "", role: "assistant", assistant: "build-test", generation: 1 };
const directories: string[] = [];
afterEach(() => {
  for (const dir of directories.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "corral-build-todo-"));
  directories.push(dir);
  const ledger = createTodoLedger(dir);
  let request = 0;
  return { dir, ledger, async send(payload: Record<string, unknown>) {
    return await ledger.handle({ requestId: `todo-${++request}`, payload: sessionTodoTool.payload(payload) }) as {
      revision: number; progress: unknown; plan: string; disposition: unknown; closed_plans: number;
    };
  } };
}

const plan = { op: "init", phases: [
  { id: "research", label: "Research", steps: [{ id: "read", label: "Read the contract" }] },
  { id: "work", label: "Work", steps: [
    { id: "implement", label: "Implement" }, { id: "verify", label: "Verify" },
  ] },
] };

async function refused(send: (payload: Record<string, unknown>) => Promise<unknown>, payload: Record<string, unknown>, reason: string) {
  let error: unknown;
  try { await send(payload); } catch (caught) { error = caught; }
  expect(error instanceof Error).toBeTrue();
  expect(error).toMatchObject({ code: "bad_request", message: reason });
}

test("an empty build snapshot restores through the widget's assistant parser", async () => {
  const { ledger } = fixture();
  expect(snapshotLedger(ledger.snapshot(), identity)).toEqual({ progress: null, continued: null });
  const host = new TodoLedger(identity, async () => ledger.snapshot());
  await host.restore();
  expect(host.current).toBeNull();
  expect(host.unavailable).toBe(false);
  const request = new RequestContext(host, () => {});
  const inert = { request: { plan: "P1", revision: 0, next: null, continuation: { state: "not_due" } } };
  expect(request.due(inert)).toBeUndefined();
  expect(request.stopped(inert)).toBeUndefined();
});

test("two-phase receipts update the widget ledger and its completion counts", async () => {
  const { ledger, send } = fixture();
  const host = new TodoLedger(identity, async () => ledger.snapshot());
  const initialized = await send(plan);
  expect(initialized).toMatchObject({ revision: 1, plan: "P1", closed_plans: 0,
    disposition: { state: "actionable", next: "read" } });
  await host.receipt({ state: "committed", result: initialized });
  await send({ op: "start", step: "read", plan: "P1" });
  const done = await send({ op: "done", step: "read", plan: "P1" });
  await host.receipt({ state: "committed", result: done });
  const progress = ProgressSchema.parse(host.current);
  const steps = progress.phases.flatMap((phase) => phase.steps);
  expect(progress.revision).toBe(3);
  expect(progress.phases).toHaveLength(2);
  expect(steps.filter((step) => step.status.state === "done")).toHaveLength(1);
  expect(steps).toHaveLength(3);
  expect(progress.phases.map((phase) => phase.steps.filter((step) => step.status.state === "done").length)).toEqual([1, 0]);
  expect(done.disposition).toEqual({ state: "actionable", next: "implement" });
});

test("revisions advance across rollover and an older plan is refused without a write", async () => {
  const { dir, ledger, send } = fixture();
  await send({ op: "init", phases: [{ id: "p", label: "Phase", steps: [{ id: "s", label: "Step" }] }] });
  await refused(send, plan, "todo_plan_open");
  await send({ op: "done", step: "s" });
  const second = await send(plan);
  expect(second).toMatchObject({ revision: 3, plan: "P2", closed_plans: 1 });
  const before = readFileSync(join(dir, "todo.json"), "utf8");
  await refused(send, { op: "done", step: "read", plan: "P1" }, "stale_todo_plan");
  expect(readFileSync(join(dir, "todo.json"), "utf8")).toBe(before);
  expect(snapshotLedger(ledger.snapshot(), identity).progress?.revision).toBe(3);
});

test("invalid identities, transitions and acceptance links cannot partially mutate the plan", async () => {
  const { dir, ledger, send } = fixture();
  await refused(send, { op: "done", step: "missing" }, "todo_not_initialized");
  await send(plan);
  await send({ op: "start", step: "read" });
  const before = ledger.snapshot();
  const persisted = readFileSync(join(dir, "todo.json"), "utf8");
  await refused(send, { op: "done", step: "missing" }, "unknown_todo_step");
  await refused(send, { op: "start", step: "implement" }, "multiple_current_steps");
  await refused(send, { op: "drop", step: "read", reason: " " }, "drop_reason_required");
  await refused(send, { op: "append", phase: "new", steps: [] }, "unknown_todo_phase");
  await refused(send, { op: "append", phase: "work", steps: [{ id: "read", label: "Duplicate" }] }, "invalid_todo_step");
  await refused(send, { op: "append", phase: "work", steps: [{ id: "linked", label: "Linked", criteria: ["A1"] }] },
    "a PM or assistant todo cannot link task acceptance criteria");
  expect(ledger.snapshot()).toEqual(before);
  expect(readFileSync(join(dir, "todo.json"), "utf8")).toBe(persisted);
  await send({ op: "done", step: "read" });
  await refused(send, { op: "start", step: "read" }, "terminal_todo_step");
});

test("append, wait, start and drop preserve canonical dispositions and clear waits", async () => {
  const { ledger, send } = fixture();
  await send({ op: "init", phases: [{ id: "p", label: "Phase", steps: [{ id: "s", label: "Step" }] }] });
  const event = await send({ op: "wait", step: "s", on: { kind: "event", event: "review finishes" }, when: ["decision:approved"] });
  expect(event.disposition).toEqual({ state: "waiting_event", event: "review finishes", steps: ["s"] });
  expect(ProgressSchema.parse(event.progress).waits).toEqual([{ step: "s", on: { kind: "event", event: "review finishes" } }]);
  await send({ op: "append", phase: "p", label: "Ignored existing label", steps: [{ id: "owner", label: "Owner answer" }] });
  const owner = await send({ op: "wait", step: "owner", on: { kind: "owner", question: "Which option?" } });
  expect(owner.disposition).toEqual({ state: "awaiting_owner", question: "Which option?", steps: ["owner"] });
  const started = await send({ op: "start", step: "s" });
  expect(started.disposition).toEqual({ state: "actionable", next: "s" });
  expect(ProgressSchema.parse(started.progress).waits?.map((wait) => wait.step)).toEqual(["owner"]);
  await send({ op: "done", step: "s" });
  const dropped = await send({ op: "drop", step: "owner", reason: "Owner cancelled this question" });
  expect(dropped.disposition).toEqual({ state: "complete" });
  expect(ProgressSchema.parse(dropped.progress).waits).toBeUndefined();
  const appended = await send({ op: "append", phase: "new", label: "New phase", steps: [{ id: "new", label: "New step" }] });
  expect(ProgressSchema.parse(appended.progress).phases.map((phase) => phase.label)).toEqual(["Phase", "New phase"]);
  const snapshot = snapshotLedger(ledger.snapshot(), identity).progress;
  expect(snapshot?.phases[0]?.steps[1]?.status).toEqual({ state: "dropped", reason: "Owner cancelled this question" });
});

test("wait validation uses corral refusals without recording invalid waits", async () => {
  const { ledger, send } = fixture();
  await send(plan);
  const before = ledger.snapshot();
  await refused(send, { op: "wait", step: "read", on: { kind: "event", event: "\u001bcommand" } }, "invalid_todo_wait");
  await refused(send, { op: "wait", step: "read", on: { kind: "owner", question: "?" }, deadline: "2000-01-01T00:00:00Z" }, "todo_wait_deadline_range");
  await refused(send, { op: "wait", step: "read", on: { kind: "event", event: "event" }, when: [] }, "todo_wait_requires_1_to_8_checks");
  await refused(send, { op: "wait", step: "read", on: { kind: "event", event: "event" }, when: ["arbitrary"] }, "resume-when requires kind:target");
  expect(ledger.snapshot()).toEqual(before);
  let invalid: unknown;
  try { await ledger.handle({ requestId: "invalid-action", payload: { op: "replace" } }); } catch (error) { invalid = error; }
  expect(invalid).toMatchObject({ code: "bad_request",
    message: "malformed todo payload (data error) at payload; expected fields: a todo init/append/start/done/drop/wait command" });
});

test("a resumed ledger reloads its plan and private file permissions", async () => {
  const { dir, ledger, send } = fixture();
  await send(plan);
  await send({ op: "done", step: "read" });
  await send({ op: "wait", step: "implement", on: { kind: "event", event: "dependency lands" }, when: ["landed:T-1"] });
  const resumed = createTodoLedger(dir);
  expect(snapshotLedger(resumed.snapshot(), identity)).toEqual(snapshotLedger(ledger.snapshot(), identity));
  expect(statSync(join(dir, "todo.json")).mode & 0o777).toBe(0o600);
  const result = await resumed.handle({ requestId: "resumed", payload: { op: "start", step: "implement", plan: "P1" } });
  expect(result).toMatchObject({ revision: 4, plan: "P1", disposition: { state: "actionable", next: "implement" } });
  expect(snapshotLedger(createTodoLedger(dir).snapshot(), identity).progress?.revision).toBe(4);
});
