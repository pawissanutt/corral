// Git preconditions shared by explicit preparation and the read-only checks before landing.
import { lstatSync } from "node:fs";
import { join } from "node:path";
import { checked, type Run } from "./corral-cli.ts";
import { checkCheckoutSupport, checkLandingEnvironment } from "./fast-forward.ts";
import { checkIndexVisibility } from "./index-visibility.ts";
import { buildProject, gateInputFingerprint } from "./gate.ts";
import { landingBaseFromRef, landingBaseLookups } from "./git.ts";
import type { BuildContext, LandCandidate } from "./types.ts";

export const PREPARATION_NEEDED = "Run build_finish to prepare and offer a fresh landing candidate.";
export const short = (sha: string) => sha.slice(0, 7);
export const message = (error: unknown) => error instanceof Error ? error.message : String(error);

export class PreparationError extends Error {
  constructor(detail: string, readonly status: Partial<LandCandidate> = {}) { super(detail); }
}

/** An otherwise valid offered pin moved; the finish coordinator may prepare a fresh offer, never land it. */
export class CandidateMoved extends PreparationError {}

export interface LandingTarget {
  readonly name: string;
  readonly ref: string;
  readonly configuredName?: string;
  readonly commands: readonly string[];
  readonly gateInputFingerprint: string;
}
export function sameCommands(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((command, i) => command === b[i]);
}

export function landingDisabled(context: BuildContext): string | undefined {
  const { session, settings } = context;
  if (session.bare) return session.repository === false ? "This session is not in a Git repository." : undefined;
  if (!session.workspaceRoot) return "This session is outside a corral workspace.";
  if (!session.project) return "This session has no configured corral project.";
  if (settings.exitPolicy === "none") return "Project policy disables landing.";
  return undefined;
}

function targetConfig({ session }: BuildContext): Pick<LandingTarget, "configuredName" | "commands" | "gateInputFingerprint"> {
  const project = session.bare ? {} : buildProject(session);
  const commands = project.gate ?? [];
  if (!Array.isArray(commands) || commands.some(command => typeof command !== "string")) throw new Error("The project gate must be an array of commands.");
  const configuredName = session.bare ? session.baseBranch : project.base;
  if (configuredName != null && typeof configuredName !== "string") throw new Error("The configured base branch must be a branch name.");
  return { configuredName: configuredName || undefined, commands: [...commands], gateInputFingerprint: gateInputFingerprint(project) };
}

export function checkTargetConfig(context: BuildContext, target: LandingTarget): void {
  const current = targetConfig(context);
  if (current.configuredName !== target.configuredName) throw new PreparationError(`The configured base branch changed. ${PREPARATION_NEEDED}`);
  if (target.gateInputFingerprint !== current.gateInputFingerprint) throw new PreparationError(`The configured gate inputs changed. ${PREPARATION_NEEDED} Run build_verify for the new commands.`);
}

export async function landingTarget(git: Run, context: BuildContext): Promise<LandingTarget> {
  const { session } = context;
  const config = targetConfig(context);
  let name = config.configuredName;
  if (!session.bare) for (const lookup of landingBaseLookups) {
    if (name) break;
    const result = await git(lookup, { cwd: session.repoRoot });
    if (result.code === 0) name = landingBaseFromRef(result.stdout, lookup);
  }
  if (!name) throw new Error(session.bare ? "The session started without a base branch." : "The project's base branch is not configured.");
  const ref = `refs/heads/${name}`;
  await checked(git, ["check-ref-format", ref], session.worktree);
  return { name, ref, ...config };
}

const SHA = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;
export async function commit(git: Run, rev: string, cwd: string): Promise<string> {
  const sha = (await checked(git, ["rev-parse", "--verify", rev], cwd)).trim();
  if (!SHA.test(sha)) throw new Error(`Git did not return a full commit ID for ${rev}.`);
  return sha;
}
export async function sourceBranch(git: Run, cwd: string): Promise<string> {
  const result = await git(["symbolic-ref", "--quiet", "HEAD"], { cwd });
  if (result.code === 1) throw new Error("A detached session has no branch to land.");
  if (result.code !== 0) throw new Error(result.stderr.trim() || "Cannot read the session branch.");
  const branch = result.stdout.trim();
  if (!branch.startsWith("refs/heads/")) throw new Error("The session is not on a local branch.");
  return branch;
}
export async function ancestor(git: Run, base: string, head: string, cwd: string): Promise<boolean> {
  if (base === head) return true;
  const result = await git(["merge-base", "--is-ancestor", base, head], { cwd });
  if (result.code === 0) return true;
  if (result.code === 1 && !result.stderr) return false;
  throw new Error(result.stderr.trim() || "Cannot verify fast-forward ancestry.");
}

