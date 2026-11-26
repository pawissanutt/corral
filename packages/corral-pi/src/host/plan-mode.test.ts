import { afterEach, expect, test } from "bun:test";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { briefedCards, type CardEntry, type CardKind } from "./remote/cards.ts";
import { registerPlanMode } from "./remote/plan.ts";
import { RemoteRegistry } from "./remote/registry.ts";
import { PLAN_ENTRY, PLAN_FEEDBACK, PLAN_KICKOFF, PLAN_NOTE, PlanMode, type PlanEntry, type PlanHost, type PlanStatus } from "./plan-mode.ts";

const dirs: string[] = [];
afterEach(() => { for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true }); });
const settle = () => new Promise(resolve => setTimeout(resolve, 0));

/**
 * A plan mode over an in-memory branch, already planning with a plan file written. `kickoff` says what the session
 * does with an approved plan's kickoff: it reaches the transcript unless it is only `taken`, `refused` or `throws`.
 */
async function planning(session: { model?: string; blocked?: string; kickoff?: "taken" | "refused" | "throws" | "after-a-settle";
  stopping?: () => void } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "plan-mode-unit-"));
  dirs.push(dir);
  const entries: PlanEntry[] = [], notes: string[] = [], journal: string[] = [];
  const noticeIds: string[] = [];
  const sent: { text: string; kickoff?: string }[] = [];
  let leaf = 0;
  const add = (entry: PlanEntry) => { entries.push(entry); leaf++; };
  /**
   * A user message in the transcript; the host alone tags the one it admitted as a kickoff, and the owner's own
   * with the review it found open as `feedback`.
   */
  const user = (text: string, kickoff?: string, feedback?: string) => {
    add({ type: "message", message: { role: "user", content: text, ...(kickoff === undefined ? {} : { [PLAN_KICKOFF]: kickoff }),
      ...(feedback === undefined ? {} : { [PLAN_FEEDBACK]: feedback }) } });
    running.refresh();
  };
  const host: PlanHost = {
    dir, cwd: dir, prompts: { contract: () => "CONTRACT", reset: () => "", approved: () => "EXECUTE" }, askUser: false, todo: false,
    entries: () => entries, leaf: () => String(leaf), append: record => { add({ type: "custom", customType: PLAN_ENTRY, data: record }); },
    busy: () => undefined, blocked: () => session.blocked, stopTurn: async () => { session.stopping?.(); },
    // The owner pauses while the approved plan's context is being summarised.
    compact: async () => { session.blocked = "The session is paused"; }, reset: async () => {},
    submit: async (text, _origin, answers) => {
      const kickoff = answers?.kickoff ? answers.card : undefined;
      if (session.kickoff === "throws") throw new Error("the held queue could not be saved");
      if (session.kickoff === "refused") return false;
      sent.push({ text, kickoff });
      // An earlier turn can settle while the kickoff is still being admitted.
      if (session.kickoff === "after-a-settle") running.turnEnded();
      if (session.kickoff !== "taken") user(text, kickoff);
      return true;
    },
    model: () => session.model, changed: () => {}, note: (text, _tone, id) => {
      notes.push(text);
      if (id) noticeIds.push(id);
    }, approved: summary => { journal.push(summary); },
  };
  const plan = new PlanMode(host);
  // The instance the host is running: a restart replaces it over the same transcript.
  let running = plan;
  await plan.enter();
  writeFileSync(plan.path, "# Same plan\n");
  return { plan, notes, noticeIds, journal, sent, add, user, restarted: () => running = new PlanMode(host),
    rewind: (length: number) => { entries.length = length; leaf++; } };
}

test("artifact publishing is refused while planning and during review", async () => {
  const { plan } = await planning();
  const refusal = () => plan.refusal("corral_artifact", { path: plan.path, kind: "report", to: "demo" });
  expect(refusal()).toBeDefined();
  plan.submit();
  expect(refusal()).toBeDefined();
  plan.leave();
  expect(refusal()).toBeUndefined();
});

test("a plan resubmitted after a rewind is its own card and its own copy", async () => {
  const { plan, rewind } = await planning();
  plan.submit();
  const first = plan.review, firstCopy = plan.record.path;
  // A rewind to before the submission rolls the revision back with the branch.
  rewind(1);
  plan.submit();
  expect(plan.record.revision).toBe(1);
  expect(plan.review).not.toBe(first);
  expect(plan.record.path).not.toBe(firstCopy);
  expect(existsSync(firstCopy ?? "")).toBeTrue();
});

