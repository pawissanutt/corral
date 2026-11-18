import { expect, test } from "bun:test";
import { parseRunFlags } from "./config.ts";


test("flags name the engine run or task reviewer without a legacy launch message", () => {
  const flags = ["--dir", "runs/T-1/R1", "--socket", "socket", "--prompt-file", "prompt.md",
    "--corral", "/test/corral-next", "--model", "anthropic/claude-opus-4-8", "--effort", "high"];
  const env = { CORRAL_NEXT_ROOT: "/fixture", CORRAL_TASK: "T-1", CORRAL_RUN: "R1" };
  const run = parseRunFlags("run", flags, "/assets", { ...env, CORRAL_ACTOR: "agent" }, "/fixture/work");
  expect(run.identity).toMatchObject({ role: "worker", task: "T-1", run: "R1" });
  expect(run.local).toMatchObject({ id: "T-1/R1", dir: "/fixture/work/runs/T-1/R1", socket: "/fixture/work/socket" });
  expect(run).toMatchObject({ root: "/fixture", corral: "/test/corral-next", provider: "anthropic", effort: "high",
    promptFile: "/fixture/work/prompt.md" });
  expect(parseRunFlags("run", flags, "/assets", { ...env, CORRAL_ACTOR: "reviewer" }, "/fixture/work").local?.id)
    .toBe("T-1/reviewer");
});
