import { expect, test } from "bun:test";
import { historyUI } from "../../test/support/history-ui.ts";
import type { HistoryRequest } from "../tools/history.ts";
import { HistoryNavigator, type HistoryRead } from "./history.ts";
import { historyEntries, nextHistoryPage, recapLines, sourceLines, taskRecap, transcriptPage } from "./history-model.ts";

/** A navigator whose host is busy while one of its own overlays is mounted, as the rich host's `canInteract` is. */
function navigator(capture: { readonly canInteract: () => boolean }, read: HistoryRead) {
  const notices: string[] = [];
  return { notices, history: new HistoryNavigator(read, capture.canInteract, text => { notices.push(text); }, () => ({})) };
}
const text = (lines: readonly (string | { readonly text: string })[]) => lines.map(line => typeof line === "string" ? line : line.text).join("\n");
/** `corral show --json` for T-1 with the given runs. */
const show = (id: string, runs: readonly Record<string, unknown>[]) => ({ task: { id, title: "legacy", status: "done" }, runs, checks: [], artifacts: [] });

test("recap facts keep provenance apart: unknown is undefined, sources name the recorded field", () => {
  expect(taskRecap({}).every(fact => fact.value === undefined)).toBeTrue();
  const facts = taskRecap({ id: "R1", outcome: "done", summary: "Fixed the parser", profile: "pi-sol", model: "gpt-sol" });
  const recap = text(recapLines(facts));
  expect(recap).toContain("Fixed the parser"); expect(recap).toContain("pi-sol · gpt-sol");
  for (const fact of facts) expect(recap).not.toContain(fact.provenance);
  // Wrapped values hang under the value column: the hanging prefix is exactly the label column.
  recapLines(facts).forEach((line, i) => {
    expect(line.text.slice(0, line.hanging).trim()).toBe(facts[i]?.label);
    expect(line.text.slice(line.hanging ?? 0)).toBe(facts[i]?.value ?? "not recorded");
  });
  const sources = text(sourceLines(facts, { worktree: "/w" }));
  for (const fact of facts) expect(sources).toContain(fact.provenance);
  expect(sources).toContain('"worktree": "/w"');
  expect(nextHistoryPage("[page byte limit; resume with --lines 4-203]", 1)).toBe(4);
  expect(nextHistoryPage("ordinary page", 201)).toBe(401);
});

test("lists keep the 200-row bound and say how many were returned", () => {
  const tasks = historyEntries(Array.from({ length: 250 }, (_, i) => ({ id: `T-${i}`, title: `task ${i}` })));
  expect(tasks.rows).toHaveLength(200);
  expect(tasks.returned).toBe(250);
});

test("a transcript page leads with readable rows; the byte cut is stated and the exact JSONL kept", () => {
  const record = '{"type":"message","timestamp":"2026-11-12T14:02:00Z","message":{"role":"user","content":"First question\\nmore"}}';
  const page = transcriptPage(`${record}\n[page byte limit; resume with --lines 4-203]\n`);
  expect(page.cut).toBeTrue();
  expect(text(page.lines)).toContain("First question");
  expect(text(page.lines)).not.toContain('"role"');
  expect(text(page.raw)).toContain(record);
  expect(transcriptPage("").empty).toBeTrue();
});

test("list → task → run: recap, d sources, transcript n/p over the byte cut, without editing the draft", async () => {
  const calls: HistoryRequest[] = [];
  const capture = historyUI([0, 0, 0], ["d", "t", "d", "n", "p", "\x1b", "\x1b"]);
  const { history, notices } = navigator(capture, async request => {
    calls.push(request);
    switch (request.kind) {
      case "tasks": return [{ id: "T-1", title: "legacy" }];
      case "task": return show("T-1", [{ id: "R1", ended: "2026-11-12T14:00:00Z", summary: "Recorded legacy summary" }]);
      case "transcript": return request.first === 1
        ? '{"type":"message","message":{"role":"user","content":"First question"}}\n[page byte limit; resume with --lines 4-203]\n'
        : '{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Later answer"}]}}\n';
    }
  });
  const before = capture.ui.getEditorText();
  await history.open(capture.ui);
  expect(calls.map(call => call.kind)).toEqual(["tasks", "task", "transcript", "transcript", "transcript"]);
  expect(calls.filter(call => call.kind === "transcript").map(call => call.first)).toEqual([1, 4, 1]);
  const frames = capture.frames.join("\n");
  for (const part of ["T-1 / R1", "Recorded legacy summary", "run.summary", "First question", '"role":"user"', "Later answer"]) expect(frames).toContain(part);
  expect(capture.ui.getEditorText()).toBe(before);
  expect(notices).toEqual([]);
});

test("a task without runs shows its record and says no run was recorded", async () => {
  const capture = historyUI([0, 0, 0]);
  const { history } = navigator(capture, async request => request.kind === "tasks" ? [{ id: "T-2", title: "fresh" }] : show("T-2", []));
  await history.open(capture.ui);
  const frames = capture.frames.join("\n");
  expect(frames).toContain("no recorded runs");
  expect(frames).toContain("No run has been recorded for this task.");
});

test("a committed board search keeps its results even when they match on fields the list does not show", async () => {
  const calls: HistoryRequest[] = [], capture = historyUI([0, "migration", 0, 0]);
  const { history } = navigator(capture, async request => {
    calls.push(request);
    if (request.kind === "tasks") return request.query ? [{ id: "T-7", title: "Body match only" }] : [{ id: "T-1", title: "other" }];
    if (request.kind === "task") return show(request.task, [{ id: "R1" }]);
    throw new Error("Unexpected operation");
  });
  await history.open(capture.ui);
  expect(calls.map(call => call.kind === "tasks" ? `tasks:${call.query}` : call.kind === "task" ? `task:${call.task}` : call.kind))
    .toEqual(["tasks:", "tasks:migration", "task:T-7"]);
});

test("a failed list read is shown inside the popup, not as a notice", async () => {
  const capture = historyUI([0]);
  const { history, notices } = navigator(capture, async () => { throw new Error("database is locked"); });
  await history.open(capture.ui);
  expect(capture.frames.join("\n")).toContain("database is locked");
  expect(notices).toEqual([]);
});

test("missing transcript is unavailable, not an invented recap or successful empty history", async () => {
  const capture = historyUI([0, 0, 0], ["t", "\x1b"]);
  const { history, notices } = navigator(capture, async request => {
    if (request.kind === "tasks") return [{ id: "T-1" }];
    if (request.kind === "task") return show("T-1", [{ id: "R1" }]);
    throw new Error("The run has no Pi transcript");
  });
  await history.open(capture.ui);
  expect(notices.join()).toContain("The run has no Pi transcript");
});

test("disposal during asynchronous read closes the popup and opens nothing later", async () => {
  const capture = historyUI([0, 0, 0]), calls: string[] = [];
  let release: ((value: unknown) => void) | undefined;
  const { history, notices } = navigator(capture, request => {
    calls.push(request.kind);
    const { promise, resolve } = Promise.withResolvers<unknown>();
    release = resolve;
    return promise;
  });
  const opening = history.open(capture.ui);
  for (let i = 0; i < 4; i++) await Promise.resolve();
  history.dispose(); release?.([{ id: "T-1" }]);
  await opening;
  expect(capture.frames).toEqual([]);
  expect(calls).toEqual(["tasks"]);
  expect(notices).toEqual([]);
});