test("an approval whose review a rewind removed while its turn was being stopped starts nothing", async () => {
  const session: Parameters<typeof planning>[0] = {};
  const { plan, sent, rewind } = await planning(session);
  plan.submit();
  // Given the owner rewinds to before the submission while the approval waits for the submitting turn to stop:
  session.stopping = () => rewind(1);
  await expect(Promise.resolve(plan.choose("execute"))).rejects.toThrow("no longer open");
  // Then the kickoff of a review that is gone is not sent into the rewound branch.
  expect(sent).toEqual([]);
  expect(plan.phase).toBe("planning");
});

test("a fallback model committed during turn settlement leaves approval context untouched", async () => {
  const session: Parameters<typeof planning>[0] = { model: "p/a" };
  const { plan, sent } = await planning(session);
  session.stopping = () => { session.model = "p/b"; };
  plan.submit();
  const card = plan.review;
  plan.choose("execute_compact");
  await settle();
  expect(session.blocked).toBeUndefined(); // The fixture sets it on any call to compact.
  expect(sent).toEqual([]);
  expect(plan.review).toBe(card);
});

test("a rewind from one pending review to another tells the views, though the phase is the same", async () => {
  const { plan, user, rewind } = await planning();
  plan.submit();
  const first = plan.review;
  user("Split step 2", undefined, first);
  plan.submit();
  expect(plan.review).not.toBe(first);
  let told = 0;
  plan.onChange(() => { told++; });
  // When the branch is taken back to the first submission, still under review:
  rewind(2);
  plan.refresh();
  // Then a view showing the second one hears of it, and can stop showing a plan that is no longer the pending one.
  expect(plan.review).toBe(first);
  expect(told).toBe(1);
});

test("a pending plan review is briefed in live state behind eight older cards", () => {
  const entry = (id: string, kind: CardKind, opened_at: string): CardEntry =>
    ({ card: { id, kind, title: id, questions: [], opened_at }, answer: () => "missing" });
  const older = Array.from({ length: 8 }, (_, n) => entry(`c-${n}`, "child_question", `2026-01-01T00:00:0${n}Z`));
  const briefs = briefedCards([...older, entry("plan-1-ab", "plan_review", "2026-01-02T00:00:00Z")]);
  // The app opens a review from its brief alone, so the newest card of nine still has to be one of the eight.
  expect(briefs.map(brief => brief.id)).toEqual([...older.slice(0, 7).map(({ card }) => card.id), "plan-1-ab"]);
});

test("a submission whose copy changed after it was shown is not executed", async () => {
  const { plan } = await planning();
  plan.submit();
  // Given the review has already read the copy:
  expect(plan.card()?.card.body).toBe("# Same plan\n");
  writeFileSync(plan.record.path ?? "", "# Another plan\n");
  // When the owner executes:
  await expect(Promise.resolve(plan.choose("execute"))).rejects.toThrow("missing or was changed");
  // Then nothing is approved and the review stays open.
  expect(plan.phase).toBe("review");
});

test("an approval the session cannot start leaves the same review open", async () => {
  // While the session could start nothing, the choice is refused before anything changes.
  const blocked = await planning({ blocked: "The session is paused" });
  blocked.plan.submit();
  expect(() => blocked.plan.choose("execute")).toThrow("The session is paused");
  expect(blocked.plan.phase).toBe("review");

  // A pause that lands while the context is prepared is seen before the kickoff, and nothing starts.
  const pausing = await planning();
  pausing.plan.submit();
  expect(pausing.plan.choose("execute_compact")).toBe("preparing");
  await settle();
  expect(pausing.plan.phase).toBe("review");
  expect(pausing.notes.join("\n")).toContain("The session is paused");

  // A kickoff the session does not take, or fails to take, leaves the submission under the same card.
  for (const kickoff of ["refused", "throws"] as const) {
    const { plan } = await planning({ kickoff });
    plan.submit();
    const review = plan.review;
    await expect(Promise.resolve(plan.choose("execute"))).rejects.toThrow("The review is still open");
    expect(plan.review).toBe(review);
  }
});

