import { spawnSync } from "node:child_process";
import { existsSync, lstatSync } from "node:fs";
import { join } from "node:path";

/** Git is always invoked with argv; its stderr survives every refusal. */
export function prepareWorktree(repoRoot: string, name: string, here: boolean, writerFd?: number): { worktree: string; branch: string | null; base: string } {
  const git = (args: string[], allowFailure = false) => {
    const result = spawnSync("git", ["-C", repoRoot, ...args], { encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe", ...(writerFd === undefined ? [] : [writerFd])] });
    if (result.error) throw new Error(`git ${args.join(" ")}: ${result.error.message}; ${result.stderr ?? ""}`);
    if (result.status !== 0 && !allowFailure) throw new Error(`git ${args.join(" ")}: ${result.stderr.trim() || `exit ${result.status}`}`);
    return result;
  };
  const base = git(["rev-parse", "HEAD"]).stdout.trim();
  if (here) {
    const branch = git(["symbolic-ref", "--quiet", "--short", "HEAD"], true);
    if (branch.status !== 0 && branch.status !== 1) throw new Error(branch.stderr.trim());
    return { worktree: repoRoot, branch: branch.status === 0 ? branch.stdout.trim() : null, base };
  }
  if (!/^[a-z0-9-]{1,40}$/.test(name)) throw new Error("Invalid build worktree name");
  const worktree = join(repoRoot, ".worktrees", `build-${name}`);
  // lstat also catches a dangling symlink, which existsSync deliberately ignores.
  let occupied = existsSync(worktree);
  try { lstatSync(worktree); occupied = true; } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  if (occupied) throw new Error(`Build worktree already exists: ${worktree}`);
  const branch = `build/${name}`;
  const existing = git(["show-ref", "--verify", "--quiet", `refs/heads/${branch}`], true);
  if (existing.status === 0) throw new Error(`Build branch already exists: ${branch}`);
  if (existing.status !== 1) throw new Error(existing.stderr.trim());
  git(["worktree", "add", "-b", branch, worktree, "HEAD"]);
  return { worktree, branch, base };
}

/** Inspect the invocation directory without creating or modifying anything in Git. */
export function prepareCodeCheckout(repoRoot: string, cwd: string): Pick<import("./types.ts").BuildSessionRecord,
  "worktree" | "branch" | "base" | "baseBranch" | "repository"> {
  const git = (args: string[]) => {
    const result = spawnSync("git", ["-C", cwd, ...args], { encoding: "utf8" });
    if (result.error) throw result.error;
    return result;
  };
  const top = git(["rev-parse", "--show-toplevel"]);
  if (top.status !== 0 && top.stderr.includes("not a git repository") && repoRoot === cwd) {
    return { worktree: cwd, branch: null, base: null, repository: false };
  }
  if (top.status !== 0 || top.stdout.trim() !== repoRoot) throw new Error(top.stderr.trim() || "The Git checkout changed during launch.");
  const branch = git(["symbolic-ref", "--quiet", "--short", "HEAD"]);
  if (branch.status !== 0 && branch.status !== 1) throw new Error(branch.stderr.trim() || "Cannot read the current branch.");
  const head = git(["rev-parse", "--verify", "--quiet", "HEAD"]);
  // On an unborn branch symbolic-ref succeeds but HEAD has no commit yet.
  if (head.status !== 0 && (head.status !== 1 || branch.status !== 0)) throw new Error(head.stderr.trim() || "Cannot read HEAD.");
  const name = branch.status === 0 ? branch.stdout.trim() : null;
  return { worktree: cwd, branch: name, baseBranch: name, base: head.status === 0 ? head.stdout.trim() : null, repository: true };
}