const OPERATIONS: readonly [marker: string, name: string][] = [
  ["rebase-apply", "rebase or am"], ["rebase-merge", "rebase"], ["MERGE_HEAD", "merge"],
  ["CHERRY_PICK_HEAD", "cherry-pick"], ["REVERT_HEAD", "revert"], ["sequencer", "sequencer operation"],
  ["BISECT_START", "bisect"], ["MERGE_AUTOSTASH", "merge autostash"], ["index.lock", "index lock"],
];
function present(path: string): boolean {
  try { lstatSync(path); return true; }
  catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return false; throw error; }
}
async function gitDir(git: Run, cwd: string): Promise<string> {
  const dir = (await checked(git, ["rev-parse", "--absolute-git-dir"], cwd)).trimEnd();
  if (!dir.startsWith("/")) throw new Error(`Cannot locate the Git directory for ${cwd}.`);
  return dir;
}
async function checkIdle(git: Run, cwd: string): Promise<void> {
  const dir = await gitDir(git, cwd);
  for (const [marker, operation] of OPERATIONS) if (present(join(dir, marker))) {
    throw new Error(`The checkout at ${cwd} has an ongoing Git ${operation}. Finish it before preparing to land.`);
  }
}
export async function checkSource(git: Run, context: BuildContext): Promise<void> {
  const { worktree, repoRoot } = context.session;
  const args = ["rev-parse", "--path-format=absolute", "--git-common-dir"];
  const common = (await checked(git, args, worktree)).trimEnd();
  if (!common || common !== (await checked(git, args, repoRoot)).trimEnd() ||
      (await checked(git, ["rev-parse", "--show-prefix"], worktree)).trimEnd()) {
    throw new Error("The build worktree is no longer a checkout of the session's repository.");
  }
  await checkIdle(git, worktree);
  await checkIndexVisibility(git, worktree);
  if ((await checked(git, ["status", "--porcelain", "--untracked-files=normal", "--ignore-submodules=none"], worktree)).trim()) {
    throw new Error("Commit or stash uncommitted build changes before landing.");
  }
}

export function baseMoved(target: LandingTarget, tip: string, actual: string): CandidateMoved {
  return new CandidateMoved(`The base ${target.name} moved from ${short(tip)} to ${short(actual)}. ${PREPARATION_NEEDED}`,
    { state: "base_moved", movedTo: actual });
}

/** Return the integration checkout, if any. Bare build sessions only land into a clean primary checkout. */
export async function checkDestination(git: Run, context: BuildContext, target: LandingTarget, base: string, head?: string): Promise<string | undefined> {
  const { session } = context;
  checkLandingEnvironment();
  const output = await checked(git, ["worktree", "list", "--porcelain", "-z"], session.worktree);
  const entries = output.split("\0\0").filter(Boolean).map(entry => entry.split("\0"));
  if (!entries.length || entries.some(entry => !entry[0]?.startsWith("worktree "))) throw new Error("Cannot locate the repository's worktrees.");
  const matches = entries.filter(entry => entry.includes(`branch ${target.ref}`));
  if (matches.length > 1) throw new Error(`The base ${target.name} is checked out in multiple worktrees.`);
  const selected = session.bare ? entries[0] : matches[0];
  if (!selected) return undefined;
  const checkout = selected[0]!.slice("worktree ".length);
  if (selected.some(line => line === "bare" || line.startsWith("prunable"))) throw new Error(`The base checkout at ${checkout} is unavailable.`);
  const branch = await git(["symbolic-ref", "--quiet", "HEAD"], { cwd: checkout });
  if (branch.code !== 0 || branch.stdout.trim() !== target.ref) {
    throw new Error(`The ${session.bare ? "repository's primary checkout" : "base checkout"} must be on ${target.name}; found ${branch.stdout.trim().replace(/^refs\/heads\//, "") || "a detached HEAD"}.`);
  }
  await checkIdle(git, checkout);
  if (session.bare) {
    if ((await checked(git, ["status", "--porcelain", "--untracked-files=no", "--ignore-submodules=none"], checkout)).trim()) {
      throw new Error("The repository's primary checkout has tracked changes; commit or stash them before landing.");
    }
  } else if (head) {
    const dirty = [...new Set([
      ...paths(await checked(git, ["diff", "--name-only", "-z", "--no-renames", "--"], checkout)),
      ...paths(await checked(git, ["diff", "--cached", "--name-only", "-z", "--no-renames", "--"], checkout)),
    ])];
    if (dirty.length) {
      const incoming = new Set(paths(await checked(git, ["diff", "--name-only", "-z", "--no-renames", `${base}..${head}`, "--"], session.worktree)));
      const overlap = dirty.filter(path => incoming.has(path));
      if (overlap.length) throw new Error(`The checkout of ${target.name} at ${checkout} has uncommitted changes to files this candidate changes: ${overlap.slice(0, 5).map(path => JSON.stringify(path)).join(", ")}${overlap.length > 5 ? ` and ${overlap.length - 5} more` : ""}. Commit or stash them before landing.`);
    }
  }
  const actual = await commit(git, "HEAD", checkout);
  if (actual !== base) throw baseMoved(target, base, actual);
  await checkCheckoutSupport(git, checkout);
  if (head) await checkUntracked(git, checkout, base, head);
  return checkout;
}

