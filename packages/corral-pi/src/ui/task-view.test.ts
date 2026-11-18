import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { clock } from "./panel-parts.ts";
import type { HostIdentity } from "../roles/types.ts";
import { TaskStore } from "./task-store.ts";
import { taskHeadline, taskLines } from "./task-view.ts";

const worker = { store: "s", project: "corral", role: "worker" as const, task: "T-796", run: "R1" };
const assistant = { store: "s", project: "", role: "assistant" as const, assistant: "A-1" };
const run = (id: string, extra: Record<string, unknown>) => ({ id, task: "T-796", role: "worker", harness: "pi", worktree: "/w", branch: "corral/T-796",
  base: "main", base_sha: "a".repeat(40), started: "2026-11-04T00:00:00Z", ...extra });
const record = {
  task: {
    id: "T-796", title: "Main regression: receipts_tests\u001b[31m list", status: "blocked", priority: "P1", project: "corral", kind: "test", size: "xs",
    body: "## Goal\nMain is red.\n\n## Acceptance\n- targeted test passes\n  continues here\n- full gate green\n\n```sh\n# not a heading\nbun test --filter receipts\n```\n",
    blocked: { reason: "agent asks", detail: null, next: [] }, depends_on: [], round: 0,
    created: "2026-11-03T00:00:00Z", updated: "2026-11-04T00:30:00Z",
  },
  runs: [run("R0", { profile: "pi-sol", outcome: "failed", ended: "2026-11-04T00:10:00Z" }),
    run("R1", { profile: "pi-terra", model: "gpt-terra", summary: "Updated expectations" })],
  checks: [], artifacts: [],
};
const NOW = Date.parse("2026-11-04T01:00:00Z");
const view = (store: TaskStore, width: number, legend = false, identity: HostIdentity = worker) => taskLines(identity, store, width, { now: NOW, legend });
const plain = (rows: readonly string[]) => rows.map(stripTerminalSequences);
const bounded = (rows: readonly string[], width: number) => { for (const row of rows) expect(visibleWidth(row)).toBeLessThanOrEqual(width); };
const WIDTHS = [120, 101, 68, 44];
const LONG_REASON = `corral show exited 1: ${"database is locked, ".repeat(12)}tail-marker`;

test("a loaded task shows its own sections, the bound run and its facts at every width", async () => {
  const store = new TaskStore(worker, async () => record, () => {});
  await store.refresh();
  for (const width of WIDTHS) {
    const rows = view(store, width, true);
    bounded(rows, width);
    const out = plain(rows).join("\n");
    for (const part of ["Main is red.", "targeted test passes", "continues here", "full gate green",
      "bun test --filter receipts", "Updated expectations", "R1", "pi-terra", "gpt-terra", "xs", "P1", "corral", "test", "T-796"]) {
      expect(out).toContain(part);
    }
    // Fenced code is not read as a heading.
    expect(out).not.toContain("NOT A HEADING");
    // The age or clock time of the data on screen survives the shortest header form.
    expect(plain(rows.slice(0, 6)).join("\n")).toMatch(/\d+[smhd] ago|\d\d:\d\d/);
  }
  expect(plain(view(store, 120)).some(row => row.includes("│"))).toBeTrue();
  // The blocked reason is a task fact: the facts column states it beside the prose.
  expect(plain(view(store, 120)).join("\n")).toContain("blocked · agent asks");
  expect(plain(view(store, 101)).some(row => row.includes("│"))).toBeFalse();
  expect(view(store, 101).join("\n")).not.toContain("\u001b[31m");
  expect(taskHeadline(worker, store)).toEqual({ id: "T-796", title: "Main regression: receipts_tests [31m list", chips: ["⚠ blocked", "P1", "xs"] });
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
      expect(out).not.toContain("Facts");
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
  let value: unknown = { ...record, task: { ...record.task, id: "T-1" } };
  const store = new TaskStore(worker, async () => value, () => {});
  await store.refresh();
  expect(store.current).toBeUndefined();
  expect(store.state.phase).toBe("failed");
  expect(taskHeadline(worker, store)?.title).toBe("Task unavailable");
  value = record; await store.refresh(true);
  expect(store.stale).toBeFalse(); expect(store.state.phase).toBe("current");
  value = { nope: 1 }; await store.refresh(true);
  expect(store.current?.task.id).toBe("T-796");
  expect(store.state.phase).toBe("retained");
});

test("a missing field is unknown, never zero", async () => {
  const bare = { task: { id: "T-796", title: "Bare", body: "", status: "ready", priority: "P2" } };
  const store = new TaskStore(worker, async () => bare, () => {});
  await store.refresh();
  for (const width of [120, 44]) {
    const out = plain(view(store, width)).join("\n");
    expect(out).toContain("No task description");
    expect(out).toContain("Run R1 is not in this task record yet");
    expect(out).not.toMatch(/\b0\b/);
  }
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
