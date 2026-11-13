import { expect, test } from "bun:test";
import type { BuildPanelState, ChildSnapshot, FleetControl, FleetSnapshot } from "../../build/types.ts";
import { childHue, childKey } from "./contract.ts";
import {
  childActions, childAuthors, childRow, childRows, childSpans, dependents, fleetExtras, modelChoices, type ChildActionHost,
} from "./fleet-rows.ts";

const ORIGIN = 1_000_000, NOW = ORIGIN + 120_000;
function child(id: string, extra: Partial<ChildSnapshot> = {}): ChildSnapshot {
  return { id, call: "call-1", title: `Deliver ${id}`, brief: `Brief for ${id}`, owns: [`src/${id}`], after: [], profile: "default",
    model: "anthropic/model-a", state: "running", createdAt: ORIGIN, startedAt: ORIGIN + 1000, edits: [],
    usage: { input: 10, output: 5, turns: 1 }, steers: [], pulse: [1, 2], history: "r", messages: [], ...extra };
}
function fleet(children: readonly ChildSnapshot[]): FleetSnapshot { return { children, maxParallel: 3, startedAt: ORIGIN }; }
function control(snapshot: () => FleetSnapshot, calls: string[][]): FleetControl {
  return {
    snapshot, steer: async () => true, answer: () => true, subscribe: () => () => {},
    cancel(id) { calls.push(["cancel", id]); return true; },
    setModel(id, model) { calls.push(["setModel", id, model]); },
    models: () => ["anthropic/model-a", "anthropic/model-deep", "openai/model-b"],
    profiles: () => [{ name: "default", model: "anthropic/model-a" }, { name: "deep", model: "anthropic/model-deep" }],
  };
}
function actionHost(fleetControl: FleetControl, calls: string[][], answers: { confirm: boolean; select?: string }): ChildActionHost {
  return {
    fleet: fleetControl,
    point(id) { calls.push(["point", id]); },
    question(id) { calls.push(["question", id]); },
    async confirm(title, message) { calls.push(["confirm", title, message]); return answers.confirm; },
    async select(title) { calls.push(["select", title]); return answers.select; },
    notice(text, tone) { calls.push(["notice", tone, text]); },
  };
}
const flush = async () => { for (let index = 0; index < 5; index++) await Promise.resolve(); };

test("a child with no price reports an unknown cost, never zero, and files counts distinct paths", () => {
  const edits = [{ path: "src/a.ts", outside: false, at: ORIGIN }, { path: "src/a.ts", outside: false, at: ORIGIN + 1 }, { path: "src/b.ts", outside: false, at: ORIGIN + 2 }];
  const row = childRow(child("slots", { edits }), fleet([child("slots", { edits })]), NOW);
  expect(row.usage?.cost).toBeUndefined();
  expect(row.usage?.contextPercent).toBeUndefined();
  expect(row.files).toBe(2);
  expect(row.outside).toBe(0);
  expect(row.notes.some(note => note.tone === "warning")).toBeFalse();
});

test("an edit outside the child's files sets outside and a warning note naming the path", () => {
  const edits = [{ path: "src/other.ts", outside: true, at: ORIGIN }, { path: "src/other.ts", outside: true, at: ORIGIN + 1 }, { path: "src/slots/a.ts", outside: false, at: ORIGIN + 2 }];
  const row = childRow(child("slots", { edits }), fleet([]), NOW);
  expect(row.files).toBe(2);
  expect(row.outside).toBe(1);
  expect(row.notes.some(note => note.tone === "warning" && note.text.includes("src/other.ts"))).toBeTrue();
});

test("history becomes wait, run, ask and run spans with a done end, and a truncated history anchors to its end", () => {
  const done = childSpans(child("slots", { state: "done", history: "qqqrrrnnrrd" }), ORIGIN, NOW);
  expect(done.spans).toEqual([
    { kind: "wait", from: ORIGIN, to: ORIGIN + 15_000 }, { kind: "run", from: ORIGIN + 15_000, to: ORIGIN + 30_000 },
    { kind: "ask", from: ORIGIN + 30_000, to: ORIGIN + 40_000 }, { kind: "run", from: ORIGIN + 40_000, to: ORIGIN + 50_000 },
  ]);
  expect(done.end).toEqual({ at: ORIGIN + 50_000, outcome: "done" });

  const later = "  qr";
  const created = childSpans(child("late", { history: later }), ORIGIN, NOW);
  expect(created.spans).toEqual([{ kind: "wait", from: ORIGIN + 10_000, to: ORIGIN + 15_000 }, { kind: "run", from: ORIGIN + 15_000 }]);
  expect(created.end).toBeUndefined();

  const endedAt = ORIGIN + 2_000_000;
  const truncated = childSpans(child("long", { state: "failed", endedAt, history: "r".repeat(239) + "f" }), ORIGIN, endedAt + 60_000);
  expect(truncated.spans).toEqual([{ kind: "run", from: ORIGIN + 161 * 5000, to: endedAt }]);
  expect(truncated.end).toEqual({ at: endedAt, outcome: "failed" });

  const running = childSpans(child("long", { history: "r".repeat(240) }), ORIGIN, endedAt);
  expect(running.spans).toEqual([{ kind: "run", from: ORIGIN + 161 * 5000 }]);
});

test("finished children offer steer, cancel and model dim as not running; a queued one cannot be steered yet", () => {
  for (const state of ["done", "failed", "cancelled"] as const) {
    const row = childRow(child("slots", { state, endedAt: NOW }), fleet([]), NOW);
    expect(row.disabled).toEqual({ steer: "not running", cancel: "not running", model: "not running" });
  }
  expect(childRow(child("slots", { state: "queued", startedAt: undefined }), fleet([]), NOW).disabled).toEqual({ steer: "not started yet" });
  expect(childRow(child("slots"), fleet([]), NOW).disabled).toBeUndefined();
});

