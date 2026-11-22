import { afterEach, expect, test } from "bun:test";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PLAN_ENTRY, PLAN_NOTE, PlanMode, type PlanEntry, type PlanHost } from "./plan-mode.ts";

const dirs: string[] = [];
afterEach(() => { for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true }); });
const settle = () => new Promise(resolve => setTimeout(resolve, 0));

/**
 * A plan mode over an in-memory branch, already planning with a plan file written. `kickoff` says what the session
 * does with an approved plan's kickoff: it reaches the transcript unless it is only `taken`, `refused` or `throws`.
 */
async function planning(session: { blocked?: string; kickoff?: "taken" | "refused" | "throws" } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "plan-mode-unit-"));
  dirs.push(dir);
  const entries: PlanEntry[] = [], notes: string[] = [], sent: string[] = [];
  let leaf = 0;
  const add = (entry: PlanEntry) => { entries.push(entry); leaf++; };
  const host: PlanHost = {
    dir, cwd: dir, prompts: { contract: () => "CONTRACT", reset: () => "", approved: () => "EXECUTE" }, askUser: false, todo: false,
    entries: () => entries, leaf: () => String(leaf), append: record => { add({ type: "custom", customType: PLAN_ENTRY, data: record }); },
    busy: () => undefined, blocked: () => session.blocked, stopTurn: async () => {},
    // The owner pauses while the approved plan's context is being summarised.
    compact: async () => { session.blocked = "The session is paused"; }, reset: async () => {},
    submit: async text => {
      if (session.kickoff === "throws") throw new Error("the held queue could not be saved");
      if (session.kickoff === "refused") return false;
      sent.push(text);
      if (session.kickoff !== "taken") add({ type: "message", message: { role: "user", content: text } });
      return true;
    },
    model: () => undefined, changed: () => {}, note: text => { notes.push(text); },
  };
  const plan = new PlanMode(host);
  await plan.enter();
  writeFileSync(plan.path, "# Same plan\n");
  return { plan, notes, sent, add, restarted: () => new PlanMode(host), rewind: (length: number) => { entries.length = length; leaf++; } };
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
  // Given a kickoff the session took but has not yet written to the transcript:
  const { plan, sent, add, restarted } = await planning({ kickoff: "taken" });
  plan.submit();
  const review = plan.review, copy = plan.record.path;
  expect(plan.choose("execute")).toBe("started");
  await settle();
  // Then nothing else decides or changes the mode meanwhile,
  expect(plan.choose("execute_clean")).toBe("started");
  expect(() => plan.leave()).toThrow("The approved plan is starting");
  expect(sent).toHaveLength(1);
  // and a host that stops here has approved nothing: the same review is open after a restart.
  expect(restarted().review).toBe(review);
  // When the kickoff is in the transcript, that is the approval.
  add({ type: "message", message: { role: "user", content: sent[0] } });
  expect(plan.phase).toBe("off");
  expect(plan.approved?.path).toBe(copy);
  expect(restarted().approved?.path).toBe(copy);
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
