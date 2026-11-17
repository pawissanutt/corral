import { expect, test } from "bun:test";
import type { DialogAnswer } from "./ask-user-dialog.ts";
import { type CanonicalAnswer, type CanonicalAsk, mirroredDialog, OwnerAskMirror, type QuestionMirror } from "./owner-ask-mirror.ts";
import { answerMarker, PendingQuestions, QUESTION_ENTRY, QUESTION_TTL, type QuestionEntry } from "./pending-questions.ts";

const question = [{ question: "Which scope?", options: ["Small (recommended)", "Large"], freeText: true }];
const small = [{ option: "Small (recommended)", text: "Only the requested fix" }];

/** Corral's owner-ask records, with the owner answering or cancelling from the app. */
function board() {
  type Row = { revision: number; state: string; answer?: CanonicalAnswer; text: string; options: readonly string[] };
  const rows = new Map<string, Row>(), opened = new Map<string, string>();
  let down = false;
  const reach = () => { if (down) throw new Error("corral_unreachable"); };
  const view = (key: string): CanonicalAsk => {
    const row = rows.get(key)!;
    return { key, revision: row.revision, state: row.state, ...(row.answer ? { answer: row.answer } : {}) };
  };
  const settle = (key: string, state: string, answer?: CanonicalAnswer): boolean => {
    const row = rows.get(key)!;
    if (row.state !== "open") return false;
    row.state = state; row.revision++;
    if (answer) row.answer = answer;
    return true;
  };
  const mirror: QuestionMirror = {
    async open(id, text, options) {
      reach();
      let key = opened.get(id);
      if (!key) { key = `pm/Q${rows.size + 1}`; opened.set(id, key); rows.set(key, { revision: 1, state: "open", text, options }); }
      return view(key);
    },
    async get(key) { reach(); return view(key); },
    async answer(key, answer) { reach(); return settle(key, "answered", answer) ? { ...view(key), mine: true } : view(key); },
    async withdraw(key) { reach(); settle(key, "withdrawn"); return view(key); },
  };
  return { mirror, rows, view, outage: (value: boolean) => { down = value; },
    app: { answer: (key: string, answer: CanonicalAnswer) => settle(key, "answered", answer), cancel: (key: string) => settle(key, "cancelled") } };
}

/** A pane question store whose mirror passes run only when the test asks for one. */
function pane() {
  const b = board(), entries: QuestionEntry[] = [], deliveries: string[] = [], failures: string[] = [];
  let now = 100;
  const create = (generation = 1) => new PendingQuestions({ session: () => "owned", generation, entries: () => entries,
    append: data => { entries.push({ type: "custom", customType: QUESTION_ENTRY, data }); }, changed() {}, now: () => now,
    record: async text => text, deliver: async text => { deliveries.push(text); return true; },
    mirror: b.mirror, mirrorFailed: error => { failures.push(String(error)); }, defer: () => () => {} });
  return { b, create, entries, deliveries, failures, tick: () => { now += QUESTION_TTL; } };
}

test("an app answer reaches the model once and the pane cannot answer over it", async () => {
  const f = pane(), q = f.create(); q.restore();
  const id = q.ask("Q", question).id;
  // Given the question is open on the board as the pane asked it:
  expect(await q.reconcile()).toBeTrue();
  expect(f.b.rows.get("pm/Q1")).toMatchObject({ state: "open", text: "Which scope?", options: question[0]!.options });
  // When the owner answers in the app:
  f.b.app.answer("pm/Q1", { text: "Ship the small one first", option: "Large" });
  expect(await q.reconcile()).toBeFalse();
  // Then the model receives that answer, and neither a later pane answer nor another pass repeats it.
  expect(f.deliveries).toEqual([`${answerMarker(id)}\nWhich scope?: Large\nShip the small one first`]);
  expect(await q.answer(id, small)).toBe("answered");
  await q.reconcile();
  expect(f.deliveries).toHaveLength(1);
  expect(f.b.view("pm/Q1").answer).toEqual({ text: "Ship the small one first", option: "Large" });
});

test("a pane answer is recorded on the board, and an app answer recorded first wins", async () => {
  const f = pane(), q = f.create(); q.restore();
  const won = q.ask("won", question).id, lost = q.ask("lost", question).id;
  await q.reconcile();
  // When the pane answers one question and the app has already answered the other differently:
  await q.answer(won, [{ option: "Large" }]);
  f.b.app.answer("pm/Q2", { text: "Large", option: "Large" });
  await q.answer(lost, small);
  // Then the board holds the pane's answer for the first, and the model gets the board's for the second.
  expect(f.b.view("pm/Q1")).toMatchObject({ state: "answered", answer: { text: "Large", option: "Large" } });
  expect(f.deliveries).toEqual([`${answerMarker(won)}\nWhich scope?: Large`, `${answerMarker(lost)}\nWhich scope?: Large`]);
  expect(await q.reconcile()).toBeFalse();
});