test("the answer capability is offered only while the child asks", () => {
  const question = { question: "Which layout?", options: [{ label: "Grid" }, { label: "List" }], askedAt: NOW - 1000 };
  for (const state of ["queued", "running", "needs", "done", "failed", "cancelled"] as const) {
    const row = childRow(child("slots", { state, ...(state === "needs" ? { question } : {}) }), fleet([]), NOW);
    expect(row.capabilities.answer).toBe(state === "needs");
  }
});

test("a child keeps its hue when a later child finishes or another is added", () => {
  const first = fleet([child("a"), child("b")]);
  const second = fleet([child("a"), child("b", { state: "done", endedAt: NOW }), child("c")]);
  expect(childHue(first, "b")).toBe(1);
  expect(childHue(second, "b")).toBe(1);
  expect(childRows(second, NOW).map(row => [row.key, row.hue])).toEqual([[childKey("a"), 0], [childKey("b"), 1], [childKey("c"), 2]]);
});

test("dependents names live children that wait on the child, transitively, and no finished ones", () => {
  const snapshot = fleet([
    child("a"),
    child("b", { state: "queued", after: ["a"] }),
    child("c", { state: "queued", after: ["b"] }),
    child("d", { state: "done", after: ["a"], endedAt: NOW }),
    child("e", { state: "queued", after: ["d"] }),
    child("f", { state: "queued" }),
  ]);
  expect(dependents(snapshot, "a")).toEqual(["b", "c"]);
  expect(dependents(snapshot, "c")).toEqual([]);
});

test("cancel asks first and never cancels when the owner declines; non-child keys reach no action", async () => {
  const calls: string[][] = [], snapshot = fleet([child("a"), child("b", { state: "queued", after: ["a"] })]);
  const declined = childActions(actionHost(control(() => snapshot, calls), calls, { confirm: false }));
  declined.cancel(childKey("a"));
  await flush();
  expect(calls.map(call => call[0])).toEqual(["confirm"]);
  expect(calls[0]?.[2]).toContain("b");

  calls.length = 0;
  const confirmed = childActions(actionHost(control(() => snapshot, calls), calls, { confirm: true }));
  confirmed.cancel(childKey("a"));
  await flush();
  expect(calls.filter(call => call[0] === "cancel")).toEqual([["cancel", "a"]]);

  calls.length = 0;
  const any = childActions(actionHost(control(() => snapshot, calls), calls, { confirm: true, select: "deep · anthropic/model-deep" }));
  for (const key of ["helper:a", "job:a", "a"]) { any.steer(key); any.cancel(key); any.model(key); any.answer(key); }
  await flush();
  expect(calls).toEqual([]);
});

test("model choices list profiles first, then uncovered models, marking the current and the pending one", async () => {
  const calls: string[][] = [];
  const subject = child("a", { profile: "deep", model: "anthropic/model-deep", pendingModel: "openai/model-b" });
  const fleetControl = control(() => fleet([subject]), calls);
  const choices = modelChoices(fleetControl, subject);
  expect(choices.map(choice => choice.value)).toEqual(["default", "deep", "openai/model-b"]);
  expect(choices[1]?.label.endsWith(" · current")).toBeTrue();
  expect(choices[2]?.label.endsWith(" · pending")).toBeTrue();
  expect(choices[0]?.label.endsWith(" · current") || choices[0]?.label.endsWith(" · pending")).toBeFalse();

  const actions = childActions(actionHost(fleetControl, calls, { confirm: true, select: choices[0]?.label ?? "" }));
  actions.model(childKey("a"));
  await flush();
  expect(calls.filter(call => call[0] === "setModel")).toEqual([["setModel", "a", "default"]]);
});

test("Changes authors OR the outside flag across repeated edits and leave out children without edits", () => {
  const snapshot = fleet([
    child("quiet"),
    child("slots", { edits: [{ path: "x.ts", outside: false, at: 1 }, { path: "x.ts", outside: true, at: 2 }, { path: "x.ts", outside: false, at: 3 }, { path: "y.ts", outside: false, at: 4 }] }),
  ]);
  const authors = childAuthors(snapshot);
  expect(authors.map(author => [author.id, author.hue])).toEqual([["slots", 1]]);
  expect([...authors[0]?.paths ?? []]).toEqual([["x.ts", true], ["y.ts", false]]);
});

test("fleetExtras is absent outside build and carries the fleet's rows, parallel cap and origin inside it", () => {
  expect(fleetExtras(undefined, NOW)).toBeUndefined();
  const state: BuildPanelState = {
    session: { version: 1, id: "B-1", name: "save-slots", created: "2026-01-01T00:00:00Z", updated: "2026-01-01T00:00:00Z", repoRoot: "/repo",
      worktree: "/repo/.worktrees/build-save-slots", branch: "build/save-slots", base: null, here: false },
    settings: { exitPolicy: "gate", maxParallel: 3, profiles: [] },
    fleet: fleet([child("a"), child("b", { state: "queued", after: ["a"] })]), target: "parent", exit: { phase: "none" }, board: [],
  };
  const extras = fleetExtras(state, NOW);
  expect(extras?.agents.map(row => row.id)).toEqual(["a", "b"]);
  expect(extras?.maxParallel).toBe(3);
  expect(extras?.origin).toBe(ORIGIN);
});
