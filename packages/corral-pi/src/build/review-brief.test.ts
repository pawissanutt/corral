import { expect, test } from "bun:test";
import { BRIEF_LIMIT, reviewBrief } from "./review-brief.ts";

const parts = (prompts: string[], slices: string[], head = ["Review it.", "Worktree: /w"]) =>
  ({ head, prompts, slices, tail: ["Latest gate: null", "Finish by calling review_verdict."], record: "/data/journal.json" });

test("a build whose slice reports outgrow the brief limit still gets a reviewer brief", () => {
  const brief = reviewBrief(parts(["[t]\nkeep me whole"], Array.from({ length: 7 }, (_, n) => `s${n}: title\nReport:\n${"r".repeat(9000)}`)));
  expect(brief.length).toBeLessThanOrEqual(BRIEF_LIMIT);
  expect(brief).toContain("keep me whole");
  expect(brief).toContain("s6: title");
  expect(brief).toContain("characters omitted");
  expect(brief).toContain("/data/journal.json");
  expect(brief.endsWith("Finish by calling review_verdict.")).toBeTrue();
});

test("a brief that fits exactly is whole, with no shortening notice", () => {
  const empty = reviewBrief(parts([], [""])).length;
  const brief = reviewBrief(parts([], ["x".repeat(BRIEF_LIMIT - empty)]));
  expect(brief.length).toBe(BRIEF_LIMIT);
  expect(brief).not.toContain("shortened");
});

test("too many reports for even their omission notes still yield a brief within the limit", () => {
  const head = ["Review it.", "f".repeat(BRIEF_LIMIT - 2000)];
  const brief = reviewBrief(parts([], Array.from({ length: 300 }, (_, n) => `s${n}: ${"r".repeat(500)}`), head));
  expect(brief.length).toBeLessThanOrEqual(BRIEF_LIMIT);
});
