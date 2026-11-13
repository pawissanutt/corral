import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { ChildSnapshot, ChildSpec, FleetSnapshot } from "../../build/types.ts";
import { agentColor } from "../agent-hues.ts";
import { CARD_MAX, finishedLines, taskCardLines, taskCardRows } from "./task-card.ts";

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
  const text = plain(taskCardLines(lost, 120, { expanded: false, color: agentColor, now: NOW }).lines).join("\n");
  expect(text).toContain("outcome unknown");
});

test("the card is a rounded box headed by the call, one row per slice with its hit, no summary row", () => {
  const asking = child("picker-ui", "needs", { question: { question: "Empty state?", options: [], askedAt: NOW - 5000 } });
  const rows = taskCardRows(args, "call-1", fleet([child("host-slots", "running"), asking]), undefined, NOW);
  const { lines, hits } = taskCardLines(rows, 140, { expanded: false, color: agentColor, now: NOW });
  const text = plain(lines);
  expect(text[0]?.startsWith("╭")).toBeTrue();
  expect(text[text.length - 1]?.startsWith("╰")).toBeTrue();
  expect(text[1]).toContain("task · 2 slices, one worktree");
  expect(text).toHaveLength(2 + rows.length + 1);
  const running = text.findIndex(line => line.includes("host-slots"));
  expect(hits.get(running)).toEqual({ id: "host-slots", needs: false });
  const needs = text.findIndex(line => line.includes("picker-ui"));
  expect(hits.get(needs)).toEqual({ id: "picker-ui", needs: true });
  expect(text[needs]).toContain("needs you");
  // The running row's state is a braille spinner frame, not a dot.
  expect(text[running]).toMatch(/│ [⠋⠙⠹⠸⠼⠴⠦⠧]  host-slots/);
});

test("columns follow the mini grid per density and the card is at most 108 cells wide", () => {
  const rows = taskCardRows(args, "call-1", fleet([child("host-slots", "done"), child("picker-ui", "queued")]), undefined, NOW);
  const at = (width: number) => plain(taskCardLines(rows, width, { expanded: false, color: agentColor, now: NOW }).lines);
  const full = at(160);
  expect(visibleWidth(full[0] ?? "")).toBe(CARD_MAX);
  // Name column of 11 cells: the title starts two cells after it.
  expect(full[2]?.indexOf("Deliver")).toBe((full[2]?.indexOf("host-slots") ?? 0) + 11 + 2);
  expect(full[2]).toContain("frontend · claude-sonnet-4.5");
  const med = at(80);
  expect(med[2]).toContain("frontend");
  expect(med[2]).not.toContain("claude-sonnet");
  const min = at(50);
  expect(min[1]).not.toContain("one worktree");
  expect(min[2]).not.toContain("frontend");
  expect(min[3]).toContain("waiting");
});

test("every line fits its width at every density", () => {
  const children = [
    child("host-slots", "running", { activity: "edit crates/host/src/slots.rs and a long tail of arguments" }),
    child("picker-ui-with-a-long-id", "needs", { question: { question: "A long question ".repeat(10), options: [], askedAt: NOW } }),
    child("app-store", "done", { usage: { input: 1, output: 1, turns: 1, cost: 0.12 }, edits: [{ path: "a.ts", outside: true, at: NOW }] }),
    child("tests", "cancelled"), child("docs", "queued"),
  ];
  const rows = taskCardRows(args, "call-1", fleet(children), undefined, NOW);
  for (const width of [30, 40, 46, 52, 59, 60, 92, 111, 112, 140, 185]) {
    for (const expanded of [false, true]) {
      const { lines } = taskCardLines(rows, width, { expanded, color: agentColor, now: NOW });
      for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
      expect(lines.length).toBeGreaterThan(children.length);
    }
    for (const line of finishedLines(children, fleet(children), width, agentColor).lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
  }
});

test("each slice that came back posts one line in the order they ended, files counting distinct paths", () => {
  const children = [
    child("a", "done", { endedAt: NOW - 1000, report: "## tsc clean\nmore", edits: [{ path: "x.ts", outside: false, at: 1 }, { path: "x.ts", outside: false, at: 2 }, { path: "y.ts", outside: true, at: 3 }] }),
    child("b", "failed", { endedAt: NOW - 5000, error: "boom" }),
    child("c", "running"),
  ];
  const { lines, hits } = finishedLines(children, fleet(children), 120, agentColor);
  const text = plain(lines);
  expect(text).toEqual(["✗ b failed · boom", "✓ a finished · 1 file + 1 outside its set · tsc clean"]);
  expect(hits.get(1)).toEqual({ id: "a", needs: false });
});
