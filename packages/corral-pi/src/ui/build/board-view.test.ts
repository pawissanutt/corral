import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { BoardPhase, BoardPlan, BoardTaskStatus } from "../../build/types.ts";
import { boardDetailLines, boardStage, BoardView, renderBoard, type BoardAction, type BoardViewHost } from "./board-view.ts";

const NOW = 1_000_000;
const PHASES: readonly BoardPhase[] = ["backlog", "waiting", "queued", "running", "gating", "reviewing", "landed", "blocked", "failed"];
const task = (id: string, phase: BoardPhase, extra: Partial<BoardTaskStatus> = {}): BoardTaskStatus =>
  ({ id, title: `Task ${id}`, status: "Ready", phase, depends: [], updatedAt: NOW - 12_000, ...extra });
const plan: BoardPlan = {
  title: "Split the save-slot export", why: "Too large for one build",
  tasks: [{ key: "a", title: "Host export", depends: [], body: "" }, { key: "b", title: "Picker", depends: ["a"], body: "" }],
};

function host(tasks: BoardTaskStatus[], options: { proposed?: BoardPlan; unavailable?: string } = {}) {
  const acted: { action: BoardAction; id?: string }[] = [];
  const value: BoardViewHost = {
    tasks: () => tasks, proposed: () => options.proposed, unavailable: () => options.unavailable,
    now: () => NOW, short: () => false, click: () => true, focus() {},
    act(action, id) { acted.push(id === undefined ? { action } : { action, id }); },
  };
  return { value, acted };
}

test("every board phase maps to a glyph and a word, and blocked and failed carry their detail", () => {
  const words = new Set<string>();
  for (const phase of PHASES) {
    const stage = boardStage(task("T-1", phase, { detail: `reason for ${phase}` }), []);
    expect(stage.glyph.length).toBeGreaterThan(0);
    expect(stage.word.length).toBeGreaterThan(0);
    words.add(stage.word);
  }
  expect(words.size).toBe(PHASES.length);
  expect(boardStage(task("T-1", "blocked", { detail: "needs an owner decision" }), []).now).toContain("needs an owner decision");
  expect(boardStage(task("T-1", "failed", { detail: "just check failed" }), []).now).toContain("just check failed");
  expect(boardStage(task("T-1", "blocked", { detail: "needs an owner decision" }), []).tone).toBe("warning");
  expect(boardStage(task("T-1", "failed"), []).tone).toBe("error");
  // A landed dependency no longer counts as waited for.
  const tasks = [task("T-1", "landed"), task("T-2", "running"), task("T-3", "waiting", { depends: ["T-1", "T-2"] })];
  const waits = boardStage(tasks[2]!, tasks).now;
  expect(waits).toContain("T-2"); expect(waits).not.toContain("T-1");
  expect(boardDetailLines(task("T-1", "failed", { detail: "just check failed" }), NOW).join("\n")).toContain("just check failed");
});

test("the selection stays on the same task when a task is inserted above it", () => {
  const tasks = [task("T-1", "running"), task("T-2", "queued"), task("T-3", "backlog")];
  const { value } = host(tasks);
  const view = new BoardView(value);
  view.move(1);
  expect(view.task()?.id).toBe("T-2");
  tasks.unshift(task("T-0", "landed"));
  expect(view.task()?.id).toBe("T-2");
  const frame = renderBoard(tasks, undefined, view.selected, 112, { now: NOW, short: false });
  expect(stripTerminalSequences(frame.lines[frame.selectedRow] ?? "")).toContain("T-2");
  view.move(1);
  expect(view.task()?.id).toBe("T-3");
});

test("the plan row appears only while a plan waits, and its hit and p review it", () => {
  const tasks = [task("T-1", "running")];
  const reviewRow = (proposed?: BoardPlan) => {
    const frame = renderBoard(tasks, proposed, undefined, 80, { now: NOW, short: false });
    return [...frame.hits.entries()].find(([, hits]) => hits.some(hit => hit.action === "review"));
  };
  expect(reviewRow()).toBeUndefined();
  const row = reviewRow(plan);
  expect(row).toBeDefined();
  const frame = renderBoard(tasks, plan, undefined, 80, { now: NOW, short: false });
  expect(stripTerminalSequences(frame.lines[row![0]] ?? "")).toContain(plan.title);

  const waiting = host(tasks, { proposed: plan });
  const view = new BoardView(waiting.value);
  expect(view.handleKey("p")).toBeTrue();
  expect(waiting.acted).toEqual([{ action: "review" }]);
  const none = host(tasks);
  expect(new BoardView(none.value).handleKey("p")).toBeFalse();
  expect(none.acted).toHaveLength(0);
});

test("outside a workspace the empty state states the reason and offers no plan action", () => {
  const reason = "This session is outside a corral workspace";
  const frame = renderBoard([], undefined, undefined, 80, { now: NOW, short: false, unavailable: reason });
  expect(frame.lines.map(line => stripTerminalSequences(line)).join("\n")).toContain(reason);
  expect([...frame.hits.values()].flat().some(hit => hit.action === "plan")).toBeFalse();
  const outside = host([], { unavailable: reason });
  const view = new BoardView(outside.value);
  expect(view.actions()).not.toContain("plan");
  expect(view.handleKey("n")).toBeFalse();
  expect(outside.acted).toHaveLength(0);

  const inside = renderBoard([], undefined, undefined, 80, { now: NOW, short: false });
  expect([...inside.hits.values()].flat().some(hit => hit.action === "plan")).toBeTrue();
});

test("every board line fits its width at every density, short or not", () => {
  const long = "a-very-long-task-identifier";
  const tasks = PHASES.map((phase, index) => task(index === 0 ? long : `T-97${index}`, phase, {
    title: "Host: witness-scoped slot export with a title much longer than any column it could be given",
    detail: "corral refused to queue this task because its dependency T-971 has an open hold that the owner must clear first",
    depends: index > 1 ? ["T-971", "T-973"] : [],
  }));
  const cases: { tasks: BoardTaskStatus[]; proposed?: BoardPlan; unavailable?: string }[] = [
    { tasks }, { tasks, proposed: plan }, { tasks: [] }, { tasks: [], proposed: plan },
    { tasks: [], unavailable: "outside a corral workspace, so nothing from this session can reach a board" },
  ];
  for (const item of cases) for (const width of [34, 52, 80, 112, 140]) for (const short of [false, true]) {
    for (const selected of [undefined, "T-978", long]) {
      const frame = renderBoard(item.tasks, item.proposed, selected, width, {
        now: NOW, short, ...(item.unavailable === undefined ? {} : { unavailable: item.unavailable }),
      });
      for (const line of frame.lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
      for (const hits of frame.hits.values()) for (const hit of hits) if (hit.to !== undefined) expect(hit.to).toBeLessThanOrEqual(width);
    }
  }
});
