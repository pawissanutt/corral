import { test, expect } from "bun:test";
import { TaskStore, taskHeadline, taskLines } from "./task-view.ts";

const worker = { store: "s", project: "corral", role: "worker" as const, task: "T-796", run: "R1", generation: 1 };
const assistant = { store: "s", project: "", role: "assistant" as const, assistant: "A-1", generation: 1 };
const record = {
  id: "T-796", title: "Main regression: receipts_tests\u001b[31m list", status: "running", status_since: "2026-11-04T00:00:00Z",
  kind: "test", size: "xs", difficulty: "routine", priority: "P1", project: "corral",
  body: "## Goal\nMain is red.\n\n## Acceptance\n- targeted test passes\n- full gate green\n",
  criteria_progress: [{ criterion: { id: "gate", required: true, live: false, scenario: "s", expected: "full gate green" }, steps: [], evidence: [] }],
  runs: [{ id: "R1", agent: "pi-terra", agent_state: "working", started: "2026-11-04T00:00:00Z", summary: "Updated expectations" }],
  comments: [{ at: "2026-11-04T00:30:00Z", author: "pm", text: "approve" }],
  extra_field: { ignored: true },
};
const text = (rows: ReturnType<typeof taskLines>) => rows.map(row => row.text).join("\n");

test("task view shows spec sections, criteria as spec only, the bound run and recent comments", async () => {
  const store = new TaskStore(worker, async () => record, () => {});
  await store.refresh();
  const out = text(taskLines(worker, store, Date.parse("2026-11-04T01:00:00Z")));
  expect(out).toContain("running for 1h · test · xs · routine · corral");
  expect(out).toContain("Goal"); expect(out).toContain("Main is red.");
  expect(out).toContain("• targeted test passes");
  expect(out).toContain("full gate green"); expect(out).toContain("not a pass");
  expect(out).toContain("Run R1 (this session)");
  expect(out).toContain("pm · 30m ago");
  expect(taskHeadline(worker, store)).toEqual({ id: "T-796", title: "Main regression: receipts_tests [31m list", chips: ["running", "P1", "xs"] });
});

test("a failed or mismatched read stays visible as stale and never invents task content", async () => {
  let value: unknown = { ...record, id: "T-1" };
  const store = new TaskStore(worker, async () => value, () => {});
  await store.refresh();
  expect(store.current).toBeUndefined();
  expect(text(taskLines(worker, store))).toContain("STALE / unavailable: Task identity mismatch");
  expect(taskHeadline(worker, store)?.title).toBe("Task unavailable");
  value = record; await store.refresh(true);
  expect(store.stale).toBeFalse();
  value = { nope: 1 }; await store.refresh(true);
  expect(store.current?.id).toBe("T-796");
  expect(taskHeadline(worker, store)?.chips).toContain("stale");
});

test("sessions without a task are unbound and never read", async () => {
  let reads = 0;
  const store = new TaskStore(assistant, async () => { reads++; return record; }, () => {});
  await store.refresh(true);
  expect(reads).toBe(0); expect(store.bound).toBeFalse();
  expect(taskHeadline(assistant, store)).toBeUndefined();
  expect(text(taskLines(assistant, store))).toContain("No task bound");
});
