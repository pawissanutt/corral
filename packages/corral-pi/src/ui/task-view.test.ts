import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { clock } from "./panel-parts.ts";
import type { HostIdentity } from "../roles/types.ts";
import { TaskStore } from "./task-store.ts";
import { taskHeadline, taskLines } from "./task-view.ts";

const worker = { store: "s", project: "corral", role: "worker" as const, task: "T-796", run: "R1", generation: 1 };
const assistant = { store: "s", project: "", role: "assistant" as const, assistant: "A-1", generation: 1 };
const record = {
  id: "T-796", title: "Main regression: receipts_tests\u001b[31m list", status: "running", status_since: "2026-11-04T00:00:00Z",
  kind: "test", size: "xs", difficulty: "routine", priority: "P1", project: "corral",
  body: "## Goal\nMain is red.\n\n## Acceptance\n- targeted test passes\n  continues here\n- full gate green\n\n```sh\n# not a heading\nbun test --filter receipts\n```\n",
  criteria_progress: [
    { criterion: { id: "gate", required: true, live: false, scenario: "run the full gate", expected: "full gate green" }, steps: [1, 2], evidence: [] },
    { criterion: { id: "docs", required: false, live: false, scenario: "", expected: "docs updated" }, steps: [], evidence: [{}] },
  ],
  runs: [{ id: "R0", agent: "pi-sol", outcome: "abandoned" }, { id: "R1", agent: "pi-terra", agent_state: "working", started: "2026-11-04T00:00:00Z", summary: "Updated expectations" }],
  comments: [{ at: "2026-11-04T00:30:00Z", author: "pm", text: "approve" }],
  extra_field: { ignored: true },
};
const NOW = Date.parse("2026-11-04T01:00:00Z");
const view = (store: TaskStore, width: number, legend = false, identity: HostIdentity = worker) => taskLines(identity, store, width, { now: NOW, legend });
const plain = (rows: readonly string[]) => rows.map(stripTerminalSequences);
const bounded = (rows: readonly string[], width: number) => { for (const row of rows) expect(visibleWidth(row)).toBeLessThanOrEqual(width); };
const WIDTHS = [120, 101, 68, 44];
const LONG_REASON = `corral show exited 1: ${"database is locked, ".repeat(12)}tail-marker`;

test("a loaded task shows its own sections, spec-only criteria, the bound run and comments at every width", async () => {
  const store = new TaskStore(worker, async () => record, () => {});
  await store.refresh();
  for (const width of WIDTHS) {
    const rows = view(store, width, true);
    bounded(rows, width);
    const out = plain(rows).join("\n");
    for (const part of ["Main is red.", "targeted test passes", "continues here", "full gate green", "docs updated", "run the full gate",
      "bun test --filter receipts", "Updated expectations", "pm", "approve", "R1", "pi-terra", "working", "routine", "xs", "P1", "corral", "test", "T-796"]) {
      expect(out).toContain(part);
    }
    // Fenced code is not read as a heading.
    expect(out).not.toContain("NOT A HEADING");
    // The age or clock time of the data on screen survives the shortest header form.
    expect(plain(rows.slice(0, 6)).join("\n")).toMatch(/\d+[smhd] ago|\d\d:\d\d/);
    // Criteria are spec: no row about a criterion carries a pass mark.
    for (const row of plain(rows).filter(row => row.includes("full gate green") || row.includes("docs updated"))) expect(row).not.toMatch(/[✓✔]|passed/);
  }
  expect(plain(view(store, 120)).some(row => row.includes("│"))).toBeTrue();
  expect(plain(view(store, 101)).some(row => row.includes("│"))).toBeFalse();
  expect(view(store, 101).join("\n")).not.toContain("\u001b[31m");
  expect(taskHeadline(worker, store)).toEqual({ id: "T-796", title: "Main regression: receipts_tests [31m list", chips: ["● running", "P1", "xs"] });
});

