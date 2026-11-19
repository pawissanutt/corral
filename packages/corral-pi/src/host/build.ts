import { randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";
import { realpathSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { ProviderId } from "../auth/providers.ts";
import { isApiConnection } from "../auth/connections.ts";
import type { CapabilityGuidance } from "../capabilities/guidance.ts";
import { LocalControlPlane } from "../build/control-plane.ts";
import { readBuildLaunch } from "../build/launch.ts";
import { createSession, loadSession, saveSession, sessionDir } from "../build/session-store.ts";
import { createTodoLedger } from "../build/todo-ledger.ts";
import { prepareWorktree } from "../build/worktree.ts";
import type { BuildContext, BuildLaunch, BuildSessionRecord } from "../build/types.ts";
import { LaunchMessageInvalid, type HostCapabilities, type HostConfig } from "./config.ts";
import { runHost } from "./main.ts";
import { requireBareProvider } from "./bare.ts";
import { createBuildBudgetRecord } from "../build/paid.ts";

// Everything here runs in the host, its own managed processes, or the local receipt plane.
// write_permissions needs assistant scratch ownership; checkpoints needs a worker run;
// hooks is a worker-only control;
// session_export needs corral artifacts; headless needs a bound launcher lifecycle;
// shadow and expert need PM state; role_enabled is planning-role admission;
// llama_cpp.enabled needs a generation-bound local inference route. budget is not an
// optional build capability: local accounting is unlimited and handled for every request.
// context_files and prompt_templates are excluded to avoid opting a standalone build
// into the bound project's context-file and prompt-template discovery policy.
export const BUILD_CAPABILITY_KEYS: readonly string[] = [
  "todo", "queue_control", "web.enabled", "mcp.enabled", "compaction", "browser", "subagents", "oracle",
  "ask_user", "codemode", "tool_search", "lsp", "skills", "bash_rules", "background_shell",
];

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

const resumeCommand = (id: string) => `\`corral pi --resume ${id}\``;

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
      "Put the build's worktree back at that path, or start a new build with `corral pi --build`.");
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
      record.branch ? `Run \`git -C ${record.worktree} switch ${record.branch}\`, then ${resume}.` : "Start a new build with `corral pi --build`.");
  }
}

