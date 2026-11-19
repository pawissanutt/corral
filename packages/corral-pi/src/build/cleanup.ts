import { spawnSync } from "node:child_process";
import { lstatSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { join, sep } from "node:path";
import { canConnect } from "../bridge/server.ts";
import { sessionSocket } from "../host/config.ts";
import { landingBaseFromRef, landingBaseLookups, landingBaseName } from "./git.ts";
import { listSessions, loadSession, saveSession, sessionDir } from "./session-store.ts";
import type { BuildSessionRecord } from "./types.ts";

export type CleanupPlan = { readonly reason: string } | { readonly worktree: boolean; readonly branches: readonly string[] };

type GitResult = { readonly status: number | null; readonly stdout: string; readonly stderr: string };
function git(cwd: string, args: readonly string[]): GitResult {
  const result = spawnSync("git", ["--no-optional-locks", "-C", cwd, ...args], { cwd, encoding: "utf8" });
  if (result.error) throw result.error;
  return result;
}

function checked(result: GitResult): string {
  if (result.status !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || `Git exited ${result.status}`);
  return result.stdout;
}

function exists(path: string): boolean {
  try { lstatSync(path); return true; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
}

function baseBranch(record: BuildSessionRecord): string | undefined {
  if (record.exit?.landed && record.exit.baseBranch) return record.exit.baseBranch;
  let base = landingBaseName(record);
  if (record.bare || !record.workspaceRoot || !record.project) return base;
  for (const lookup of landingBaseLookups) {
    if (base) break;
    const result = git(record.repoRoot, lookup);
    if (result.status === 0) base = landingBaseFromRef(result.stdout, lookup);
  }
  return base;
}

export function inspectBuildCleanup(record: BuildSessionRecord): CleanupPlan {
  if (record.here || record.repository === false) return { reason: "works in the checkout itself" };
  if (record.cleaned) return { reason: "build checkout already removed; record retained" };
  const entries = checked(git(record.repoRoot, ["worktree", "list", "--porcelain", "-z"]))
    .split("\0\0").map(entry => entry.split("\0"));
  const index = entries.findIndex(entry => entry[0] === `worktree ${record.worktree}`);
  if (index === 0 || record.worktree === record.repoRoot) return { reason: "does not own this checkout" };
  const present = exists(record.worktree);
  if (present) {
    if (index < 0) return { reason: "path is not a registered worktree" };
    // Do not mistake an enclosing repository or a replacement checkout for the registered one.
    const [top, common] = checked(git(record.worktree, ["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-common-dir"])).trimEnd().split("\n");
    if (top !== record.worktree || common !== checked(git(record.repoRoot, ["rev-parse", "--path-format=absolute", "--git-common-dir"])).trimEnd()) {
      return { reason: "worktree belongs to a different checkout" };
    }
    if (checked(git(record.worktree, ["status", "--porcelain", "--untracked-files=normal"]))) return { reason: "uncommitted changes" };
    const base = baseBranch(record);
    if (!base) return { reason: "no landing base branch" };
    const commits = checked(git(record.worktree, ["cherry", `refs/heads/${base}`, "HEAD"]))
      .split("\n").filter(line => line.startsWith("+ ")).length;
    if (commits) return { reason: `${commits} ${commits === 1 ? "commit" : "commits"} not on ${base}` };
    if (entries[index]?.includes(`branch refs/heads/${base}`) || record.branch === base) return { reason: `would remove the landing base ${base}` };
  }
  const attached = entries[index]?.find(field => field.startsWith("branch refs/heads/"))?.slice("branch refs/heads/".length);
  const branches: string[] = [];
  for (const branch of [record.branch, attached]) {
    if (!branch || branches.includes(branch)) continue;
    const ref = git(record.repoRoot, ["show-ref", "--verify", "--quiet", `refs/heads/${branch}`]);
    if (ref.status === 1) continue;
    checked(ref);
    branches.push(branch);
  }
  // Git can unregister a missing linked worktree with the same non-forced remove command.
  return { worktree: index >= 0, branches };
}

export function hasOwnerMessage(dataRoot: string, record: BuildSessionRecord): boolean {
  const journal = join(sessionDir(dataRoot, record.id), "journal.json");
  try {
    const value = JSON.parse(readFileSync(journal, "utf8"));
    if (!Array.isArray(value?.intent)) throw new Error(`Invalid owner intent in ${journal}`);
    if (value.intent.length) return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  if (!record.sessionFile) return false;
  // Older socket input may not have been captured by the journal. Preserve its transcript.
  const transcript = readFileSync(record.sessionFile, "utf8");
  for (let start = 0; start < transcript.length;) {
    const end = transcript.indexOf("\n", start);
    const line = end < 0 ? transcript.slice(start) : transcript.slice(start, end);
    if (line) {
      const entry = JSON.parse(line);
      if (entry?.type === "message" && entry.message?.role === "user") return true;
    }
    if (end < 0) break;
    start = end + 1;
  }
  return false;
}

export function removeBuild(dataRoot: string, record: BuildSessionRecord,
  plan: Exclude<CleanupPlan, { reason: string }>, removeRecord: boolean, dryRun = false): string {
  const dir = sessionDir(dataRoot, record.id);
  const removed: string[] = [];
  try {
    if (plan.worktree) {
      if (exists(record.worktree)) {
        const worktree = realpathSync(record.worktree);
        const paths = !removeRecord && record.sessionFile ? [dir, record.sessionFile] : [dir];
        if (paths.some(path => {
          const actual = realpathSync(path);
          return actual === worktree || actual.startsWith(worktree + sep);
        })) throw new Error("worktree contains the session record or transcript");
      }
      if (!dryRun) checked(git(record.repoRoot, ["worktree", "remove", "--", record.worktree]));
      removed.push(`worktree ${record.worktree} (including ignored build output)`);
    }
    for (const branch of plan.branches) {
      if (!dryRun) checked(git(record.repoRoot, ["branch", "-d", "--", branch]));
      removed.push(`branch ${branch}`);
    }
    if (removeRecord) {
      if (!dryRun) rmSync(dir, { recursive: true });
      removed.push(`record ${dir}`);
    } else if (!dryRun) saveSession(dataRoot, { ...record, cleaned: true });
  } catch (error) {
    throw new Error(`${removed.length ? `removed ${removed.join(", ")}; stopped: ` : ""}${errorMessage(error)}`);
  }
  if (!removed.length) return "no worktree or branch remains; record kept";
  return `${dryRun ? "would remove" : "removed"} ${removed.join(", ")}${removeRecord ? "" : "; record kept"}`;
}

function livePid(record: BuildSessionRecord): boolean {
  if (!record.pid) return false;
  try { process.kill(record.pid, 0); return true; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ESRCH") return false;
    throw error;
  }
}

export async function buildIsRunning(dataRoot: string, record: BuildSessionRecord): Promise<boolean> {
  const socket = record.socket ?? sessionSocket(sessionDir(dataRoot, record.id), process.env);
  // A listener is live even before hello is ready. A PID covers startup and old --socket records.
  return await canConnect(socket) || livePid(record);
}

export function errorMessage(error: unknown): string {
  return (error instanceof Error ? error.message : String(error)).replace(/\r?\n/g, "; ");
}

export async function pruneBuilds(dataRoot: string, dryRun: boolean, write: (line: string) => void): Promise<void> {
  const sessions = listSessions(dataRoot);
  const owners = new Map<string, BuildSessionRecord>();
  for (const record of sessions) {
    if (record.here || record.repository === false || record.cleaned) continue;
    const previous = owners.get(record.worktree);
    if (!previous || record.created > previous.created) owners.set(record.worktree, record);
  }
  let kept = 0, pruned = 0, refused = 0;
  for (const listed of sessions) {
    const say = (detail: string) => write(`${listed.id} (${listed.name}): ${detail}`);
    try {
      const record = loadSession(dataRoot, listed.id);
      if (await buildIsRunning(dataRoot, record)) {
        kept++; say("kept (running)"); continue;
      }
      if (!record.here && record.repository !== false && !record.cleaned && owners.get(record.worktree)?.id !== record.id) {
        kept++; say("kept (newer session owns this worktree path)"); continue;
      }
      const plan = inspectBuildCleanup(record);
      if ("reason" in plan) { kept++; say(`kept (${plan.reason})`); continue; }
      const removeRecord = !hasOwnerMessage(dataRoot, record);
      const current = loadSession(dataRoot, record.id);
      if (livePid(current)) { kept++; say("kept (running)"); continue; }
      if (current.pid !== record.pid || current.updated !== record.updated || current.cleaned !== record.cleaned) {
        kept++; say("kept (session changed during inspection)"); continue;
      }
      say(removeBuild(dataRoot, record, plan, removeRecord, dryRun));
      if (plan.worktree || plan.branches.length || removeRecord) pruned++; else kept++;
    } catch (error) { refused++; say(`cleanup refused: ${errorMessage(error)}`); }
  }
  write(`${pruned} ${dryRun ? "would prune" : "pruned"}, ${kept} kept, ${refused} refused`);
}
