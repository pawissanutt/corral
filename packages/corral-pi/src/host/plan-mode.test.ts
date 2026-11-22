import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PLAN_ENTRY, PlanMode, type PlanEntry } from "./plan-mode.ts";

test("a plan resubmitted after a rewind is its own card and its own copy", async () => {
  const dir = mkdtempSync(join(tmpdir(), "plan-mode-unit-"));
  try {
    const entries: PlanEntry[] = [];
    let leaf = 0;
    const plan = new PlanMode({
      dir, cwd: dir, prompts: { contract: () => "", reset: () => "", approved: () => "" }, askUser: false, todo: false,
      entries: () => entries, leaf: () => String(leaf),
      append: record => { entries.push({ type: "custom", customType: PLAN_ENTRY, data: record }); leaf++; },
      busy: () => undefined, stopTurn: async () => {}, compact: async () => {}, reset: async () => {},
      submit: async () => true, model: () => undefined, changed: () => {}, note: () => {},
    });
    await plan.enter();
    writeFileSync(plan.path, "# Same plan\n");
    plan.submit();
    const first = plan.review, firstCopy = plan.record.path;
    // A rewind to before the submission rolls the revision back with the branch.
    entries.length = 1;
    leaf++;
    plan.submit();
    expect(plan.record.revision).toBe(1);
    expect(plan.review).not.toBe(first);
    expect(plan.record.path).not.toBe(firstCopy);
    expect(existsSync(firstCopy ?? "")).toBeTrue();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