/** Build the same rich-host configuration for a new build and an explicitly resumed one. */
export function buildHostConfig(launch: BuildLaunch, record: BuildSessionRecord, assets: string): HostConfig {
  const dir = sessionDir(launch.dataRoot, record.id);
  // readBuildLaunch has validated these snapshots with the rich-launch parsers.
  const snapshot = launch.capabilities as HostCapabilities | undefined;
  const guidanceSnapshot = launch.guidance as CapabilityGuidance | undefined;
  const capabilities: HostCapabilities = {
    catalog: snapshot?.catalog ?? 10, digest: snapshot?.digest ?? "0".repeat(64), preset: snapshot?.preset ?? "default",
    enabled: snapshot?.enabled.filter(key => BUILD_CAPABILITY_KEYS.includes(key)) ?? [],
    ...(snapshot?.web ? { web: snapshot.web } : {}), ...(snapshot?.mcp ? { mcp: snapshot.mcp } : {}),
  };
  const guidance = guidanceSnapshot ? { ...guidanceSnapshot, settings: Object.fromEntries(
    Object.entries(guidanceSnapshot.settings).filter(([key]) => BUILD_CAPABILITY_KEYS.includes(key)
      || key === "web" && capabilities.enabled.includes("web.enabled") || key === "mcp" && capabilities.enabled.includes("mcp.enabled"))) } : undefined;
  return {
    identity: { store: "build", project: record.project ?? "", role: "build", generation: 1 },
    secrets: { hostCap: randomBytes(32).toString("base64url"), ctlCap: randomBytes(32).toString("base64url"), bindNonce: randomBytes(32).toString("base64url") },
    pane: "", cwd: record.worktree, root: record.workspaceRoot ?? record.repoRoot, corral: launch.corral ?? "", assets,
    sessionDir: join(dir, "sessions"), runtimeDir: join(dir, "runtime"), ui: "rich",
    ...(record.sessionFile ? { resumeSession: record.sessionFile } : {}), capabilities,
    ...(guidance ? { guidance } : {}), todo: capabilities.enabled.includes("todo"),
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
  if (error instanceof LaunchMessageInvalid) {
    return [`The build host received no valid launch message (${message}).`, "Start a build with `corral pi --build`; the host is not run on its own."];
  }
  return [`corral pi --build stopped: ${message}`, resume ? `Run ${resumeCommand(resume)} to try again.` : "Run `corral pi --build` to try again."];
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
    if (argv.length !== 2 || argv[0] !== "--launch-fd" || !/^[0-9]+$/.test(argv[1] ?? "")) {
      throw new LaunchMessageInvalid("run with --launch-fd <n>");
    }
    const fd = Number(argv[1]);
    if (!Number.isSafeInteger(fd) || fd < 3) throw new LaunchMessageInvalid("invalid launch descriptor");
    const launch = readBuildLaunch(fd);
    resume = launch.resume;
    if (launch.bare && !(launch.binding && isApiConnection(launch.binding.connection))) requireBareProvider(launch.provider as ProviderId);
    if (launch.provider === "llama.cpp") {
      throw new LaunchRefused("Local inference is unavailable in a build session", "Local inference (llama.cpp) is unavailable in a build session.",
        "Choose another provider with `corral pi --build --provider <provider>`.");
    }
    let record: BuildSessionRecord;
    if (launch.resume) {
      const id = launch.resume;
      try { record = loadSession(launch.dataRoot, id); }
      catch (error) {
        const detail = error instanceof Error ? error.message : String(error);
        throw new LaunchRefused(detail, `Build ${id} cannot resume: its session record is missing or unreadable (${detail}).`,
          "Pick a recent build in the `corral pi` launcher, or start a new one with `corral pi --build`.");
      }
      if ((record.bare === true) !== (launch.bare === true)) throw new LaunchMessageInvalid("resume bare marker differs from the saved session");
      validateBuildResume(record);
      if (!record.sessionFile) {
        throw new LaunchRefused("The build session has no recorded transcript to resume", `Build ${id} has no recorded conversation to resume.`,
          "Start a new build with `corral pi --build`.");
      }
      if (launch.paid && launch.paid.session !== record.id) throw new LaunchMessageInvalid("paid Build session differs from saved lineage");
      // Spending caps are disabled: an approval/allowance edit does not block resume.
      if (record.route?.paid && (!launch.route?.paid || record.route.binding?.digest !== launch.route.binding?.digest)) {
        throw new LaunchMessageInvalid("paid Build route cannot change");
      }
      record = { ...record, ...(launch.route ? { route: launch.route } : {}) };
      saveSession(launch.dataRoot, record);
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
          taken ? "Choose another name with `corral pi --build --name <name>`, or resume that build from the `corral pi` launcher."
            : "Fix that Git error, or work in the checkout itself with `corral pi --build --here`.");
      }
      saveSession(launch.dataRoot, record);
    }
    const dir = sessionDir(launch.dataRoot, record.id);
    const paidRecord = createBuildBudgetRecord(launch);
    const build: BuildContext = { session: record, settings: launch.settings, dir, corral: launch.corral ?? "",
      ...(paidRecord ? { paidRecord } : {}),
      ...(launch.profileCatalog ? { profileCatalog: launch.profileCatalog } : {}),
      ...(launch.profileRequestPath ? { profileRequestPath: launch.profileRequestPath } : {}) };
    const config = buildHostConfig(launch, record, assets);
    const ledger = createTodoLedger(dir);
    // Pi ends an ordinary quit with process.exit(0); this prints after the terminal is restored.
    const session = record;
    process.on("exit", code => {
      if (code !== 0) return;
      const lines = quitLines(launch.dataRoot, session);
      if (lines.length) process.stdout.write(`${lines.join("\n")}\n`);
    });
    await runHost(config, { bridge: false, build, client: (identity, sessionId) => new LocalControlPlane({
      dir, root: config.root, identity, sessionId, handlers: { todo: ledger.handle }, todoSnapshot: () => ledger.snapshot(),
    }) });
  } catch (error) {
    process.stderr.write(`${refusalLines(error, resume).join("\n")}\n`);
    process.exit(2);
  }
}

if (import.meta.main) await buildCommand(process.argv.slice(2), fileURLToPath(new URL("../..", import.meta.url)).replace(/\/$/, ""));
