import { expect, test } from "bun:test";
import { historyUI } from "../../test/support/history-ui.ts";
import { historyArgs, type HistoryRequest } from "../tools/history.ts";
import { HistoryNavigator, type HistoryRead } from "./history.ts";
import { assistantRecap, historyEntries, nextHistoryPage, recapLines, sourceLines, taskRecap, transcriptPage } from "./history-model.ts";

/** A navigator whose host is busy while one of its own overlays is mounted, as the rich host's `canInteract` is. */
function navigator(capture: { readonly canInteract: () => boolean }, read: HistoryRead) {
  const notices: string[] = [];
  return { notices, history: new HistoryNavigator(read, capture.canInteract, text => { notices.push(text); }, () => ({})) };
}
const text = (lines: readonly (string | { readonly text: string })[]) => lines.map(line => typeof line === "string" ? line : line.text).join("\n");

test("history command vocabulary excludes replacement/restore and validates page and subjects", () => {
  expect(historyArgs({ kind: "task", task: "T-695" })).toEqual(["show", "T-695", "--json", "--recorded"]);
  expect(historyArgs({ kind: "recovery", task: "T-695", run: "R1" }).at(-1)).toBe("--list");
  for (const first of [0, -1, 1.5, Infinity, 1_000_001]) expect(() => historyArgs({ kind: "transcript", task: "T-1", run: "R1", first })).toThrow();
  expect(() => historyArgs({ kind: "task", task: "../../private" })).toThrow();
  expect(() => historyArgs({ kind: "recovery", task: "T-1", run: "--to" })).toThrow();
});

test("recap facts keep provenance apart: unknown is undefined, sources name the recorded field", () => {
  const unknown = taskRecap({}, {}, undefined);
  expect(unknown.every(fact => fact.value === undefined)).toBeTrue();
  const facts = taskRecap({ id: "T-1" }, { id: "R1", checkpoints: [{ checkpoint: { input: { next_action: "Inspect handler" }, captured: "then" } }] }, []);
  const next = facts.find(fact => fact.label === "next step");
  expect(next?.value).toBe("Inspect handler");
  expect(next?.provenance).toContain("run.checkpoints[-1]");
  const recap = text(recapLines(facts));
  expect(recap).toContain("Inspect handler");
  for (const fact of facts) expect(recap).not.toContain(fact.provenance);
  // Wrapped values hang under the value column: the hanging prefix is exactly the label column.
  recapLines(facts).forEach((line, i) => {
    expect(line.text.slice(0, line.hanging).trim()).toBe(facts[i]?.label);
    expect(line.text.slice(line.hanging ?? 0)).toBe(facts[i]?.value ?? "not recorded");
  });
  const sources = text(sourceLines(facts, { binding: { session: "s-1" } }));
  for (const fact of facts) expect(sources).toContain(fact.provenance);
  expect(sources).toContain('"session": "s-1"');
  expect(nextHistoryPage("[page byte limit; resume with --lines 4-203]", 1)).toBe(4);
  expect(nextHistoryPage("ordinary page", 201)).toBe(401);
});

test("a failed question read is stated, not read as nothing pending", () => {
  const waiting = taskRecap({ id: "T-1" }, { id: "R1" }, new Error("database is locked")).find(fact => fact.label === "waiting on");
  expect(waiting?.value).toContain("database is locked");
});

test("assistant recap uses only recorded ledger and question facts, with ended closure", () => {
  const container = { state: { questions: [{ state: "open", text: "Actual question" }], todo: { phases: [{ steps: [{ id: "next", label: "Recorded work", status: { state: "pending" } }] }] } } };
  const waiting = (session: Record<string, unknown>) => assistantRecap(session, container).find(fact => fact.label === "waiting on")?.value;
  expect(waiting({})).toContain("Actual question");
  expect(waiting({ ended: "yesterday" })).not.toContain("Actual question");
  expect(assistantRecap({}, container).find(fact => fact.label === "next step")?.value).toContain("Recorded work");
  expect(assistantRecap({}, container).find(fact => fact.label === "outcome")?.value).toBeUndefined();
});