test("an approval is durable only with its kickoff, and holds the mode until then", async () => {
  // Given a review after a compaction, and a kickoff the session took but has not yet written to the transcript:
  const { plan, journal, sent, add, user, restarted } = await planning({ kickoff: "taken" });
  plan.submit();
  add({ type: "compaction" });
  const review = plan.review, copy = plan.record.path;
  expect(await plan.choose("execute")).toBe("started");
  const kickoff = sent[0];
  if (!kickoff) throw new Error("no kickoff was submitted");
  // Then nothing else decides or changes the mode meanwhile, and a second answer is told so,
  expect(() => plan.choose("execute_clean")).toThrow("already being answered");
  expect(() => plan.choose("revise", "Use a flag parser")).toThrow("already being answered");
  expect(() => plan.leave()).toThrow("already being answered");
  expect(sent).toHaveLength(1);
  // only the kickoff's own run goes without the planning contract,
  expect(plan.turnMessage(kickoff.text)).toBeUndefined();
  expect(plan.turnMessage("an unrelated input")?.details.kinds).toEqual(["contract"]);
  // the same words from anyone else approve nothing,
  user(kickoff.text);
  expect(plan.review).toBe(review);
  // and nothing is approved yet, for this host or for one that restarts here.
  expect(journal).toEqual([]);
  // When the message the host admitted as the kickoff is in the transcript, that is the approval.
  user(kickoff.text, kickoff.kickoff);
  expect(plan.phase).toBe("off");
  expect(plan.approved?.path).toBe(copy);
  expect(journal).toEqual(["Approved plan: Same plan (revision 1, context kept)"]);
});

test("feedback the session does not take leaves the review open for it", async () => {
  const refusing = await planning({ kickoff: "refused" });
  refusing.plan.submit();
  const review = refusing.plan.review, before = refusing.plan.brief()?.approval_epoch;
  // Other devices can distinguish failed feedback from their stale review snapshot.
  const feedback = refusing.plan.choose("revise", "Use a flag parser");
  expect(refusing.plan.brief()?.approval_pending).toBe(true);
  expect(refusing.plan.brief()?.approval_epoch).not.toBe(before);
  await expect(Promise.resolve(feedback)).rejects.toThrow("did not take the feedback");
  expect(refusing.plan.brief()?.approval_pending).toBeUndefined();
  expect(refusing.plan.brief()?.approval_error).toContain("did not take the feedback");
  expect(refusing.plan.review).toBe(review);

  // Feedback the session takes withdraws the review when it reaches the model, like any owner message.
  const taking = await planning({ kickoff: "taken" });
  taking.plan.submit();
  expect(await taking.plan.choose("revise", "Use a flag parser")).toBe("delivered");
  expect(taking.sent[0]?.text).toContain("Use a flag parser");
  // Until it does, no other answer can override it.
  expect(taking.plan.phase).toBe("review");
  expect(() => taking.plan.choose("execute")).toThrow("already being answered");
  taking.user(taking.sent[0]?.text ?? "", undefined, taking.plan.review);
  expect(taking.plan.phase).toBe("planning");
});

test("a remote ordinary approval publishes a new epoch that outlasts its later refusal", async () => {
  const { plan, notes, noticeIds } = await planning({ kickoff: "refused" });
  expect(plan.brief()?.approval_epoch).toBeUndefined();
  plan.submit();
  const review = plan.review, before = plan.brief()?.approval_epoch;
  expect(typeof before).toBe("string");
  plan.refresh();
  expect(plan.brief()?.approval_epoch).toBe(before);

  // The first notification of preparation already carries the new epoch, before the ACK.
  const published: ReturnType<PlanMode["brief"]>[] = [];
  plan.onChange(() => { published.push(plan.brief()); });
  expect(plan.choose("execute", undefined, true)).toBe("preparing");
  const attempt = plan.brief()?.approval_epoch;
  expect(typeof attempt).toBe("string");
  expect(attempt).not.toBe(before);
  expect(published[0]).toMatchObject({ preparing: "keep", approval_epoch: attempt });
  await settle();
  expect(plan.review).toBe(review);
  expect(plan.brief()?.preparing).toBeUndefined();
  expect(plan.brief()?.approval_epoch).toBe(attempt);
  expect(notes.join("\n")).toContain("The plan was not started");
  expect(plan.brief()?.approval_error).toContain("The plan was not started");
  expect(plan.brief()?.approval_pending).toBeUndefined();

  expect(plan.choose("execute", undefined, true)).toBe("preparing");
  const retry = plan.brief()?.approval_epoch;
  expect(plan.brief()?.approval_failures?.find(failure => failure.epoch === attempt)?.error).toContain("The plan was not started");
  expect(typeof retry).toBe("string");
  expect(retry).not.toBe(attempt);
  expect(retry).not.toBe(before);
  await settle();
  expect(plan.brief()?.approval_epoch).toBe(retry);
  const host = plan.brief()?.approval_host;
  expect(noticeIds).toEqual([`plan-failure-${host}-${attempt}`, `plan-failure-${host}-${retry}`]);
});

