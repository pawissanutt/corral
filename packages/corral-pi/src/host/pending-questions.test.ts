import { expect, test } from "bun:test";
import { PendingQuestions, QUESTION_ENTRY, PENDING_MAX, QUESTION_TTL, type QuestionEntry } from "./pending-questions.ts";

const question = [{ question: "Which scope?", options: ["Small (recommended)", "Large"], freeText: true }];
const answer = [{ option: "Small (recommended)", text: "Only the requested fix" }];
function fixture() {
  let now = 100;
  const entries: QuestionEntry[] = [], records: string[] = [], deliveries: string[] = [];
  let accept = true;
  const create = (generation = 1) => new PendingQuestions({ session: () => "owned", generation,
    entries: () => entries, append: data => { entries.push({ type: "custom", customType: QUESTION_ENTRY, data }); },
    now: () => now, changed() {},
    record: async text => { records.push(text); return `${text}\n[owner input A-1/g${generation}/i1]`; },
    deliver: async text => { deliveries.push(text); return accept; },
  });
  return { create, entries, records, deliveries, tick: () => { now += QUESTION_TTL; }, reject: () => { accept = false; } };
}

test("pending lifecycle is bounded, idempotent and never turns no-answer states into consent", async () => {
  const f = fixture(), q = f.create(); q.restore();
  const ids = Array.from({ length: PENDING_MAX }, (_, i) => q.ask(`call${i}`, question).id);
  expect(q.pending()).toHaveLength(PENDING_MAX);
  expect(() => q.ask("overflow", question)).toThrow("pending_question_limit");
  expect(q.ask("call0", [{ options: question[0]!.options, freeText: true, question: "Which scope?" }]).id).toBe(ids[0]!);
  expect(() => q.ask("call0", [{ ...question[0]!, question: "Different" }])).toThrow("question_key_conflict");
  expect(q.cancel(ids[0]!)).toBe("cancelled");
  expect(await q.answer(ids[0]!, answer)).toBe("cancelled");
  const replacement = q.ask("replacement", question, "stable-key", ids[1]);
  expect(q.get(ids[1]!)?.status).toBe("withdrawn");
  expect(await q.answer(ids[1]!, answer)).toBe("withdrawn");
  const resumed = f.create(2); q.close(); resumed.restore();
  expect(resumed.pending()).toHaveLength(PENDING_MAX - 1);
  expect(resumed.get(replacement.id)?.generation).toBe(1);
  expect(resumed.ask("retry", question, "stable-key").id).toBe(replacement.id);
  f.tick(); resumed.expire();
  expect(await resumed.answer(replacement.id, answer)).toBe("expired");
  expect(resumed.pending()).toHaveLength(0);
  expect(f.records).toEqual([]); expect(f.deliveries).toEqual([]);
  const late = resumed.ask("late", question); resumed.close(true);
  expect(await resumed.answer(late.id, answer)).toBe("session_closed");
  const ended = f.create(3); ended.restore();
  expect(ended.get(late.id)?.status).toBe("ended");
  expect(await ended.answer(late.id, answer)).toBe("ended");
});

test("crash/resume retries a selected unconsumed answer but never replays a consumed answer", async () => {
  const f = fixture(), q = f.create(); q.restore();
  const id = q.ask("Q", question).id;
  await q.answer(id, answer);
  await q.answer(id, answer); await q.recover();
  expect(f.records).toHaveLength(1); expect(f.deliveries).toHaveLength(1);
  q.close(); // The queued follow-up was lost before the user message was persisted.
  const resumed = f.create(2); resumed.restore(); await resumed.recover();
  expect(f.records).toHaveLength(2); expect(f.deliveries).toHaveLength(2);
  const message = f.deliveries[1]!;
  f.entries.push({ type: "message", message: { role: "user", content: [{ type: "text", text: message }] } });
  resumed.close();
  const consumed = f.create(3); consumed.restore(); await consumed.recover();
  expect(consumed.status(id)).toBe("delivered");
  expect(f.records).toHaveLength(2); expect(f.deliveries).toHaveLength(2);
});

test("shutdown during recording fences late delivery and resume preserves the actual UI answer", async () => {
  const entries: QuestionEntry[] = [];
  let release!: (text: string) => void;
  const records = new Promise<string>(resolve => { release = resolve; });
  let sent = 0;
  const q = new PendingQuestions({ session: () => "owned", generation: 1, entries: () => entries,
    append: data => { entries.push({ type: "custom", customType: QUESTION_ENTRY, data }); }, changed() {},
    record: () => records, deliver: async () => { sent++; return true; },
  });
  q.restore(); const id = q.ask("Q", question).id;
  const answering = q.answer(id, answer); q.close(); release("actual owner answer"); await answering;
  expect(sent).toBe(0);
  expect(q.get(id)?.answers).toEqual(answer);
});
