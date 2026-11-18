import { spawnSync } from "node:child_process";
import { realpathSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import type { ProviderId } from "../auth/providers.ts";
import { parseBuildFlags, type LocalBuildOptions } from "../build/launch.ts";
import { createSession, loadSession, saveSession, sessionDir } from "../build/session-store.ts";
import { prepareWorktree } from "../build/worktree.ts";
import type { BuildContext, BuildLaunch, BuildSessionRecord } from "../build/types.ts";
import { sessionSocket, workspaceSettings, type HostConfig } from "./config.ts";
import { runHost } from "./main.ts";
import { requireBareProvider } from "./bare.ts";

/** A refused launch or resume: the error's message keeps the code and detail; `what` and `fix` are what the owner reads. */
class LaunchRefused extends Error {
  constructor(message: string, readonly what: string, readonly fix: string) { super(message); }
}

class BuildResumeRefused extends LaunchRefused {
  readonly code = "build_resume_refused";
  constructor(record: BuildSessionRecord, reason: string, what: string, fix: string) {
    super(`build_resume_refused: ${record.worktree}: ${reason}`, `Build ${record.id} cannot resume: ${what}`, fix);
  }
}

const startCommand = "corral-pi build";
const resumeCommand = (id: string) => `\`corral-pi build --resume ${id}\``;

/** Verify the recorded checkout before opening its transcript or binding any tools. */
export function validateBuildResume(record: BuildSessionRecord): void {
  const resume = resumeCommand(record.id);
  if (record.bare && record.repository === false) {
    if (!statSync(record.worktree).isDirectory()) throw new BuildResumeRefused(record, "session directory is missing",
      `${record.worktree} is no longer a directory.`, `Restore that directory, then run ${resume}.`);
    return;
  }
  const commonDirectory = (path: string): string => {
    const result = spawnSync("git", ["-C", path, "rev-parse", "--git-common-dir"], { encoding: "utf8" });
    if (result.error || result.status !== 0) {
      const detail = result.stderr?.trim() || result.error?.message || `exit ${result.status}`;
      throw new BuildResumeRefused(record, `cannot read Git common directory for ${path}: ${detail}`,
        `Git cannot read ${path} (${detail}).`, `Check that ${path} still exists and is a Git checkout, then run ${resume}.`);
    }
    try { return realpathSync(resolve(path, result.stdout.trim())); }
    catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      throw new BuildResumeRefused(record, `cannot resolve Git common directory for ${path}: ${detail}`,
        `Git cannot resolve the repository of ${path} (${detail}).`, `Check that ${path} still exists and is a Git checkout, then run ${resume}.`);
    }
  };
  const expected = commonDirectory(record.repoRoot);
  const observed = commonDirectory(record.worktree);
  if (observed !== expected) {
    throw new BuildResumeRefused(record, `Git common directory differs: expected ${expected}, found ${observed}`,
      `${record.worktree} now belongs to a different Git repository than ${record.repoRoot}.`,
      `Put the build's worktree back at that path, or start a new build with \`${startCommand}\`.`);
  }
  if (record.here) return;
  const result = spawnSync("git", ["-C", record.worktree, "symbolic-ref", "--quiet", "--short", "HEAD"], { encoding: "utf8" });
  if (result.error || result.status !== 0 && result.status !== 1) {
    const detail = result.stderr?.trim() || result.error?.message || `exit ${result.status}`;
    throw new BuildResumeRefused(record, `cannot read checked-out branch: ${detail}`,
      `Git cannot read the branch checked out in ${record.worktree} (${detail}).`, `Check that worktree, then run ${resume}.`);
  }
  const branch = result.status === 0 ? result.stdout.trim() : null;
  if (!record.branch || branch !== record.branch) {
    const found = branch ? `branch ${branch}` : "a detached HEAD";
    throw new BuildResumeRefused(record, `branch differs: expected ${record.branch ?? "a recorded build branch"}, found ${branch ?? "detached HEAD"}`,
      record.branch ? `${record.worktree} has ${found} checked out, not ${record.branch}.` : `${record.worktree} has no recorded build branch.`,
      record.branch ? `Run \`git -C ${record.worktree} switch ${record.branch}\`, then ${resume}.` : `Start a new build with \`${startCommand}\`.`);
  }
}

/** Build the same rich-host configuration for a new build and an explicitly resumed one. */
export function buildHostConfig(launch: BuildLaunch, record: BuildSessionRecord, assets: string, local: LocalBuildOptions): HostConfig {
  const dir = sessionDir(launch.dataRoot, record.id);
  return {
    identity: { store: "build", project: record.project ?? "", role: "build" },
    pane: "", cwd: record.worktree, root: record.workspaceRoot ?? record.repoRoot, corral: launch.corral ?? "", assets,
    sessionDir: join(dir, "sessions"), runtimeDir: join(dir, "runtime"), ui: "rich",
    ...workspaceSettings(record.workspaceRoot ?? record.repoRoot, "build"),
    local: { dir, id: record.id, socket: local.socket ?? sessionSocket(dir, process.env), headless: local.headless },
    provider: (record.route?.provider ?? launch.provider) as ProviderId, model: record.route?.model ?? launch.model,
    ...((record.route?.effort ?? launch.effort) ? { effort: record.route?.effort ?? launch.effort } : {}),
    ...((record.route?.binding ?? launch.binding) ? { binding: record.route?.binding ?? launch.binding } : {}),
    ...(launch.aws ? { aws: launch.aws } : {}),
  };
}