function planStatusHandler(plan: PlanMode) {
  const registry = new RemoteRegistry();
  registerPlanMode(registry, plan);
  const handler = registry.get("plan_status")!;
  return (args: Record<string, unknown>) => handler(args) as { plan_status: PlanStatus };
}

test("plan_status reads the current host and exact card even through a restart and closure", async () => {
  const { plan, restarted, sent, journal, notes } = await planning({ kickoff: "taken" });
  plan.submit();
  const card = plan.review!;
  const handler = planStatusHandler(plan);
  const initial = handler({ card }).plan_status;
  expect(initial).toEqual({ card, host: plan.brief()?.approval_host, epoch: "0", open: true, pending: false, failures: [] });
  expect(await plan.choose("execute")).toBe("started");
  const pending = handler({ card }).plan_status;
  expect(pending).toMatchObject({ card, host: initial.host, open: true, pending: true });
  expect(pending.epoch).not.toBe(initial.epoch);
  expect(handler({ card: "different-card" }).plan_status).toEqual({ card: "different-card", host: initial.host,
    epoch: pending.epoch, open: false, pending: false, failures: [] });

  const again = restarted();
  const probe = planStatusHandler(again);
  let publications = 0;
  again.onChange(() => { publications++; });
  const afterRestart = probe({ card }).plan_status;
  expect(afterRestart).toMatchObject({ card, epoch: "0", open: true, pending: false, failures: [] });
  expect(afterRestart.host).not.toBe(initial.host);
  expect(probe({ card }).plan_status).toEqual(afterRestart);
  for (const invalid of [undefined, "", "bad/name", "card\n", "é", "x".repeat(161), 123]) {
    expect(() => probe({ card: invalid })).toThrow("invalid plan review card");
  }
  expect(probe({ card: "x".repeat(160) }).plan_status.open).toBeFalse();
  expect(publications).toBe(0);
  expect(sent).toHaveLength(1);
  expect(journal).toEqual([]);
  expect(notes).toEqual([]);

  again.leave();
  expect(probe({ card }).plan_status).toEqual({ ...afterRestart, open: false });
});

test("plan_status returns recent failures only for the requested card", async () => {
  const { plan } = await planning({ kickoff: "refused" });
  plan.submit();
  const card = plan.review!;
  const probe = planStatusHandler(plan);
  expect(plan.choose("execute", undefined, true)).toBe("preparing");
  const attempt = probe({ card }).plan_status;
  await settle();
  const failed = probe({ card }).plan_status;
  expect(failed).toMatchObject({ epoch: attempt.epoch, pending: false, open: true });
  expect(failed.error).toContain("The plan was not started");
  expect(failed.failures).toEqual([{ epoch: attempt.epoch, error: failed.error }]);
  await plan.choose("revise");
  plan.submit();
  expect(probe({ card: plan.review }).plan_status).toMatchObject({ open: true, pending: false, failures: [] });
  expect(probe({ card }).plan_status).toEqual({ ...attempt, open: false, pending: false, failures: failed.failures });
});

test("an earlier turn settling during the kickoff's admission does not lose the approval's record", async () => {
  const { plan, journal } = await planning({ kickoff: "after-a-settle" });
  plan.submit();
  expect(await plan.choose("execute")).toBe("started");
  expect(plan.phase).toBe("off");
  expect(journal).toEqual(["Approved plan: Same plan (revision 1, context kept)"]);
});

test("a host that restarts during a review still has it, and records its approval", async () => {
  const { plan, journal, restarted } = await planning();
  plan.submit();
  const review = plan.review, copy = plan.record.path, host = plan.brief()?.approval_host;
  // Given a new host over the same transcript, which has told its views nothing yet:
  const again = restarted();
  expect(again.review).toBe(review);
  expect(typeof again.brief()?.approval_epoch).toBe("string");
  expect(again.brief()?.approval_host).not.toBe(host);
  // When the owner executes there:
  expect(await again.choose("execute")).toBe("started");
  // Then the plan is approved and the Build record says so.
  expect(again.approved?.path).toBe(copy);
  expect(journal).toEqual(["Approved plan: Same plan (revision 1, context kept)"]);
});

test("a turn during a review that follows a compaction is given the contract again", async () => {
  const { plan, add } = await planning();
  const told = () => {
    const message = plan.turnMessage();
    if (message) add({ type: "custom_message", customType: PLAN_NOTE, details: message.details });
    return message?.details.kinds;
  };
  expect(told()).toEqual(["contract"]);
  plan.submit();
  expect(told()).toBeUndefined();
  // The read-only rules still hold, and the summary is not the contract.
  add({ type: "compaction" });
  expect(told()).toEqual(["contract"]);
  expect(told()).toBeUndefined();
});
