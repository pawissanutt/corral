import { expect, test } from "bun:test";
import type { ChildSnapshot, FleetSnapshot } from "../../build/types.ts";
import { sessionRecord } from "./session-record.ts";

const slice: ChildSnapshot = {
  id: "slice", kind: "slice", call: "call", title: "Implement", brief: "Implement", owns: ["owned.ts"], after: [],
  profile: "default", model: "provider/worker", state: "done", createdAt: 0, startedAt: 1, endedAt: 2,
  edits: [{ path: "owned.ts", outside: false, at: 1 }, { path: "outside.ts", outside: true, at: 1 }, { path: "outside.ts", outside: true, at: 2 }],
  usage: { input: 10, output: 20, turns: 2, cost: 1 }, steers: [], pulse: [], history: "d", messages: [], report: "r".repeat(8193),
};
const review: ChildSnapshot = { ...slice, id: "review", kind: "review", title: "Review", model: "provider/reviewer", edits: [],
  usage: { input: 3, output: 4, turns: 1, cost: 0.5 }, verdict: { verdict: "changes", summary: "Fix this", head: "abc", at: 3,
    findings: [{ severity: "major", path: "owned.ts", line: 2, text: "Incorrect result" }] } };
const fleet: FleetSnapshot = { children: [slice, review], maxParallel: 2, mail: [
  { seq: 1, at: 1, from: "slice", to: "parent", text: "m".repeat(2049), delivery: "delivered" },
  { seq: 2, at: 2, from: "parent", to: "review", text: "Review this", delivery: "queued" },
] };

test("worker session records preserve child work, typed reviews and messages with build-record caps and separate usage", () => {
  const record = sessionRecord(fleet, { task: "T-007", run: "R2" });
  expect(record.version).toBe(1);
  expect(record.task).toBe("T-007");
  expect(record.run).toBe("R2");
  expect(record.slices).toHaveLength(2);
  expect(record.slices[0]).toEqual({ id: "slice", kind: "slice", title: "Implement", owns: ["owned.ts"], model: "provider/worker",
    effort: undefined, state: "done", started: new Date(1).toISOString(), ended: new Date(2).toISOString(),
    usage: { input: 10, output: 20, turns: 2, cost: 1 }, outside: ["outside.ts"], report: "r".repeat(8192) });
  expect(record.reviews).toEqual([{ round: 1, reviewer: "review", model: "provider/reviewer", verdict: "changes", summary: "Fix this",
    head: "abc", at: new Date(3).toISOString(), findings: [{ id: "F1", severity: "major", path: "owned.ts", line: 2, text: "Incorrect result" }] }]);
  expect(record.messages).toEqual([
    { from: "slice", to: "parent", at: new Date(1).toISOString(), text: "m".repeat(2048), delivery: "delivered" },
    { from: "parent", to: "review", at: new Date(2).toISOString(), text: "Review this", delivery: "queued" },
  ]);
  expect(record.usage).toEqual({ children: { input: 10, output: 20, cost: 1 }, reviewers: { input: 3, output: 4, cost: 0.5 },
    total: { input: 13, output: 24, cost: 1.5 } });
});

test("worker session records retain only the newest 500 messages and never invent unknown model prices or review heads", () => {
  const unpriced = { ...review, usage: { input: 3, output: 4, turns: 1 }, verdict: { ...review.verdict!, head: undefined } };
  const record = sessionRecord({ ...fleet, children: [slice, unpriced],
    mail: Array.from({ length: 501 }, (_, index) => ({ ...fleet.mail[0]!, seq: index + 1, at: index, text: String(index) })) });
  expect(record.messages).toHaveLength(500);
  expect(record.messages[0]?.text).toBe("1");
  expect(record.messages.at(-1)?.text).toBe("500");
  expect(record.usage.reviewers).toEqual({ input: 3, output: 4 });
  expect(record.usage.total).toEqual({ input: 13, output: 24 });
  expect(record.reviews[0]?.head).toBeUndefined();
});
