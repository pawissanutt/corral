import { afterEach, expect, test } from "bun:test";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PLAN_ENTRY, PLAN_NOTE, PlanMode, type PlanEntry } from "./plan-mode.ts";

const dirs: string[] = [];
afterEach(() => { for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true }); });

/** A plan mode over an in-memory branch, already planning with a plan file written. */
async function planning(session: { compacting?: boolean; takes?: boolean } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "plan-mode-unit-"));
  dirs.push(dir);
  const entries: PlanEntry[] = [], notes: string[] = [];
  let leaf = 0;
  const add = (entry: PlanEntry) => { entries.push(entry); leaf++; };
  const plan = new PlanMode({
    dir, cwd: dir, prompts: { contract: () => "CONTRACT", reset: () => "", approved: () => "" }, askUser: false, todo: false,
    entries: () => entries, leaf: () => String(leaf), append: record => { add({ type: "custom", customType: PLAN_ENTRY, data: record }); },
    busy: () => undefined, compacting: () => session.compacting === true, stopTurn: async () => {}, compact: async () => {},
    reset: async () => {}, submit: async () => session.takes !== false, model: () => undefined, changed: () => {},
    note: text => { notes.push(text); },
  });
  await plan.enter();
  writeFileSync(plan.path, "# Same plan\n");
  return { plan, entries, notes, add, rewind: (length: number) => { entries.length = length; leaf++; } };
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
  await new Promise(resolve => setTimeout(resolve, 0));
  // Then nothing is approved and the review stays open.
  expect(plan.phase).toBe("review");
  expect(notes.join("\n")).toContain("missing or was changed");
});

test("an approval the session cannot start leaves the same review open", async () => {
  // During a compaction the session takes no turn, so the choice is refused before anything changes.
  const compacting = await planning({ compacting: true });
  compacting.plan.submit();
  expect(() => compacting.plan.choose("execute")).toThrow("Wait for the running compaction to finish");
  expect(compacting.plan.phase).toBe("review");

  // A kickoff the session does not take puts the submission back under the same card.
  const refusing = await planning({ takes: false });
  refusing.plan.submit();
  const review = refusing.plan.review;
  expect(refusing.plan.choose("execute")).toBe("started");
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(refusing.plan.review).toBe(review);
  expect(refusing.notes.join("\n")).toContain("The review is open again");
});

test("a reply to a review after a compaction is told with the contract, in one turn", async () => {
  const { plan, add } = await planning();
  const told = () => {
    const message = plan.turnMessage();
    if (message) add({ type: "custom_message", customType: PLAN_NOTE, details: message.details });
    return message;
  };
  expect(told()?.details.kinds).toEqual(["contract"]);
  plan.submit();
  add({ type: "compaction" });
  plan.ownerSpoke();
  const message = told();
  expect(message?.details.kinds).toEqual(["contract", "replied"]);
  expect(message?.content).toContain("The owner replied instead of approving");
  // The next turn has nothing left to be told.
  expect(plan.turnMessage()).toBeUndefined();
});
