import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gitRun } from "./corral-cli.ts";
import { BRIEF_LIMIT, previousReviewBrief, reviewBrief } from "./review-brief.ts";

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

test("a follow-up reviewer sees dispositions and the actual repair patch before the original build context", async () => {
  const cwd = mkdtempSync(join(tmpdir(), "review-delta-"));
  const git = (...args: string[]) => execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
  const commit = (message: string) => git("-c", "user.name=Reviewer Test", "-c", "user.email=review-test@example.test", "commit", "-qam", message);
  try {
    git("init", "-q", "-b", "main");
    git("-c", "user.name=Reviewer Test", "-c", "user.email=review-test@example.test", "commit", "--allow-empty", "-qm", "Base");
    const base = git("rev-parse", "HEAD");
    const path = join(cwd, "parser.ts");
    writeFileSync(path, "parse unsafely\n");
    git("add", "parser.ts");
    commit("Initial candidate");
    const priorHead = git("rev-parse", "HEAD");
    writeFileSync(path, "parse after validating\n");
    commit("Repair");
    const head = git("rev-parse", "HEAD");
    const previous = await previousReviewBrief(gitRun(), cwd, {
      round: 2, reviewer: "review-2", model: "provider/model", base, head: priorHead, verdict: "changes", at: "2026-01-01",
      findings: [
        { id: "F1", severity: "blocker", path: "parser.ts", line: 1, text: "Validate input.", resolution: { state: "fixed", commit: head } },
        { id: "F2", severity: "minor", text: "Rename the file.", resolution: { state: "declined", reason: "Public entry point" } },
        { id: "F3", severity: "major", text: "Check trailing input." },
      ],
    }, head, base);
    const brief = reviewBrief({ ...parts(["Original request"], ["x".repeat(BRIEF_LIMIT)]), previous });
    expect(brief.length).toBeLessThanOrEqual(BRIEF_LIMIT);
    expect(brief).toContain(`F1 [blocker] parser.ts:1: Validate input.\nDisposition: fixed at ${head}`);
    expect(brief).toContain("F2 [minor] Rename the file.\nDisposition: declined — Public entry point");
    expect(brief).toContain("F3 [major] Check trailing input.\nDisposition: open");
    expect(brief).toContain("-parse unsafely\n+parse after validating");
    expect(brief.indexOf("+parse after validating") < brief.indexOf("Original request")).toBe(true);
  } finally { rmSync(cwd, { recursive: true, force: true }); }
});
