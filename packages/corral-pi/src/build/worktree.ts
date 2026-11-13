import { spawnSync } from "node:child_process";
import { existsSync, lstatSync } from "node:fs";
import { join } from "node:path";

/** Git is always invoked with argv; its stderr survives every refusal. */
export function prepareWorktree(repoRoot: string, name: string, here: boolean): { worktree: string; branch: string | null; base: string } {
  const git = (args: string[], allowFailure = false) => {
    const result = spawnSync("git", ["-C", repoRoot, ...args], { encoding: "utf8" });
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
