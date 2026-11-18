import { expect, test } from "bun:test";
import { answerMarker, PendingQuestions, QUESTION_ENTRY, PENDING_MAX, QUESTION_TTL, type QuestionEntry } from "./pending-questions.ts";

const question = [{ question: "Which scope?", options: ["Small (recommended)", "Large"], freeText: true }];
const answer = [{ option: "Small (recommended)", text: "Only the requested fix" }];
function fixture() {
  let now = 100;
  const entries: QuestionEntry[] = [], deliveries: string[] = [];
  const create = () => new PendingQuestions({ session: () => "owned",
    entries: () => entries, append: data => { entries.push({ type: "custom", customType: QUESTION_ENTRY, data }); },
    now: () => now, changed() {},
    deliver: async text => { deliveries.push(text); return true; },
  });
  return { create, entries, deliveries, tick: () => { now += QUESTION_TTL; } };
}

test("pending lifecycle is bounded, idempotent and never turns no-answer states into consent", async () => {
  const f = fixture(), q = f.create(); q.restore();
  const ids = Array.from({ length: PENDING_MAX }, (_, i) => q.ask(`call${i}`, question).id);
  expect(q.pending()).toHaveLength(PENDING_MAX);
  expect(() => q.ask("overflow", question)).toThrow("pending_question_limit");
  expect(q.ask("call0", [{ options: question[0]!.options, freeText: true, question: "Which scope?" }]).id).toBe(ids[0]!);
  expect(() => q.ask("call0", [{ ...question[0]!, question: "Different" }])).toThrow("question_call_conflict");
  expect(q.cancel(ids[0]!)).toBe("cancelled");
  expect(await q.answer(ids[0]!, answer)).toBe("cancelled");
  expect(q.withdraw(ids[1]!)).toBe("withdrawn");
  const replacement = q.ask("replacement", question);
  expect(await q.answer(ids[1]!, answer)).toBe("withdrawn");
  const resumed = f.create(); q.close(); resumed.restore();
  expect(resumed.pending()).toHaveLength(PENDING_MAX - 1);
  expect(resumed.get(replacement.id)).toEqual(replacement);
  expect(resumed.ask("replacement", question).id).toBe(replacement.id);
  f.tick(); resumed.expire();
  expect(await resumed.answer(replacement.id, answer)).toBe("expired");
  expect(resumed.pending()).toHaveLength(0);
  expect(f.deliveries).toEqual([]);
  const late = resumed.ask("late", question); resumed.close(true);
  expect(await resumed.answer(late.id, answer)).toBe("session_closed");
  const ended = f.create(); ended.restore();
  expect(ended.get(late.id)?.status).toBe("ended");
  expect(await ended.answer(late.id, answer)).toBe("ended");
});

test("crash/resume retries a selected unconsumed answer but never replays a consumed answer", async () => {
  const f = fixture(), q = f.create(); q.restore();
  const id = q.ask("Q", question).id;
  await Promise.all([q.answer(id, answer), q.answer(id, [{ option: "Large" }])]);
  await q.recover();
  const message = `${answerMarker(id)}\nWhich scope?: Small (recommended)\nOnly the requested fix`;
  expect(f.deliveries).toEqual([message]);
  q.close(); // The queued follow-up was lost before the user message was persisted.
  const resumed = f.create(); resumed.restore(); await resumed.recover();
  expect(f.deliveries).toEqual([message, message]);
  f.entries.push({ type: "message", message: { role: "user", content: [{ type: "text", text: message }] } });
  resumed.close();
  const consumed = f.create(); consumed.restore(); await consumed.recover();
  expect(consumed.status(id)).toBe("delivered");
  expect(f.deliveries).toEqual([message, message]);
});

test("a typed answer needs nonempty text and respects an explicit freeText prohibition", async () => {
  const f = fixture(), q = f.create(); q.restore();
  const open = q.ask("open", [{ question: "Which?", options: ["A", "B"] }]).id;
  const closed = q.ask("closed", [{ question: "Which?", options: ["A", "B"], freeText: false }]).id;
  await expect(q.answer(open, [{ text: "  " }])).rejects.toThrow("invalid_question_answer");
  await expect(q.answer(open, [{ option: "A", text: "note" }])).rejects.toThrow("invalid_question_answer");
  await expect(q.answer(closed, [{ text: "mine" }])).rejects.toThrow("invalid_question_answer");
  expect(await q.answer(open, [{ text: "neither" }])).toBe("answered");
  expect(f.deliveries).toEqual([`${answerMarker(open)}\nWhich?: neither`]);
});
