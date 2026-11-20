import { expect, test } from "bun:test";
import { BRIEF_LIMIT, reviewBrief } from "./review-brief.ts";

test("a build whose slice reports outgrow the brief limit still gets a reviewer brief", () => {
  const parts = { head: ["Review it.", "Worktree: /w"], prompts: ["[t]\nkeep me whole"],
    slices: Array.from({ length: 7 }, (_, n) => `s${n}: title\nReport:\n${"r".repeat(9000)}`),
    tail: ["Latest gate: null", "Finish by calling review_verdict."], record: "/data/journal.json" };
  const brief = reviewBrief(parts);
  expect(brief.length).toBeLessThanOrEqual(BRIEF_LIMIT);
  expect(brief).toContain("keep me whole");
  expect(brief).toContain("s6: title");
  expect(brief).toContain("characters omitted");
  expect(brief).toContain("/data/journal.json");
  expect(brief.endsWith("Finish by calling review_verdict.")).toBeTrue();
});
