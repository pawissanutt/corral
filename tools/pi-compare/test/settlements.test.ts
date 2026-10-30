import { expect, test } from "bun:test";
import { interventions, uniqueEvents } from "../src/events.ts";
import type { Event } from "../src/schema.ts";

const at = (second: number) => `2026-10-24T00:00:${String(second).padStart(2, "0")}Z`;
const event = (kind: string, second: number, detail: Readonly<Record<string, unknown>>): Event => ({
  ts: at(second), task: "T-synthetic", kind,
  detail: kind.startsWith("question_") ? { revision: 3, at: at(second), ...detail } : detail,
});
const end = Date.parse("2026-10-24T00:01:00Z");

test("a non-control delivery acknowledgement settles its matching decision", () => {
  const input = [
    event("needs_me", 1, { decision: "D1" }),
    event("reply_marked_delivered", 5, { decision: "D1", control: false, resumes: true }),
    event("done", 20, {}),
  ];
  expect(interventions(uniqueEvents(input), end).needs_me_seconds).toBe(4);
});

test.each(["question_cancelled", "question_withdrawn"])("%s settles its matching question", (kind) => {
  const input = [
    event("needs_me", 1, { question: "Q1" }),
    event(kind, 5, { key: "Q1" }),
    event("done", 20, {}),
  ];
  const actual = interventions(uniqueEvents(input), end);
  expect(actual.needs_me_seconds).toBe(4);
  expect(actual.pm_answers).toBe(0);
  expect(actual.owner_answers).toBe(0);
});

for (const kind of ["question_cancelled", "question_withdrawn"]) {
  test.each(["question", "decision"])(
    `${kind} preserves overlapping holds until the last %s settles`,
    (last) => {
      const input = [
        event("needs_me", 1, { question: "Q1" }),
        event("needs_me", 2, { question: "Q2" }),
        event("needs_me", 3, { decision: "D1" }),
        event(kind, 4, { key: "unrelated" }),
        event(kind, 5, { key: "Q1" }),
        event(kind, last === "question" ? 10 : 8, { key: "Q2" }),
        event("run_resumed", last === "decision" ? 10 : 8, { decision: "D1", outcome: "answered" }),
        event("done", 20, {}),
      ];
      expect(interventions(uniqueEvents(input), end).needs_me_seconds).toBe(9);
    },
  );
}

test("acknowledgements consume one decision without clearing other decisions or questions", () => {
  const input = [
    event("needs_me", 1, { decision: "D1" }),
    event("needs_me", 2, { decision: "D2" }),
    event("needs_me", 3, { question: "Q1" }),
    event("reply_marked_delivered", 4, { decision: "unrelated", control: false, resumes: true }),
    event("reply_marked_delivered", 5, { decision: "D1", control: false, resumes: false }),
    event("reply_marked_delivered", 8, { decision: "D2", control: false, resumes: true }),
    event("question_cancelled", 10, { key: "Q1" }),
    event("done", 20, {}),
  ];
  expect(interventions(uniqueEvents(input), end).needs_me_seconds).toBe(9);
});

test.each([
  { control: true, submitted: true },
  { control: false, submitted: false },
])("control/failed acknowledgement cannot answer a decision: %j", (delivery) => {
  const input = [
    event("needs_me", 1, { decision: "D1" }),
    event("reply_marked_delivered", 5, { decision: "D1", resumes: true, ...delivery }),
    event("run_resumed", 8, { decision: "unrelated", outcome: "answered" }),
    event("run_resumed", 12, { decision: "D1", outcome: "answered" }),
    event("done", 20, {}),
  ];
  expect(interventions(uniqueEvents(input), end).needs_me_seconds).toBe(11);
});

test("an acknowledgement without resume semantics cannot close a hold on its own", () => {
  const input = [
    event("needs_me", 1, { decision: "D1" }),
    event("reply_marked_delivered", 5, { decision: "D1", control: false, resumes: false }),
    event("done", 20, {}),
  ];
  expect(interventions(uniqueEvents(input), end).needs_me_seconds).toBe(19);
});