test("a failed reread keeps the data, names its time and keeps the whole reason in the legend", async () => {
  let value: unknown = record;
  const store = new TaskStore(worker, async () => { if (value instanceof Error) throw value; return value; }, () => {});
  await store.refresh();
  expect(store.state.phase).toBe("current");
  value = new Error(LONG_REASON); await store.refresh(true);
  expect(store.state.phase).toBe("retained");
  expect(store.failedAt).toBeDefined();
  expect(taskHeadline(worker, store)?.chips).toContain("stale");
  for (const width of [101, 44]) {
    const page = plain(view(store, width));
    bounded(page, width);
    expect(page.join("\n")).toContain("Main is red.");
    expect(page.join("\n")).toContain(clock(store.checkedAt ?? 0));
    expect(page.join("\n")).not.toContain("tail-marker");
    const legend = plain(view(store, width, true));
    bounded(legend, width);
    expect(legend.join(" ").replace(/\s+/g, " ")).toContain("tail-marker");
  }
  value = record; await store.refresh(true);
  expect(store.state.phase).toBe("current"); expect(store.failedAt).toBeUndefined();
});

test("a task that was never read, is being read or could not be read shows no invented content", async () => {
  const gate = Promise.withResolvers<unknown>();
  const store = new TaskStore(worker, () => gate.promise, () => {});
  const empty = (phase: string) => {
    expect(store.state.phase).toBe(phase);
    for (const width of [101, 44]) {
      const rows = view(store, width);
      bounded(rows, width);
      const out = plain(rows).join("\n");
      expect(out).toContain("T-796");
      expect(out).not.toContain("Goal");
      expect(out).not.toMatch(/[☐◇]/);
    }
  };
  empty("never_read");
  const pending = store.refresh();
  empty("loading");
  gate.reject(new Error(LONG_REASON)); await pending;
  empty("failed");
  expect(store.current).toBeUndefined();
  expect(store.state.error).toContain("tail-marker");
  for (const width of [101, 44]) {
    const rows = view(store, width, true);
    bounded(rows, width);
    expect(plain(rows).join(" ").replace(/\s+/g, " ")).toContain("tail-marker");
  }
});

test("a mismatched read is a failure and recovery clears it without inventing content", async () => {
  let value: unknown = { ...record, id: "T-1" };
  const store = new TaskStore(worker, async () => value, () => {});
  await store.refresh();
  expect(store.current).toBeUndefined();
  expect(store.state.phase).toBe("failed");
  expect(taskHeadline(worker, store)?.title).toBe("Task unavailable");
  value = record; await store.refresh(true);
  expect(store.stale).toBeFalse(); expect(store.state.phase).toBe("current");
  value = { nope: 1 }; await store.refresh(true);
  expect(store.current?.id).toBe("T-796");
  expect(store.state.phase).toBe("retained");
});

test("a missing field is unknown, never zero", async () => {
  const bare = { id: "T-796", title: "Bare", status: "ready", criteria_progress: [{ criterion: { id: "c1", required: true, scenario: "s", expected: "works" } }] };
  const store = new TaskStore(worker, async () => bare, () => {});
  await store.refresh();
  for (const width of [101, 44]) {
    const out = plain(view(store, width)).join("\n");
    expect(out).toContain("works");
    expect(out).not.toMatch(/\b0 (step|steps|evidence)\b/);
  }
  const legacy = new TaskStore(worker, async () => ({ id: "T-796", title: "Bare", status: "ready" }), () => {});
  await legacy.refresh();
  expect(plain(view(legacy, 101)).join("\n")).not.toContain("0 criteria");
});

test("sessions without a task, or without a reader, are not applicable and never read", async () => {
  let reads = 0;
  const store = new TaskStore(assistant, async () => { reads++; return record; }, () => {});
  await store.refresh(true);
  expect(reads).toBe(0); expect(store.bound).toBeFalse();
  expect(store.state.phase).toBe("not_applicable"); expect(store.state.reason).toBe("no_task");
  expect(taskHeadline(assistant, store)).toBeUndefined();
  const unbound = new TaskStore(worker, undefined, () => {});
  await unbound.refresh(true);
  expect(unbound.state.phase).toBe("not_applicable"); expect(unbound.state.reason).toBe("no_reader");
  for (const width of [101, 44]) {
    for (const [identity, held] of [[assistant, store], [worker, unbound]] as const) {
      const rows = view(held, width, true, identity);
      bounded(rows, width);
      expect(rows.length).toBeGreaterThan(0);
    }
  }
});
