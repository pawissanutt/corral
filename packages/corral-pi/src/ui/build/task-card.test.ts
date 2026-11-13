import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { ChildSnapshot, ChildSpec, FleetSnapshot } from "../../build/types.ts";
import { agentColor } from "../agent-hues.ts";
import { taskCardLines, taskCardRows, taskTotals } from "./task-card.ts";

const NOW = 2_000_000;
function child(id: string, state: ChildSnapshot["state"], extra: Partial<ChildSnapshot> = {}): ChildSnapshot {
  return {
    id, call: "call-1", title: `Deliver the ${id} slice with a deliberately long title that has to be clipped`, brief: "brief",
    owns: [`src/${id}`], after: [], profile: "frontend", model: "anthropic/claude-sonnet-4.5", state, createdAt: NOW - 90_000,
    ...(state === "queued" ? {} : { startedAt: NOW - 80_000 }),
    ...(state === "done" || state === "failed" || state === "cancelled" ? { endedAt: NOW - 10_000 } : {}),
    edits: [], usage: { input: 10, output: 20, turns: 1 }, steers: [], pulse: [], history: "", messages: [], ...extra,
  };
}
const spec = (id: string): ChildSpec => ({ id, title: `Slice ${id}`, brief: "b", owns: [] });
const args = { agents: [spec("host-slots"), spec("picker-ui")] };
const fleet = (children: ChildSnapshot[]): FleetSnapshot => ({ children, maxParallel: 3, startedAt: NOW - 100_000 });
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));

test("only children started by this tool call become rows", () => {
  const live = fleet([child("host-slots", "running"), child("other", "running", { call: "call-2" }), child("picker-ui", "queued")]);
  expect(taskCardRows(args, "call-1", live, undefined, NOW).map(row => row.id)).toEqual(["host-slots", "picker-ui"]);
  expect(taskCardRows(args, "call-2", live, undefined, NOW).map(row => row.id)).toEqual(["other"]);
});

test("after a resume the settled result supplies the rows, and with neither the outcome is unknown", () => {
  const settled = [child("host-slots", "done"), child("picker-ui", "failed", { error: "boom" })];
  const resumed = taskCardRows(args, "call-1", fleet([]), settled, NOW);
  expect(resumed.map(row => row.state)).toEqual(["done", "failed"]);
  const lost = taskCardRows(args, "call-1", fleet([]), undefined, NOW);
  expect(lost.map(row => row.state)).toEqual(["unknown", "unknown"]);
  const text = plain(taskCardLines(lost, 120, { expanded: false, color: agentColor }).lines).join("\n");
  expect(text).toContain("outcome unknown");
});

test("a row waiting for an answer carries the question and its hit says it needs the owner", () => {
  const asking = child("picker-ui", "needs", { question: { question: "Empty state?\nMore detail", options: [], askedAt: NOW - 5000 } });
  const rows = taskCardRows(args, "call-1", fleet([child("host-slots", "running"), asking]), undefined, NOW);
  const { lines, hits } = taskCardLines(rows, 120, { expanded: false, color: agentColor });
  const index = plain(lines).findIndex(line => line.includes("▲ Empty state?"));
  expect(index).toBeGreaterThan(0);
  expect(hits.get(index)).toEqual({ id: "picker-ui", needs: true });
  const running = plain(lines).findIndex(line => line.includes("host-slots"));
  expect(hits.get(running)).toEqual({ id: "host-slots", needs: false });
});

test("every line fits its width at every density", () => {
  const children = [
    child("host-slots", "running", { activity: "edit crates/host/src/slots.rs and a long tail of arguments" }),
    child("picker-ui-with-a-long-id", "needs", { question: { question: "A long question ".repeat(10), options: [], askedAt: NOW } }),
    child("app-store", "done", { usage: { input: 1, output: 1, turns: 1, cost: 0.12 }, edits: [{ path: "a.ts", outside: true, at: NOW }] }),
    child("tests", "cancelled"), child("docs", "queued"),
  ];
  const rows = taskCardRows(args, "call-1", fleet(children), undefined, NOW);
  for (const width of [30, 40, 59, 60, 111, 112, 140]) {
    for (const expanded of [false, true]) {
      const { lines } = taskCardLines(rows, width, { expanded, color: agentColor, totals: taskTotals(children) });
      for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
      expect(lines.length).toBeGreaterThan(children.length);
    }
  }
});

test("totals never claim a zero cost when every cost is unknown", () => {
  const children = [child("a", "done"), child("b", "failed")];
  const totals = taskTotals(children);
  expect(totals.cost).toBeUndefined();
  const text = plain(taskCardLines([], 120, { expanded: false, color: agentColor, totals }).lines).join("\n");
  expect(text).not.toContain("$0.00");
  expect(text).toContain("1 failed");
});

test("files count distinct paths and outside counts paths edited outside their owner's files", () => {
  const children = [
    child("a", "done", { edits: [{ path: "x.ts", outside: false, at: 1 }, { path: "x.ts", outside: false, at: 2 }, { path: "y.ts", outside: true, at: 3 }] }),
    child("b", "done", { edits: [{ path: "y.ts", outside: true, at: 4 }] }),
  ];
  expect(taskTotals(children)).toMatchObject({ done: 2, files: 2, outside: 1 });
});
