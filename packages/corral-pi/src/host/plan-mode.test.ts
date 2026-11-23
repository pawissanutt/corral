import { afterEach, expect, test } from "bun:test";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PLAN_ENTRY, PLAN_KICKOFF, PLAN_NOTE, PlanMode, type PlanEntry, type PlanHost } from "./plan-mode.ts";

const dirs: string[] = [];
afterEach(() => { for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true }); });
const settle = () => new Promise(resolve => setTimeout(resolve, 0));

/**
 * A plan mode over an in-memory branch, already planning with a plan file written. `kickoff` says what the session
 * does with an approved plan's kickoff: it reaches the transcript unless it is only `taken`, `refused` or `throws`.
 */
async function planning(session: { blocked?: string; kickoff?: "taken" | "refused" | "throws" | "after-a-settle";
  stopping?: () => void } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "plan-mode-unit-"));
  dirs.push(dir);
  const entries: PlanEntry[] = [], notes: string[] = [], journal: string[] = [];
  const sent: { text: string; kickoff?: string }[] = [];
  let leaf = 0;
  const add = (entry: PlanEntry) => { entries.push(entry); leaf++; };
  /** A user message in the transcript; the host alone tags the one it admitted as a kickoff. */
  const user = (text: string, kickoff?: string) => {
    add({ type: "message", message: { role: "user", content: text, ...(kickoff === undefined ? {} : { [PLAN_KICKOFF]: kickoff }) } });
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
    model: () => undefined, changed: () => {}, note: text => { notes.push(text); }, approved: summary => { journal.push(summary); },
  };
  const plan = new PlanMode(host);
  // The instance the host is running: a restart replaces it over the same transcript.
  let running = plan;
  await plan.enter();
  writeFileSync(plan.path, "# Same plan\n");
  return { plan, notes, journal, sent, add, user, restarted: () => running = new PlanMode(host),
    rewind: (length: number) => { entries.length = length; leaf++; } };
}

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
  const { plan, sent, notes, rewind } = await planning(session);
  plan.submit();
  // Given the owner rewinds to before the submission while the approval waits for the submitting turn to stop:
  session.stopping = () => rewind(1);
  expect(plan.choose("execute")).toBe("started");
  await settle();
  // Then the kickoff of a review that is gone is not sent into the rewound branch.
  expect(sent).toEqual([]);
  expect(plan.phase).toBe("planning");
  expect(notes.join("\n")).toContain("no longer open");
});

test("a rewind from one pending review to another tells the views, though the phase is the same", async () => {
  const { plan, rewind } = await planning();
  plan.submit();
  const first = plan.review;
  plan.ownerSpoke();
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

test("a submission whose copy changed after it was shown is not executed", async () => {
  const { plan, notes } = await planning();
  plan.submit();
  // Given the review has already read the copy:
  expect(plan.card()?.card.body).toBe("# Same plan\n");
  writeFileSync(plan.record.path ?? "", "# Another plan\n");
  // When the owner executes:
  expect(plan.choose("execute")).toBe("started");
  await settle();
  // Then nothing is approved and the review stays open.
  expect(plan.phase).toBe("review");
  expect(notes.join("\n")).toContain("missing or was changed");
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
  expect(pausing.plan.choose("execute_compact")).toBe("started");
  await settle();
  expect(pausing.plan.phase).toBe("review");
  expect(pausing.notes.join("\n")).toContain("The session is paused");

  // A kickoff the session does not take, or fails to take, leaves the submission under the same card.
  for (const kickoff of ["refused", "throws"] as const) {
    const { plan, notes } = await planning({ kickoff });
    plan.submit();
    const review = plan.review;
    expect(plan.choose("execute")).toBe("started");
    await settle();
    expect(plan.review).toBe(review);
    expect(notes.join("\n")).toContain("The review is still open");
  }
});

test("an approval is durable only with its kickoff, and holds the mode until then", async () => {
  // Given a review after a compaction, and a kickoff the session took but has not yet written to the transcript:
  const { plan, journal, sent, add, user, restarted } = await planning({ kickoff: "taken" });
  plan.submit();
  add({ type: "compaction" });
  const review = plan.review, copy = plan.record.path;
  expect(plan.choose("execute")).toBe("started");
  await settle();
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
  const review = refusing.plan.review;
  // The answer is refused, so whoever holds the owner's text keeps it.
  await expect(Promise.resolve(refusing.plan.choose("revise", "Use a flag parser"))).rejects.toThrow("did not take the feedback");
  expect(refusing.plan.review).toBe(review);

  // Feedback the session takes withdraws the review when it reaches the model, like any owner message.
  const taking = await planning();
  taking.plan.submit();
  expect(await taking.plan.choose("revise", "Use a flag parser")).toBe("delivered");
  expect(taking.sent[0]?.text).toContain("Use a flag parser");
  // Until it does, no other answer can override it.
  expect(taking.plan.phase).toBe("review");
  expect(() => taking.plan.choose("execute")).toThrow("already being answered");
  taking.plan.ownerSpoke();
  expect(taking.plan.phase).toBe("planning");
});

test("an earlier turn settling during the kickoff's admission does not lose the approval's record", async () => {
  const { plan, journal } = await planning({ kickoff: "after-a-settle" });
  plan.submit();
  expect(plan.choose("execute")).toBe("started");
  await settle();
  expect(plan.phase).toBe("off");
  expect(journal).toEqual(["Approved plan: Same plan (revision 1, context kept)"]);
});

test("a host that restarts during a review still has it, and records its approval", async () => {
  const { plan, journal, restarted } = await planning();
  plan.submit();
  const review = plan.review, copy = plan.record.path;
  // Given a new host over the same transcript, which has told its views nothing yet:
  const again = restarted();
  expect(again.review).toBe(review);
  // When the owner executes there:
  expect(again.choose("execute")).toBe("started");
  await settle();
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