test("lists keep the 200-row bound and say how many were returned; assistants filter by words locally", () => {
  const many = Array.from({ length: 250 }, (_, i) => ({ id: `T-${i}`, title: `task ${i}` }));
  const tasks = historyEntries("tasks", many, "ignored by the local model");
  expect(tasks.rows).toHaveLength(200);
  expect(tasks.returned).toBe(250);
  const assistants = historyEntries("assistants", [{ session: "A-1", focus: "board polish" }, { session: "A-2", focus: "usage" }], "polish");
  expect(assistants.rows.map(row => row.id)).toEqual(["A-1"]);
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
      case "task": return { id: "T-1", runs: [{ id: "R1", ended: "yesterday", summary: "Recorded legacy summary" }] };
      case "questions": return [];
      case "transcript": return request.first === 1
        ? '{"type":"message","message":{"role":"user","content":"First question"}}\n[page byte limit; resume with --lines 4-203]\n'
        : '{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Later answer"}]}}\n';
      default: throw new Error("Unexpected operation");
    }
  });
  const before = capture.ui.getEditorText();
  await history.open(capture.ui);
  expect(calls.map(call => call.kind)).toEqual(["tasks", "task", "questions", "transcript", "transcript", "transcript"]);
  expect(calls.filter(call => call.kind === "transcript").map(call => call.first)).toEqual([1, 4, 1]);
  const frames = capture.frames.join("\n");
  expect(frames).toContain("Recorded legacy summary");
  expect(frames).toContain("run.summary");
  expect(frames).toContain("First question");
  expect(frames).toContain('"role":"user"');
  expect(frames).toContain("Later answer");
  expect(capture.ui.getEditorText()).toBe(before);
  expect(notices).toEqual([]);
});

test("ended assistants use recorded summary, never load a session file and offer no transcript", async () => {
  const calls: string[] = [], capture = historyUI([1, 0], ["t", "c", "\x1b"]);
  const { history } = navigator(capture, async request => {
    calls.push(request.kind);
    return request.kind === "assistants" ? [{ session: "A-1", focus: "past" }] : {
      session: { id: "A-1", ended: "yesterday", summary: "Owner recorded summary" }, container: { state: { generations: [{ n: 1 }] } },
    };
  });
  await history.open(capture.ui);
  expect(calls).toContain("assistant");
  for (const kind of ["task", "questions", "transcript", "recovery"]) expect(calls).not.toContain(kind);
  expect(capture.frames.join("\n")).toContain("Owner recorded summary");
});

test("checkpoint inspection uses only the existing list API and renders recovery as recorded", async () => {
  const capture = historyUI([0, 0, 0], ["c", "\x1b", "\x1b"]), calls: string[] = [];
  const { history } = navigator(capture, async request => {
    calls.push(request.kind);
    if (request.kind === "tasks") return [{ id: "T-1" }];
    if (request.kind === "task") return { id: "T-1", runs: [{ id: "R1", agent_host: { generations: [{ n: 2 }], rewinds: [{ state: "aborted" }] } }] };
    if (request.kind === "questions") return [];
    if (request.kind === "recovery") return { checkpoints: [], rewinds: [{ state: "aborted" }], pending: null };
    throw new Error("Unexpected operation");
  });
  await history.open(capture.ui);
  expect(calls).toEqual(["tasks", "task", "questions", "recovery"]);
  expect(capture.frames.join("\n")).toContain("aborted");
  expect(capture.frames.join("\n")).not.toContain("Restore now");
});

test("a committed board search keeps its results even when they match on fields the list does not show", async () => {
  const calls: HistoryRequest[] = [], capture = historyUI([0, "migration", 0, 0]);
  const { history } = navigator(capture, async request => {
    calls.push(request);
    if (request.kind === "tasks") return request.query ? [{ id: "T-7", title: "Body match only" }] : [{ id: "T-1", title: "other" }];
    if (request.kind === "task") return { id: request.task, runs: [{ id: "R1" }] };
    if (request.kind === "questions") return [];
    throw new Error("Unexpected operation");
  });
  await history.open(capture.ui);
  expect(calls.map(call => call.kind === "tasks" ? `tasks:${call.query}` : call.kind === "task" ? `task:${call.task}` : call.kind))
    .toEqual(["tasks:", "tasks:migration", "task:T-7", "questions"]);
});

test("a failed list read is shown inside the popup, not as a notice", async () => {
  const capture = historyUI([0]);
  const { history, notices } = navigator(capture, async () => { throw new Error("database is locked"); });
  await history.open(capture.ui);
  expect(capture.frames.join("\n")).toContain("database is locked");
  expect(notices).toEqual([]);
});

test("missing assistant record is an explicit failed read", async () => {
  const capture = historyUI([1, 0]);
  const { history, notices } = navigator(capture, async request => {
    if (request.kind === "assistants") return [{ session: "A-1" }];
    throw new Error("assistant not found");
  });
  await history.open(capture.ui);
  expect(notices.join()).toContain("assistant not found");
});

test("missing transcript is unavailable, not an invented recap or successful empty history", async () => {
  const capture = historyUI([0, 0, 0], ["t", "\x1b"]);
  const { history, notices } = navigator(capture, async request => {
    if (request.kind === "tasks") return [{ id: "T-1" }];
    if (request.kind === "task") return { id: "T-1", runs: [{ id: "R1" }] };
    if (request.kind === "questions") return [];
    throw new Error("transcript_unavailable: no session locator");
  });
  await history.open(capture.ui);
  expect(notices.join()).toContain("transcript_unavailable");
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