/** The two lines a stopped launch prints: what happened, then the command or action that fixes it. */
function refusalLines(error: unknown, resume: string | undefined): readonly [string, string] {
  if (error instanceof LaunchRefused) return [error.what, error.fix];
  const message = error instanceof Error ? error.message : String(error);
  const command = startCommand;
  return [`${command} stopped: ${message}`, resume ? `Run ${resumeCommand(resume)} to try again.`
    : "See `corral-pi build --help`."];
}

/** What an ordinary quit leaves behind when no exit took the work elsewhere: where it is and how to come back to it. */
function quitLines(dataRoot: string, fallback: BuildSessionRecord): readonly string[] {
  let record = fallback;
  try { record = loadSession(dataRoot, fallback.id); } catch { /* The record read at launch still names the work. */ }
  const where = `${record.worktree}${record.repository === false ? " (outside Git)" : ` on ${record.branch ? `branch ${record.branch}` : "a detached HEAD"}`}`;
  if (!record.exit) {
    return [`Build ${record.name} stopped without an exit; its work stays in ${where}.`,
      `Resume it with ${resumeCommand(record.id)}, then choose an exit with /finish.`];
  }
  if (record.exit.choice === "keep") return [`Build ${record.name} is kept in ${where}.`, `Resume it with ${resumeCommand(record.id)}.`];
  return [];
}

export async function buildCommand(argv: readonly string[], assets: string): Promise<void> {
  let resume: string | undefined;
  try {
    const { launch, local } = parseBuildFlags(argv, assets);
    resume = launch.resume;
    if (launch.provider === "openai") throw new Error("Metered builds are unavailable until the local spending cap is supported");
    if (launch.bare) requireBareProvider(launch.provider as ProviderId);
    if (launch.provider === "llama.cpp") {
      throw new LaunchRefused("Local inference is unavailable in a build session", "Local inference (llama.cpp) is unavailable in a build session.",
        `Choose another provider with \`${startCommand} --provider <provider>\`.`);
    }
    let record: BuildSessionRecord;
    if (launch.resume) {
      const id = launch.resume;
      try { record = loadSession(launch.dataRoot, id); }
      catch (error) {
        const detail = error instanceof Error ? error.message : String(error);
        throw new LaunchRefused(detail, `Build ${id} cannot resume: its session record is missing or unreadable (${detail}).`,
          `Use a recorded session id with ${resumeCommand("<id>")}, or start a new one with \`${startCommand}\`.`);
      }
      if ((record.bare === true) !== (launch.bare === true)) throw new Error("Resume must use the same workspace association as the saved build session");
      validateBuildResume(record);
      if (!record.sessionFile) {
        throw new LaunchRefused("The build session has no recorded transcript to resume", `Build ${id} has no recorded conversation to resume.`,
          `Start a new build with \`${startCommand}\`.`);
      }
      if (record.route?.paid) throw new Error("Metered builds are unavailable until the local spending cap is supported");
    } else {
      const draft = createSession(launch, { worktree: launch.repoRoot, branch: null, base: null });
      try {
        if (launch.bare) {
          const repository = spawnSync("git", ["-C", launch.repoRoot, "rev-parse", "--show-toplevel"], { encoding: "utf8" });
          if (repository.status !== 0) {
            if (!repository.stderr?.includes("not a git repository")) throw new Error(repository.error?.message ?? repository.stderr.trim());
            record = { ...draft, here: true, repository: false };
          } else {
            const branch = spawnSync("git", ["-C", launch.repoRoot, "symbolic-ref", "--quiet", "--short", "HEAD"], { encoding: "utf8" });
            if (branch.status !== 0 && branch.status !== 1) throw new Error(branch.error?.message ?? branch.stderr.trim());
            record = { ...draft, ...prepareWorktree(launch.repoRoot, draft.name, launch.here), repository: true,
              baseBranch: branch.status === 0 ? branch.stdout.trim() : null };
          }
        } else record = { ...draft, ...prepareWorktree(launch.repoRoot, draft.name, launch.here) };
      }
      catch (error) {
        const detail = error instanceof Error ? error.message : String(error);
        const taken = /^Build (worktree|branch) already exists: (.*)$/.exec(detail);
        throw new LaunchRefused(detail, taken ? `A build ${taken[1]} already exists: ${taken[2]}.` : `The build worktree could not be created: ${detail}.`,
          taken ? `Choose another name with \`${startCommand} --name <name>\`, or resume the existing build.`
            : `Fix that Git error, or work in the checkout itself with \`${startCommand} --here\`.`);
      }
      saveSession(launch.dataRoot, record);
    }
    const dir = sessionDir(launch.dataRoot, record.id);
    const build: BuildContext = { session: record, settings: launch.settings, dir, corral: launch.corral ?? "" };
    const config = buildHostConfig(launch, record, assets, local);
    // Pi ends an ordinary quit with process.exit(0); this prints after the terminal is restored.
    const session = record;
    process.on("exit", code => {
      if (code !== 0) return;
      const lines = quitLines(launch.dataRoot, session);
      if (lines.length) process.stdout.write(`${lines.join("\n")}\n`);
    });
    await runHost(config, { build });
  } catch (error) {
    process.stderr.write(`${refusalLines(error, resume).join("\n")}\n`);
    process.exit(2);
  }
}

