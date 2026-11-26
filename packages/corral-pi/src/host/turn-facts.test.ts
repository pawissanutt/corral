import { expect, test } from "bun:test";
import { recordTranscriptMark, type MarkSink, type TranscriptMark } from "./transcript-marks.ts";
import { TurnFacts, turnReason, turnWaits } from "./turn-facts.ts";

// TX-6: completion is a run boundary, not the last assistant message's blank line.
test("turn reasons distinguish completion, waits, owner stop, failure, length and continuation stop", () => {
  expect(turnReason({})).toBe("done");
  expect(turnReason({ waits: [{ kind: "job", id: "j1", label: "job j1" }] })).toBe("waiting");
  expect(turnReason({ ownerStopped: true })).toBe("stopped");
  expect(turnReason({ stopReason: "error" })).toBe("error");
  expect(turnReason({ stopReason: "length" })).toBe("length");
  expect(turnReason({ continuationStopped: "no_progress" })).toBe("continue_stopped");
  expect(turnReason({ ownerStopped: true, stopReason: "error", continuationStopped: "limit" })).toBe("stopped");
});

test("waits include owned work still draining, but exclude confirmed terminal work", () => {
  const waits = turnWaits({ jobs: [
    { job: "j1-2", state: "running", command: "bun test" },
    { job: "j1-3", state: "exited", command: "check", groupClear: false },
    { job: "j1-4", state: "exited", command: "check", groupClear: true },
  ], questions: [{ id: "Q1", status: "pending" }, { id: "Q2", status: "answered" }],
    children: [{ id: "slice", state: "stopping", label: "agent slice" }] });
  expect(waits.map(wait => [wait.kind, wait.id])).toEqual([["job", "j1-2"], ["job", "j1-3"], ["owner", "Q1"], ["child", "slice"]]);
});

test("a missing sink is a no-op and replayed ends cannot create a mark", () => {
  const facts = new TurnFacts();
  facts.start(1, 10, 0);
  facts.assistant(1, "stop", []);
  facts.end(1, 20, 0, []);
  expect(facts.finish(1)).toBe(false);
  const marks: TranscriptMark[] = [];
  const replay = new TurnFacts({ add: mark => { marks.push(mark); return true; } });
  replay.end(1, 20, 0, []);
  replay.finish(1);
  expect(marks.length).toBe(0);
});

test("one run records all assistant messages once, with source cost and rebilled tokens", () => {
  const marks: TranscriptMark[] = [];
  const facts = new TurnFacts({ add: mark => { marks.push(mark); return true; } });
  facts.start(1, 100, 2);
  facts.assistant(1, "toolUse", ["tool1", "tool2"], 25000);
  facts.toolStarted(1, "tool1");
  facts.assistant(1, "aborted", []);
  facts.end(1, 350, 2.5, [], { ownerStopped: true });
  facts.end(1, 400, 3, [], { ownerStopped: true });
  facts.finish(1);
  facts.finish(1);
  facts.start(1, 500, 3);
  facts.end(1, 600, 4, []);
  facts.finish(1);
  expect(marks).toEqual([{ kind: "turn_end", at: 350, durationMs: 250, reason: "stopped",
    toolCalls: 2, notRun: 1, waits: [], cost: 0.5, rebilledTokens: 25000 }]);
});

test("the lifecycle continuation decision completes the already-ended run", () => {
  const marks: TranscriptMark[] = [];
  const facts = new TurnFacts({ add: mark => { marks.push(mark); return true; } });
  facts.start(1, 10, 0);
  facts.end(1, 20, 0, []);
  expect(facts.finish(1, { continuationStopped: "limit" })).toBe(true);
  expect(facts.finish(1, { continuationStopped: "limit" })).toBe(false);
  expect(marks[0]?.kind === "turn_end" && marks[0].reason).toBe("continue_stopped");
});

test("an unbound interactive build records run, model, child and note facts, but replay and headless do not", () => {
  const marks: TranscriptMark[] = [];
  const journal: MarkSink = { add: mark => { marks.push(mark); return true; } };
  const state = { attached: "interactive" as const, plain: false, bound: false, build: true };
  const sink: MarkSink = { add: mark => recordTranscriptMark(journal, mark, state) };
  const facts = new TurnFacts(sink);
  facts.start(1, 10, 0);
  facts.end(1, 20, 0, []);
  expect(facts.finish(1)).toBe(true);
  expect(sink.add({ kind: "model", at: 20, model: "test/model" })).toBe(true);
  expect(sink.add({ kind: "child", at: 20, act: "asked", child: "one", text: "Question" })).toBe(true);
  const note: TranscriptMark = { kind: "note", at: 20, tone: "warning", text: "Notice" };
  expect(sink.add(note)).toBe(true);
  expect(marks.map(mark => mark.kind)).toEqual(["turn_end", "model", "child", "note"]);
  expect(recordTranscriptMark(journal, note, { ...state, attached: undefined })).toBe(false);
  expect(recordTranscriptMark(journal, note, { ...state, attached: "headless" })).toBe(false);
  expect(recordTranscriptMark(journal, note, { ...state, plain: true })).toBe(false);
  expect(recordTranscriptMark(journal, note, { ...state, build: false })).toBe(false);
  expect(recordTranscriptMark({ add: () => false }, note, state)).toBe(false);
  expect(marks.length).toBe(4);
});

test("a host cancellation is not an owner stop and carries its cause on the single run mark", () => {
  const marks: TranscriptMark[] = [];
  const facts = new TurnFacts({ add: mark => { marks.push(mark); return true; } });
  facts.start(1, 10, 0);
  facts.assistant(1, "aborted", []);
  facts.end(1, 20, 0, [], { ownerStopped: true, error: true, stopCause: "host" });
  expect(facts.finish(1)).toBe(true);
  const mark = marks[0];
  expect(mark?.kind === "turn_end" && mark.reason).toBe("error");
  expect(mark?.kind === "turn_end" && mark.note).toContain("host");
  expect(marks.length).toBe(1);
});