const paths = (text: string) => text.split("\0").filter(Boolean);

async function checkUntracked(git: Run, cwd: string, base: string, head: string): Promise<void> {
  const incoming = new Set(paths(await checked(git, ["diff", "--name-only", "-z", "--no-renames", "--diff-filter=AMT", `${base}..${head}`, "--"], cwd)));
  if (!incoming.size) return;
  // Deliberately omit --exclude-standard (ignored files are still user data).
  // Collapse untracked directories so e.g. a large node_modules stays bounded.
  const others = paths(await checked(git, ["ls-files", "--others", "--directory", "--no-empty-directory", "-z"], cwd));
  const files = new Set(others.filter(path => !path.endsWith("/")));
  const dirs = new Set(others.filter(path => path.endsWith("/")).map(path => path.slice(0, -1)));
  const obstruction = (path: string) => { throw new Error(`The landing checkout at ${cwd} has an untracked or ignored obstruction at ${JSON.stringify(path)}. Move it before landing.`); };
  for (const path of incoming) {
    const parts = path.split("/");
    if (parts.some(part => !part || part === "." || part === ".." || part.includes("\uFFFD"))) throw new Error("Cannot safely check an incoming Git path for untracked obstructions.");
    let prefix = "", collapsed = false;
    for (const part of parts) {
      prefix += (prefix ? "/" : "") + part;
      if (files.has(prefix) || prefix === path && dirs.has(prefix)) obstruction(prefix);
      // Git reports only the containing directory when it collapses an untracked
      // subtree. An unrelated file under it is fine; an existing incoming file
      // (or non-directory ancestor) would be overwritten.
      if (collapsed) {
        try {
          if (!lstatSync(join(cwd, prefix)).isDirectory() || prefix === path) obstruction(prefix);
        } catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") break; throw error; }
      }
      collapsed ||= dirs.has(prefix);
    }
  }
  // Also reject replacing a directory which contains untracked files or directories.
  for (const path of others) for (let end = path.indexOf("/"); end !== -1; end = path.indexOf("/", end + 1)) {
    if (incoming.has(path.slice(0, end))) obstruction(path);
  }
}

/** Rebase only onto the captured tip, and abort only a rebase started by this invocation. */
export async function rebaseCandidate(git: Run, cwd: string, base: string, source: string): Promise<void> {
  const dir = await gitDir(git, cwd);
  const result = await git(["-c", "rerere.enabled=false", "rebase", "--no-autostash", "--no-update-refs", "--no-fork-point", base], { cwd });
  if (result.code === 0) return;
  const unmerged = await git(["diff", "--name-only", "-z", "--diff-filter=U"], { cwd });
  const conflicts = unmerged.code === 0 ? paths(unmerged.stdout) : [];
  let recovery = "Inspect this checkout before trying again.";
  let restored = false;
  if (present(join(dir, "rebase-merge")) || present(join(dir, "rebase-apply"))) {
    const abort = await git(["rebase", "--abort"], { cwd });
    if (abort.code) recovery = `Rebase abort failed: ${abort.stderr.trim() || abort.stdout.trim()}. Inspect and recover the worktree manually.`;
    else if (await commit(git, "HEAD", cwd).catch(() => undefined) === source) {
      restored = true;
      recovery = "The rebase was aborted; the branch is unchanged.";
    }
  }
  throw new PreparationError(conflicts.length
    ? `Rebase conflicts: ${conflicts.slice(0, 10).map(path => JSON.stringify(path)).join(", ")}${conflicts.length > 10 ? ` and ${conflicts.length - 10} more` : ""}. ${recovery}`
    : `Rebase failed: ${result.stderr.trim() || result.stdout.trim() || `exit ${result.code}`}. ${recovery}`,
  conflicts.length ? { state: restored ? "conflicts" : "failed", conflicts: conflicts.slice(0, 100) } : {});
}
