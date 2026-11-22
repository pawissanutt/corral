import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checked, gitRun, type Run } from "../src/build/corral-cli.ts";
import { PreparationError, rebaseCandidate } from "../src/build/prepare.ts";

async function fixture(run: (f: {
  cwd: string; source: string; base: string; git: (...args: string[]) => Promise<string>;
  exec: Run;
}) => Promise<void>, backend = "merge") {
  const cwd = mkdtempSync(join(tmpdir(), "build-rebase-owner-"));
  const exec = gitRun();
  const git = (...args: string[]) => checked(exec, args, cwd);
  const change = (value: string) => {
    for (const file of ["one.txt", "two.txt"]) writeFileSync(join(cwd, file), value + "\n");
  };
  try {
    await git("init", "-b", "main");
    await git("config", "user.name", "Build Fixture");
    await git("config", "user.email", "build@example.test");
    await git("config", "rebase.backend", backend);
    await git("config", "core.logAllRefUpdates", "true");
    change("common ancestor");
    await git("add", ".");
    await git("commit", "-m", "common ancestor");
    await git("branch", "candidate");
    change("main change");
    await git("commit", "-am", "main change");
    const base = (await git("rev-parse", "HEAD")).trim();
    await git("checkout", "candidate");
    writeFileSync(join(cwd, "clean.txt"), "clean pick\n");
    await git("add", "clean.txt");
    await git("commit", "-m", "clean pick");
    change("candidate change");
    await git("commit", "-am", "candidate change");
    const source = (await git("rev-parse", "HEAD")).trim();
    await run({ cwd, source, base, git, exec });
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
}

async function failure(run: Promise<void>): Promise<PreparationError> {
  try { await run; }
  catch (error) {
    if (error instanceof PreparationError) return error;
    throw error;
  }
  throw new Error("Expected preparation to fail");
}

test("a conflicted rebase proved to be Corral's own is aborted and restored", () => fixture(async ({ cwd, source, base, git, exec }) => {
  const result = await failure(rebaseCandidate(exec, cwd, base, source));
  expect(result.status).toEqual({ state: "conflicts", conflicts: ["one.txt", "two.txt"] });
  expect(result.message).toContain("the branch is unchanged");
  expect((await git("symbolic-ref", "HEAD")).trim()).toBe("refs/heads/candidate");
  expect((await git("rev-parse", "HEAD")).trim()).toBe(source);
  expect(await git("status", "--porcelain")).toBe("");
}));

test("an own conflict is also safely aborted with the apply backend", () => fixture(async ({ cwd, source, base, git, exec }) => {
  const result = await failure(rebaseCandidate(exec, cwd, base, source));
  expect(result.status.state).toBe("conflicts");
  expect((await git("rev-parse", "HEAD")).trim()).toBe(source);
  expect(await git("status", "--porcelain")).toBe("");
}, "apply"));

async function stageExternalResolution(cwd: string, base: string, git: (...args: string[]) => Promise<string>, exec: Run) {
  // Same base and source as Corral: matching the operation metadata alone cannot prove ownership.
  const external = await exec(["rebase", base], { cwd });
  expect(external.code).not.toBe(0);
  expect(await git("diff", "--name-only", "--diff-filter=U")).toBe("one.txt\ntwo.txt\n");
  writeFileSync(join(cwd, "one.txt"), "user resolution\n");
  await git("add", "one.txt");
}

test("an external rebase started just before Corral invokes Git retains staged conflict resolution", () => fixture(async ({ cwd, source, base, git, exec }) => {
  let attempted = false;
  const run: Run = async (args, options) => {
    if (!attempted && args.includes("rebase")) {
      attempted = true;
      await stageExternalResolution(cwd, base, git, exec);
    }
    return exec(args, options);
  };
  const result = await failure(rebaseCandidate(run, cwd, base, source));
  expect(attempted).toBeTrue();
  expect(result.status).toEqual({ state: "failed", conflicts: ["two.txt"] });
  expect(result.message).toContain("did not abort");
  expect(result.message).not.toContain("unchanged");
  expect(await git("diff", "--name-only", "--diff-filter=U")).toBe("two.txt\n");
  expect(await git("show", ":one.txt")).toBe("user resolution\n");
  expect(readFileSync(join(cwd, "one.txt"), "utf8")).toBe("user resolution\n");
  expect(readFileSync(join(cwd, ".git", "rebase-merge", "orig-head"), "utf8").trim()).toBe(source);
}));

test("a rebase replaced while the ownership proof is being read is not aborted", () => fixture(async ({ cwd, source, base, git, exec }) => {
  const run: Run = async (args, options) => {
    const result = await exec(args, options);
    if (args[0] === "reflog") {
      // Return the once-valid proof only after another process replaced that rebase.
      expect(result.stdout).toContain("corral-prepare-");
      await git("rebase", "--abort");
      await stageExternalResolution(cwd, base, git, exec);
    }
    return result;
  };
  const result = await failure(rebaseCandidate(run, cwd, base, source));
  expect(result.status.state).toBe("failed");
  expect(result.message).toContain("did not abort");
  expect(await git("show", ":one.txt")).toBe("user resolution\n");
}));

test("unavailable ownership evidence leaves a conflicted rebase for manual recovery", () => fixture(async ({ cwd, source, base, git, exec }) => {
  const run: Run = (args, options) => args[0] === "reflog"
    ? Promise.resolve({ code: 1, stdout: "", stderr: "no reflog" })
    : exec(args, options);
  const result = await failure(rebaseCandidate(run, cwd, base, source));
  expect(result.status.state).toBe("failed");
  expect(result.message).toContain("did not abort");
  expect(await git("diff", "--name-only", "--diff-filter=U")).toBe("one.txt\ntwo.txt\n");
}));
