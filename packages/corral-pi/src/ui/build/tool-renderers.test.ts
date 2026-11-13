import { expect, test } from "bun:test";
import { Box, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { BuildPanelState, ChildSnapshot, ExitState, FleetSnapshot } from "../../build/types.ts";
import { agentColor } from "../agent-hues.ts";
import { boardEntry, buildEntryRenderer, exitEntry, finishEntry } from "./build-entries.ts";
import type { BuildCardSource } from "./contract.ts";
import { buildToolRenderers } from "./tool-renderers.ts";

const NOW = 4_000_000;
function child(id: string, state: ChildSnapshot["state"]): ChildSnapshot {
  return {
    id, call: "call-1", title: `Slice ${id}`, brief: "b", owns: [], after: [], profile: "default", model: "p/m", state,
    createdAt: NOW - 60_000, startedAt: NOW - 50_000, ...(state === "running" ? {} : { endedAt: NOW - 1000 }),
    edits: [], usage: { input: 1, output: 1, turns: 1 }, steers: [], pulse: [], history: "", messages: [],
  };
}

function fakeSource(fleet: () => FleetSnapshot) {
  const watched: string[] = [];
  let released = 0;
  // The cards read only `fleet` and `exit` from the panel state.
  const state = () => ({ fleet: fleet(), exit: { phase: "none" } }) as unknown as BuildPanelState;
  const source: BuildCardSource = {
    state, summary: () => undefined, latestFinish: () => undefined, color: agentColor, now: () => NOW,
    watch(id) { watched.push(id); return () => { released += 1; }; },
    question() {}, exit() {}, plan() {}, openChild() {},
  };
  return { source, watched, released: () => released };
}
function context(args: unknown, state: object = {}) {
  return { args, toolCallId: "call-1", invalidate() {}, lastComponent: undefined, state, cwd: "/", executionStarted: true,
    argsComplete: true, isPartial: false, expanded: false, showImages: false, isError: false, durationMs: undefined, outputPad: 0,
    keyText: () => "", keyHint: () => "" } as never;
}

test("renderers exist for exactly the three build tools", () => {
  const source = () => undefined;
  expect(buildToolRenderers("task", source)?.renderCall).toBeDefined();
  expect(buildToolRenderers("task", source)?.renderResult).toBeDefined();
  expect(buildToolRenderers("build_finish", source)?.renderCall).toBeDefined();
  expect(buildToolRenderers("board_plan", source)?.renderResult).toBeDefined();
  for (const name of ["agent_message", "agent_cancel", "read", "bash", "corral_task"]) expect(buildToolRenderers(name, source)).toBeUndefined();
});

test("the task card watches once per tool call and releases once every row settled and the result arrived", () => {
  let children = [child("host-slots", "running"), child("picker-ui", "running")];
  const fake = fakeSource(() => ({ children, maxParallel: 3, startedAt: NOW - 60_000 }));
  const renderers = buildToolRenderers("task", () => fake.source)!;
  const args = { agents: children.map(item => ({ id: item.id, title: item.title, brief: "b", owns: [] })) };
  const ctx = context(args);
  for (let index = 0; index < 3; index++) renderers.renderCall!(args, undefined as never, ctx);
  expect(fake.watched).toEqual(["call-1"]);

  const card = renderers.renderCall!(args, undefined as never, ctx);
  expect(stripTerminalSequences(card.render(100).join("\n"))).toContain("picker-ui");

  children = children.map(item => ({ ...item, state: "done" as const, endedAt: NOW - 1000 }));
  renderers.renderCall!(args, undefined as never, ctx);
  expect(fake.released()).toBe(0);

  const result = { content: [{ type: "text" as const, text: "## host-slots: done" }], details: { children } };
  renderers.renderResult!(result, { expanded: false, isPartial: false }, undefined as never, ctx);
  expect(fake.released()).toBe(1);
  renderers.renderCall!(args, undefined as never, ctx);
  renderers.renderResult!(result, { expanded: false, isPartial: false }, undefined as never, ctx);
  expect(fake.watched).toHaveLength(1);
  expect(fake.released()).toBe(1);
});

test("a result does not release the watch while a row is still live", () => {
  const children = [child("host-slots", "running")];
  const fake = fakeSource(() => ({ children, maxParallel: 3 }));
  const renderers = buildToolRenderers("task", () => fake.source)!;
  const args = { agents: [{ id: "host-slots", title: "t", brief: "b", owns: [] }] };
  const ctx = context(args);
  renderers.renderCall!(args, undefined as never, ctx);
  renderers.renderResult!({ content: [], details: {} }, { expanded: false, isPartial: false }, undefined as never, ctx);
  expect(fake.watched).toHaveLength(1);
  expect(fake.released()).toBe(0);
});

test("an exit entry tells the parent which exit the owner chose and how it ended", () => {
  const landed: ExitState = { phase: "landed", record: { choice: "land", at: "t", task: "T-962" }, detail: "T-962 landed at 3f2a9c1." };
  const entry = exitEntry({ choice: "land", label: "Land after the gate", detail: "d" }, landed);
  expect(entry.content).toContain("Land after the gate");
  expect(entry.content).toContain("T-962 landed at 3f2a9c1.");
  const failed: ExitState = { phase: "failed", record: { choice: "ship", at: "t" }, detail: "The gate failed for T-963" };
  const shipped = exitEntry({ choice: "ship", label: "Ship through the board", detail: "d" }, failed);
  expect(shipped.content).toContain("Ship through the board");
  expect(shipped.content).toContain("The gate failed for T-963");
  expect(shipped.display).toBeTrue();
});

test("conversation entries start in the same column as the task card and stay within the row", () => {
  const children = [child("alpha", "done"), child("beta", "done")];
  const fake = fakeSource(() => ({ children, maxParallel: 3, startedAt: NOW - 60_000 }));
  const args = { agents: children.map(item => ({ id: item.id, title: item.title, brief: "b", owns: [] })) };
  const firstColumn = (lines: readonly string[]) => stripTerminalSequences(lines[0] ?? "").search(/\S/);
  const kept: ExitState = { phase: "kept", record: { choice: "keep", at: "t" }, detail: "The worktree and branch are kept." };
  const entries = [
    finishEntry("finish-1"),
    exitEntry({ choice: "keep", label: "Keep on the branch", detail: "d" }, kept),
    boardEntry([{ id: "T-971", title: "Slot manifest", status: "Backlog", phase: "backlog", depends: [], updatedAt: NOW }], false),
  ];
  const renderer = buildEntryRenderer(() => fake.source);
  for (const width of [40, 80, 140]) {
    // Pi draws a tool card inside its tool box, which insets it by the output padding on both sides.
    const box = new Box(1, 0);
    box.addChild(buildToolRenderers("task", () => fake.source)!.renderCall!(args, undefined as never, context(args)));
    const card = box.render(width);
    for (const entry of entries) {
      const message = { role: "custom" as const, ...entry, timestamp: NOW };
      const lines = renderer(message, { expanded: false, outputPad: 1 } as never, undefined as never)!.render(width);
      expect(lines.length).toBeGreaterThan(0);
      expect(firstColumn(lines)).toBe(firstColumn(card));
      for (const line of [...lines, ...card]) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
    }
  }
});
