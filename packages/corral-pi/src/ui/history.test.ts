import { expect, test } from "bun:test";
import { historyUI } from "../../test/support/history-ui.ts";
import { historyArgs, type HistoryRequest } from "../tools/history.ts";
import { HistoryNavigator, assistantRecap, historyRecap, nextHistoryPage } from "./history.ts";

test("history command vocabulary excludes replacement/restore and validates page and subjects", () => {
  expect(historyArgs({ kind: "task", task: "T-695" })).toEqual(["show", "T-695", "--json", "--recorded"]);
  expect(historyArgs({ kind: "recovery", task: "T-695", run: "R1" }).at(-1)).toBe("--list");
  for (const first of [0, -1, 1.5, Infinity, 1_000_001]) expect(() => historyArgs({ kind: "transcript", task: "T-1", run: "R1", first })).toThrow();
  expect(() => historyArgs({ kind: "task", task: "../../private" })).toThrow();
  expect(() => historyArgs({ kind: "recovery", task: "T-1", run: "--to" })).toThrow();
});

test("unknown recap is explicit and recorded next action has checkpoint provenance", () => {
  expect(historyRecap({}, {}, undefined).join("\n")).toContain("Unknown / not recorded");
  expect(historyRecap({}, { checkpoints: [{ checkpoint: { input: { next_action: "Inspect handler" }, captured: "then" } }] }, []).join("\n")).toContain("Inspect handler [run.checkpoints");
  expect(nextHistoryPage("[page byte limit; resume with --lines 4-203]", 1)).toBe(4);
  expect(nextHistoryPage("ordinary page", 201)).toBe(401);
});

test("assistant recap uses only recorded ledger and question facts, with ended closure", () => {
  const container = { state: { questions: [{ state: "open", text: "Actual question" }], todo: { phases: [{ steps: [{ id: "next", label: "Recorded work", status: { state: "pending" } }] }] } } };
  expect(assistantRecap({}, container).join("\n")).toContain("Actual question");
  expect(assistantRecap({ ended: "yesterday" }, container).join("\n")).not.toContain("Actual question");
  expect(assistantRecap({}, container).join("\n")).toContain("next: Recorded work");
});

test("legacy/ended run opens bounded pages, byte-limit continuation and previous without editing draft", async () => {
  const calls: HistoryRequest[] = [], capture = historyUI([0, 0, 0], ["t", "n", "p", "\x1b"]);
  const navigator = new HistoryNavigator(async request => {
    calls.push(request);
    switch (request.kind) {
      case "tasks": return [{ id: "T-1", title: "legacy" }];
      case "task": return { id: "T-1", runs: [{ id: "R1", ended: "yesterday", summary: "Recorded legacy summary" }] };
      case "questions": return [];
      case "transcript": return request.first === 1 ? "record\n[page byte limit; resume with --lines 4-203]" : "next record";
      default: throw new Error("Unexpected operation");
    }
  }, () => true);
  const before = capture.ui.getEditorText();
  await navigator.open(capture.ui);
  expect(calls.filter(call => call.kind === "transcript").map(call => call.first)).toEqual([1, 4, 1]);
  expect(capture.frames.join("\n")).toContain("Recorded legacy summary");
  expect(capture.frames.join("\n")).toContain("Unknown / not recorded");
  expect(capture.ui.getEditorText()).toBe(before);
  expect(capture.notices).toEqual([]);
});

test("ended assistants use recorded summary and generations, never load a session file", async () => {
  const calls: string[] = [], capture = historyUI([1, 0]);
  await new HistoryNavigator(async request => {
    calls.push(request.kind);
    return request.kind === "assistants" ? [{ session: "A-1", focus: "past" }] : {
      session: { id: "A-1", ended: "yesterday", summary: "Owner recorded summary" }, container: { state: { generations: [{ n: 1 }] } },
    };
  }, () => true).open(capture.ui);
  expect(calls).toEqual(["assistants", "assistant"]);
  expect(capture.frames.join("\n")).toContain("Owner recorded summary");
  expect(capture.frames.join("\n")).toContain("Transcript: unavailable here");
});

test("checkpoint inspection uses only the existing list API and renders unknown recovery honestly", async () => {
  const capture = historyUI([0, 0, 0], ["c", "\x1b"]), calls: string[] = [];
  await new HistoryNavigator(async request => {
    calls.push(request.kind);
    if (request.kind === "tasks") return [{ id: "T-1" }];
    if (request.kind === "task") return { id: "T-1", runs: [{ id: "R1", agent_host: { generations: [{ n: 2 }], rewinds: [{ state: "aborted" }] } }] };
    if (request.kind === "questions") return [];
    if (request.kind === "recovery") return { checkpoints: [], rewinds: [{ state: "aborted" }], pending: null };
    throw new Error("Unexpected operation");
  }, () => true).open(capture.ui);
  expect(calls).toEqual(["tasks", "task", "questions", "recovery"]);
  expect(capture.frames.join("\n")).toContain("aborted");
  expect(capture.frames.join("\n")).not.toContain("Restore now");
});

test("missing assistant record is an explicit failed read", async () => {
  const capture = historyUI([1, 0]);
  await new HistoryNavigator(async request => {
    if (request.kind === "assistants") return [{ session: "A-1" }];
    throw new Error("assistant not found");
  }, () => true).open(capture.ui);
  expect(capture.notices.join()).toContain("assistant not found");
});

test("missing transcript is unavailable, not an invented recap or successful empty history", async () => {
  const capture = historyUI([0, 0, 0], ["t"]);
  await new HistoryNavigator(async request => {
    if (request.kind === "tasks") return [{ id: "T-1" }];
    if (request.kind === "task") return { id: "T-1", runs: [{ id: "R1" }] };
    if (request.kind === "questions") return [];
    throw new Error("transcript_unavailable: no session locator");
  }, () => true).open(capture.ui);
  expect(capture.notices.join()).toContain("transcript_unavailable");
});

test("disposal during asynchronous read prevents late historical overlay", async () => {
  const capture = historyUI([0, 0, 0]);
  let release: ((value: unknown) => void) | undefined;
  const navigator = new HistoryNavigator(() => new Promise(resolve => { release = resolve; }), () => true);
  const opening = navigator.open(capture.ui);
  // Microtask boundaries advance only the immediate native selection/input promises.
  for (let i = 0; i < 4; i++) await Promise.resolve();
  navigator.dispose(); release?.([{ id: "T-1" }]);
  await opening;
  expect(capture.frames).toEqual([]);
});
