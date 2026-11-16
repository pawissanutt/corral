import { expect, test } from "bun:test";
import { EventLog } from "../bridge/events.ts";
import type { CorralClient, CorralReceipt } from "../tools/corral.ts";
import { LOCAL_UNAVAILABLE, nativeFor, planningLine, runHandoff, sheetFor, type HandoffState } from "./handoff.ts";
import type { HandoffSheet, SheetResult } from "../ui/handoff-sheet.ts";

const board: HandoffState = { driver: "board", label: "Board is handling this", waiting: "Queued for normal admission",
  plan_only: false, phase: "board_owned", task: "T-9", contract_revision: "c1", transfer: "x", revision: 2, shown: "v1" };

function fake(state: HandoffState, pick: (sheet: HandoffSheet) => void) {
  const calls: { op: string; payload: Record<string, unknown> }[] = [];
  const ok = (result: unknown) => ({ state: "committed", result }) as unknown as CorralReceipt;
  const client = { call: async (op: string, c: { payload: Record<string, unknown> }) => {
    calls.push({ op, payload: c.payload });
    if (op === "assistant_handoff" && c.payload.action === "read") return ok(state);
    if (op === "assistant_handoff") return ok({ option: (c.payload.native as { action: string }).action === "save" ? "save_for_later" : "work_on_this_here", digest: "d".repeat(64) });
    if (c.payload.kind === "owner_input") return ok({ input: "A-1/g1/i5" });
    return ok({ action: (c.payload.native as { action: string }).action });
  } } as unknown as CorralClient;
  const ui = { custom: async <T,>(factory: (tui: never, t: never, k: never, done: (r: T) => void) => unknown) => {
    let result: T | undefined;
    const sheet = factory({ requestRender() {} } as never, undefined as never, undefined as never, r => { result = r; });
    pick(sheet as HandoffSheet);
    return result as T;
  } };
  const deps = { client, events: new EventLog(1), generation: 1, sessionId: () => "s", turn: () => undefined, ui: ui as never, hasUI: true };
  return { calls, deps };
}

test("local work without a prepared package offers only disabled actions with the reason", () => {
  const shape = sheetFor({ ...board, driver: "assistant", label: "Working together", task: null, contract_revision: undefined, phase: "ordinary" });
  expect(shape.actions.every(a => a.disabled === LOCAL_UNAVAILABLE)).toBe(true);
});

test("cancel changes nothing: no Choice journaled, no consumer call", async () => {
  const { calls, deps } = fake(board, s => s.handleInput("\x1b"));
  expect((await runHandoff(deps)).text).toContain("nothing changed");
  expect(calls.map(c => c.op)).toEqual(["assistant_handoff"]);
});

test("a selection journals a bound Choice, then the host-only consumer cites it", async () => {
  const { calls, deps } = fake(board, s => { s.handleInput("\x1b[B"); s.handleInput("\x1b[B"); s.handleInput("\r"); }); // Work on this here
  await runHandoff(deps);
  // Bound to the state the sheet displayed, never re-read after the selection.
  expect(calls.find(c => c.payload.action === "binding")!.payload.shown).toBe("v1");
  const input = calls.find(c => c.payload.kind === "owner_input")!.payload;
  expect(input).toMatchObject({ source: "choice", text: "work_on_this_here", binding: "d".repeat(64) });
  expect(calls.find(c => c.payload.kind === "handoff_choice")!.payload).toEqual({ kind: "handoff_choice", input: "A-1/g1/i5",
    native: { action: "take_back", task: "T-9", contract_revision: "c1" }, shown: "v1" });
});

test("no interactive UI never manufactures a choice", async () => {
  const { calls, deps } = fake(board, () => {});
  expect((await runHandoff({ ...deps, hasUI: false })).level).toBe("warning");
  expect(calls).toHaveLength(1);
});

test("a completed r3 local review after r4 is published reads stale, never current", () => {
  const line = planningLine({ plan_revision: 4, plan_stage: "ready",
    reviews: [{ review: "LR1", revision: 3, current_revision: 4, freshness: "stale", lifecycle: "completed" }] });
  expect(line).toBe("Plan r4 · ready · Local review r3 completed · stale (current r4) · advisory only");
  expect(planningLine(null)).toBeUndefined();
});

test("assistant-led work with a local offer binds a native Local action; conversation context stays unavailable", () => {
  const state: HandoffState = { driver: "assistant", label: "Working together", waiting: null, plan_only: false, phase: "ordinary",
    local: { project: "app", entry: "A-1/g1/i2", goal_text: "fix the cache", title: "fix the cache" } };
  const shape = sheetFor(state);
  expect(shape.actions.every(a => !a.disabled)).toBe(true);
  expect(shape.offContexts?.conversation).toBeDefined();
  expect(nativeFor(state, { action: "save_for_later", context: "task" })).toEqual({ action: "local", local: { project: "app",
    goal: "implementation_prepare", context: "task", save: true, title: "fix the cache", entry: "A-1/g1/i2" } });
  expect(nativeFor(state, { action: "continue_selected_goal", context: "minimal" })).toMatchObject({ local: { goal: "implementation_ship", save: false } });
  expect(nativeFor(state, { action: "save_for_later", context: "conversation" })).toBeUndefined();
});

test("saved plan offers goal-aware plan continuations; a selection binds corral's exact Start preview", async () => {
  const saved: HandoffState = { ...board, label: "Saved plan · Held", plan_only: true };
  expect(sheetFor(saved).actions.map(a => a.id)).toEqual(["save_for_later", "plan_continue", "plan_review_only", "work_on_this_here"]);
  expect(sheetFor({ ...board, label: "Saved for later · Held" }).actions.map(a => a.id))
    .toEqual(["save_for_later", "implementation_ship", "work_on_this_here"]);
  const calls: { op: string; payload: Record<string, unknown> }[] = [];
  const ok = (result: unknown) => ({ state: "committed", result }) as unknown as CorralReceipt;
  const choice = { transfer: "x", task: "T-9", revision: 2, contract_revision: "c1", goal: "plan_review_only", plan: null, code: null };
  const client = { call: async (op: string, c: { payload: Record<string, unknown> }) => {
    calls.push({ op, payload: c.payload });
    if (c.payload.action === "read") return ok(saved);
    if (c.payload.action === "continue_preview") return ok({ choice });
    if (c.payload.action === "binding") return ok({ option: "plan_review_only", digest: "d".repeat(64) });
    if (c.payload.kind === "owner_input") return ok({ input: "A-1/g1/i5" });
    return ok({ action: "start", task: "T-9" });
  } } as unknown as CorralClient;
  const ui = { custom: async <T,>(factory: (tui: never, t: never, k: never, done: (r: T) => void) => unknown) => {
    let result: T | undefined;
    const sheet = factory({ requestRender() {}, terminal: { rows: 40 } } as never, undefined as never, undefined as never, r => { result = r; }) as HandoffSheet;
    sheet.handleInput("\x1b[B"); sheet.handleInput("\x1b[B"); sheet.handleInput("\r"); // Review this plan
    return result as T;
  } };
  await runHandoff({ client, events: new EventLog(1), generation: 1, sessionId: () => "s", turn: () => undefined, ui: ui as never, hasUI: true });
  expect(calls.find(c => c.payload.action === "continue_preview")!.payload.goal).toBe("plan_review_only");
  expect(calls.find(c => c.payload.kind === "handoff_choice")!.payload.native).toEqual({ action: "start", choice });
});