test("a pane question that ends unanswered stops asking on the board, and an app cancel ends it in the pane", async () => {
  const f = pane(), q = f.create(); q.restore();
  const withdrawn = q.ask("a", question).id, cancelled = q.ask("b", question).id;
  const appCancelled = q.ask("c", question).id; q.ask("d", question);
  await q.reconcile();
  q.withdraw(withdrawn); q.cancel(cancelled); f.b.app.cancel("pm/Q3");
  expect(await q.reconcile()).toBeTrue(); // the fourth is still open
  expect(["pm/Q1", "pm/Q2", "pm/Q3", "pm/Q4"].map(key => f.b.view(key).state)).toEqual(["withdrawn", "withdrawn", "cancelled", "open"]);
  expect(await q.answer(appCancelled, small)).toBe("cancelled");
  f.tick(); // the pane question expires
  expect(await q.reconcile()).toBeFalse();
  expect(f.b.view("pm/Q4").state).toBe("withdrawn");
  expect(f.deliveries).toEqual([]);
});

test("a question asked while corral is unreachable is mirrored once later, across a restart", async () => {
  const f = pane(), q = f.create(); q.restore();
  f.b.outage(true);
  const id = q.ask("Q", question).id;
  expect(await q.reconcile()).toBeTrue();
  expect(f.failures).toEqual(["Error: corral_unreachable"]);
  expect(q.get(id)?.status).toBe("pending");
  f.b.outage(false);
  await q.reconcile();
  // The host dies after corral opened the record but before its key reached the journal.
  f.entries.pop(); q.close();
  const resumed = f.create(2); resumed.restore();
  await resumed.reconcile();
  expect(f.b.rows.size).toBe(1);
  f.b.app.answer("pm/Q1", { text: "Large", option: "Large" });
  await resumed.reconcile();
  expect(f.deliveries).toEqual([`${answerMarker(id)}\nWhich scope?: Large`]);
});

test("losing the revision race to the app returns the board's answer, not the pane's", async () => {
  const sent: Record<string, unknown>[] = [];
  const record = { key: "pm/Q1", revision: 1, state: "open" };
  const receipt = (state: "committed" | "rejected", result: unknown) => ({ version: 1, request_id: "rq", op: "owner_ask", state, result });
  const mirror = new OwnerAskMirror({ call: async (_op, call) => {
    sent.push(call.payload);
    if (call.payload.action === "answer") {
      // The app answered between this host's read and its write.
      Object.assign(record, { revision: 2, state: "answered" });
      return receipt("rejected", { code: "stale_revision", reason: "pm/Q1 is at revision 2, not 1" });
    }
    return receipt("committed", { ...record,
      ...(record.state === "answered" ? { question: { answer: { by: "owner", text: "Large", option: "Large" } } } : {}) });
  } }, () => "session", () => 3);
  expect(await mirror.answer("pm/Q1", { text: "Small", option: "Small" }))
    .toEqual({ key: "pm/Q1", revision: 2, state: "answered", answer: { text: "Large", option: "Large" } });
  expect(sent).toEqual([{ action: "get", key: "pm/Q1" },
    { action: "answer", key: "pm/Q1", revision: 1, text: "Small", option: "Small" }, { action: "get", key: "pm/Q1" }]);
});

test("an app answer closes the open pane dialog, and a question nobody answered is withdrawn", async () => {
  const b = board();
  const asked = [{ question: "Which scope?", options: [{ label: "Small", description: "one crate" }, { label: "Large" }] }];
  // Given a dialog that stays open until it is closed from outside, and the owner answering in the app:
  const open = (signal?: AbortSignal) => {
    const { promise, resolve } = Promise.withResolvers<(DialogAnswer | undefined)[]>();
    b.app.answer("pm/Q1", { text: "Ship it", option: "Large" });
    signal?.addEventListener("abort", () => { resolve([undefined]); }, { once: true });
    return promise;
  };
  expect(await mirroredDialog(b.mirror, "call-1", asked, undefined, open, 1)).toEqual([{ selected: ["Large"], text: "Ship it" }]);
  expect(b.rows.get("pm/Q1")?.text).toBe("Which scope?\nSmall: one crate");
  // A dialog the owner dismisses leaves nothing waiting in the Inbox.
  expect(await mirroredDialog(b.mirror, "call-2", asked, undefined, async () => [undefined], 1)).toEqual([undefined]);
  expect(b.view("pm/Q2").state).toBe("withdrawn");
});
