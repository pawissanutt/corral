import { expect, test } from "bun:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { planLedger } from "./plan-detail.ts";
import { installPlanCommand, ProgressSchema, snapshotProgress, TodoLedger, type PlanSurface } from "./todo.ts";

const progress = ProgressSchema.parse({
  revision: 4, spec_revision: "s1", plan: "P3",
  phases: [{ id: "build", label: "Implementation", steps: [
    { id: "read", label: "Read existing UI", status: { state: "done" } },
    { id: "widget", label: "Build expandable widget", status: { state: "in_progress" }, criteria: ["A1"] },
    { id: "test", label: "Test keyboard and mouse", status: { state: "pending" } },
    { id: "extra", label: "Extra check", status: { state: "dropped", reason: "Covered by targeted checks" } },
  ] }],
  waits: [{ step: "test", on: { kind: "owner", question: "Ship it?" } }, { step: "gone", on: { kind: "event", event: "ci" } }],
});
const identity = { store: "s", project: "p", role: "worker", task: "T-1", run: "R1", generation: 2 } as const;

/** The `/todo` command and `session_start` handler a host installs, over a recording Plan surface. */
function harness(load: () => Promise<unknown>) {
  let command: ((args: string, ctx: unknown) => Promise<void>) | undefined;
  let start: ((event: unknown, ctx: unknown) => Promise<void>) | undefined;
  const pi = {
    on: (name: string, fn: typeof start) => { if (name === "session_start") start = fn; },
    registerCommand: (_name: string, spec: { handler: typeof command }) => { command = spec.handler; },
  } as unknown as ExtensionAPI;
  const calls: string[] = [];
  const surface: PlanSurface = {
    show: () => { calls.push("show"); },
    scroll: (delta) => { calls.push(`scroll ${delta}`); },
    notice: (text, tone) => { calls.push(`${tone}: ${text}`); },
  };
  const ledger = new TodoLedger(identity, load);
  installPlanCommand(pi, ledger, true, () => surface);
  const ctx = { mode: "tui", ui: { notify: (text: string) => { calls.push(`notify: ${text}`); } } };
  return { ledger, calls, run: (args: string) => command!(args, ctx), start: () => start!({}, ctx) };
}

test("/todo shows the Plan tab, up and down turn its page, refresh rereads and says so", async () => {
  // Given a host whose terminal UI opened and restored the canonical ledger.
  const host = harness(async () => ({ runs: [{ id: "R1", progress }] }));
  await host.start();
  expect(host.ledger.current?.revision).toBe(4);
  // When the owner runs each form of the command.
  for (const args of ["", "up", "down", "refresh", "later"]) await host.run(args);
  // Then each reaches the one Plan surface; no widget or transcript line is involved.
  expect(host.calls).toEqual(["show", "scroll -1", "scroll 1", "text: Plan refreshed · revision 4",
    "warning: Use /todo, /todo up, /todo down or /todo refresh"]);
});

test("a failed reread keeps the confirmed revision, marks it stale and says why on the surface", async () => {
  // Given a confirmed revision.
  let fail = false;
  const host = harness(async () => {
    if (fail) throw new Error("todo_read: store offline");
    return { runs: [{ id: "R1", progress }] };
  });
  await host.start();
  // When an older receipt replays and a refresh then fails.
  expect(host.ledger.apply({ ...progress, revision: 1, phases: [] })).toBeFalse();
  fail = true;
  await host.run("refresh");
  // Then the newer ledger stays, unavailable, and the failure is a warning notice, not a success.
  expect(host.ledger.current?.revision).toBe(4);
  expect(host.ledger.unavailable).toBeTrue();
  expect(host.calls).toEqual(["warning: Plan is stale · showing revision 4 · todo_read: store offline"]);
});

test("the full ledger keeps every id, criterion, wait, drop reason, revision and continuation, and opens on a step", () => {
  // Given a stale plan with an auto-continue that stopped, and labels carrying terminal controls.
  const unsafe = { ...progress, phases: [{ ...progress.phases[0]!, steps: [
    { id: "ctl", label: "Escape \u001b[31m\nsplit", status: { state: "pending" as const }, criteria: [] },
    ...progress.phases[0]!.steps] }] };
  const ledger = planLedger({ progress: unsafe, stale: true,
    continued: { key: "continue:P3:4", plan: "P3", revision: 4, attempt: 3, stopped: "no_progress" } });
  const text = ledger.lines.join("\n");
  // Then every recorded fact is in the pager text, and nothing a step did not record is claimed.
  for (const part of ["Plan P3 · revision 4", "spec revision s1", "stale", "Implementation · id build", "id read · done",
    "id widget · in progress · criteria A1", "? waits for you: Ship it?", "– dropped: Covered by targeted checks",
    "… gone waits for ci", "! auto-continue stopped: no progress · attempt 3 · plan P3 revision 4 · key continue:P3:4"]) {
    expect(text).toContain(part);
  }
  expect(text).not.toMatch(/accepted|passed/i);
  expect(ledger.title).toBe("Plan P3 · revision 4");
  // Controls never reach the pager or split a step over two source lines, so a step's line is where it opens.
  expect(text).not.toContain("\u001b");
  expect(ledger.lines[ledger.startOf("widget")]).toBe("  ● Build expandable widget");
  expect(ledger.startOf(undefined)).toBe(0);
  expect(ledger.startOf("missing")).toBe(0);
});

test("worker restore selects the bound run, not the latest run", () => {
  // Given historical and current runs with different ledgers.
  const snapshot = { runs: [{ id: "R1", progress }, { id: "R2", progress: null }] };
  // When resuming the earlier bound run.
  const restored = snapshotProgress(snapshot, identity);
  // Then the original ledger is restored.
  expect(restored).toEqual(progress);
});

test("PM restores its own ledger and malformed snapshots are refused", () => {
  // Given the PM container snapshot.
  const pm = { store: "s", project: "p", role: "pm", generation: 2 } as const;
  // When the PM ledger is restored.
  expect(snapshotProgress({ container: { todo: progress } }, pm)).toEqual(progress);
  // Then an invalid status is not accepted as canonical progress.
  expect(() => snapshotProgress({ container: { todo: { ...progress, phases: "broken" } } }, pm)).toThrow();
});
